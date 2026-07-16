//! Background send/recv RTP tasks for `engine::MediaEngine` -- split out of
//! `engine.rs` purely for file size (same precedent as `views/settings/`,
//! `views/dialer/`, `sip-core/src/call/lifecycle/`), not a behavior change.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;

use deelip_sip::zrtp::{Role, is_zrtp_packet};

use crate::aec::EchoCanceller;
use crate::agc::AutomaticGainControl;
use crate::audio::{CaptureRx, EchoRefBuf, FRAME_SAMPLES, PlaybackTx, SharedGain};
use crate::codec_dispatch::{AudioDecoder, AudioEncoder};
use crate::dtls_demux::is_dtls_packet;
use crate::dtls_srtp_session::DtlsSrtpOutcome;
use crate::dtmf::{DTMF_PAYLOAD_TYPE, INBAND_FRAME_COUNT, build_dtmf_burst, char_to_event, dtmf_tone_frame};
use crate::engine::RtpSocket;
use crate::recording::RecordingWriter;
use crate::rtp::{RtpPacket, RtpSender};
use crate::stats::{JitterTracker, SharedStats};
use crate::vad::{ComfortNoiseState, VadDecision, VoiceActivityDetector, synthesize_comfort_noise};
use crate::zrtp_session::{ZrtpOutcome, ZrtpRuntime};

/// Which leg a send/stats update belongs to -- `Two` is a no-op on the
/// stats side when `CallStatsSnapshot.leg2` is `None` (single-call, not a
/// conference), matching every call site's previous individual handling.
pub(crate) enum Leg {
    One,
    Two,
}

/// Encrypts `bytes` via `ctx` (if `Some` -- SRTP negotiated) and sends the
/// result to `remote` over `sock`, updating `stats`' leg1/leg2 packet/byte
/// counters on a successful send. Returns `Err(())` if either encryption or
/// the send itself failed (already logged via `tracing::error!`, tagged with
/// `what`) -- callers decide what "skip this frame/packet/leg" means for
/// their own loop. See docs/crates/media-engine.md for why DTMF sends now
/// also count toward `stats` (deliberate, not an oversight to undo).
pub(crate) async fn encrypt_and_send(
    ctx: Option<&mut SrtpContext>, bytes: &[u8], sock: &RtpSocket, remote: SocketAddr, stats: &SharedStats, leg: Leg,
    what: &str,
) -> Result<(), ()> {
    let encrypted;
    let out: &[u8] = match ctx {
        Some(ctx) => match ctx.encrypt_rtp(bytes) {
            Ok(b) => {
                encrypted = b;
                &encrypted
            }
            Err(e) => {
                error!("SRTP encrypt ({what}): {e}");
                return Err(());
            }
        },
        None => bytes,
    };
    match sock.send_to(out, remote).await {
        Ok(()) => {
            let mut s = stats.lock().unwrap();
            match leg {
                Leg::One => {
                    s.leg1.packets_sent += 1;
                    s.leg1.bytes_sent += out.len() as u64;
                }
                Leg::Two => {
                    if let Some(ls) = s.leg2.as_mut() {
                        ls.packets_sent += 1;
                        ls.bytes_sent += out.len() as u64;
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            error!("RTP send ({what}): {e}");
            Err(())
        }
    }
}

/// Act on whatever `ZrtpRuntime::handle_incoming`/`tick` just produced --
/// see `docs/crates/media-engine.md`'s ZRTP section. The two tasks each own their
/// own `SrtpContext`, so `encrypt_tx` is how a completed handshake's key
/// material reaches the send task from here (the recv task).
pub(crate) async fn handle_zrtp_outcomes(
    outcomes: Vec<ZrtpOutcome>, sock: &RtpSocket, remote_rtp: SocketAddr, decrypt_ctx: &mut Option<SrtpContext>,
    encrypt_tx: &mpsc::UnboundedSender<([u8; 16], [u8; 14])>, sas: &Arc<Mutex<Option<String>>>,
) {
    for outcome in outcomes {
        match outcome {
            ZrtpOutcome::SendBytes(bytes) => {
                if let Err(e) = sock.send_to(&bytes, remote_rtp).await {
                    warn!("Failed to send ZRTP packet: {e:#}");
                }
            }
            ZrtpOutcome::Sas(value) => {
                *sas.lock().unwrap() = Some(value);
            }
            ZrtpOutcome::Secure { srtp_key_i, srtp_salt_i, srtp_key_r, srtp_salt_r, role } => {
                let (encrypt_key, encrypt_salt, decrypt_key, decrypt_salt) = match role {
                    Role::Initiator => (srtp_key_i, srtp_salt_i, srtp_key_r, srtp_salt_r),
                    Role::Responder => (srtp_key_r, srtp_salt_r, srtp_key_i, srtp_salt_i),
                };
                match SrtpContext::new(
                    &decrypt_key,
                    &decrypt_salt,
                    ProtectionProfile::Aes128CmHmacSha1_80,
                    Some(srtp_replay_protection(64)),
                    None,
                ) {
                    Ok(ctx) => {
                        debug!("ZRTP: switching to SRTP-encrypted recv");
                        *decrypt_ctx = Some(ctx);
                    }
                    Err(e) => error!("ZRTP: failed to build SRTP decrypt context: {e}"),
                }
                let _ = encrypt_tx.send((encrypt_key, encrypt_salt));
            }
            ZrtpOutcome::Failed(reason) => {
                warn!("ZRTP handshake failed, continuing without encryption: {reason}");
            }
        }
    }
}

/// Bundles the leg-1-only ZRTP state a `recv_loop` needs -- `None` for leg 2
/// (conference legs stay SDES/plain-only, never ZRTP -- see
/// `MediaEngineOptions::zrtp`'s doc comment).
pub(crate) struct ZrtpRecvState {
    pub(crate) runtime: ZrtpRuntime,
    pub(crate) encrypt_tx: mpsc::UnboundedSender<([u8; 16], [u8; 14])>,
    pub(crate) sas: Arc<Mutex<Option<String>>>,
}

/// Bundles the leg-1-only DTLS-SRTP state a `recv_loop` needs -- `None` for
/// leg 2, same restriction/reasoning as `ZrtpRecvState`. See
/// docs/crates/media-engine.md's "DTLS-SRTP session driving" section for how
/// this compares to ZRTP's own in-band, `recv_loop`-driven shape.
pub(crate) struct DtlsRecvState {
    /// Forwards demuxed DTLS-record bytes to `dtls_demux::DemuxConn`, whose
    /// receiving end `run_dtls_handshake`'s background task owns.
    pub(crate) inbound_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(crate) outcome_rx: mpsc::UnboundedReceiver<DtlsSrtpOutcome>,
    pub(crate) encrypt_tx: mpsc::UnboundedSender<([u8; 16], [u8; 14])>,
    /// Used only to actively tear down this call's media on
    /// `DtlsSrtpOutcome::FingerprintMismatch` -- see `handle_dtls_outcome`.
    pub(crate) stop_tx: watch::Sender<bool>,
}

/// Waits on `dtls`'s outcome channel if present, or never resolves if not --
/// lets `recv_loop`'s `tokio::select!` treat "no DTLS-SRTP for this call" and
/// "haven't gotten the (one-shot) outcome yet" uniformly, the same trick
/// `zrtp_retransmit_tick`'s `if zrtp.is_some()` guard uses for ZRTP.
pub(crate) async fn recv_dtls_outcome(dtls: &mut Option<DtlsRecvState>) -> Option<DtlsSrtpOutcome> {
    match dtls {
        Some(state) => state.outcome_rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Acts on `run_dtls_handshake`'s one-shot outcome -- see
/// docs/crates/media-engine.md's "DTLS-SRTP session driving" section for why
/// `FingerprintMismatch` tears down media via `stop_tx` while an ordinary
/// `Failed` just falls back to unencrypted media like ZRTP does.
pub(crate) async fn handle_dtls_outcome(
    outcome: DtlsSrtpOutcome, decrypt_ctx: &mut Option<SrtpContext>,
    encrypt_tx: &mpsc::UnboundedSender<([u8; 16], [u8; 14])>, stop_tx: &watch::Sender<bool>,
) {
    match outcome {
        DtlsSrtpOutcome::Secure { local_key, local_salt, remote_key, remote_salt } => {
            match SrtpContext::new(
                &remote_key,
                &remote_salt,
                ProtectionProfile::Aes128CmHmacSha1_80,
                Some(srtp_replay_protection(64)),
                None,
            ) {
                Ok(ctx) => {
                    debug!("DTLS-SRTP: switching to SRTP-encrypted recv");
                    *decrypt_ctx = Some(ctx);
                }
                Err(e) => error!("DTLS-SRTP: failed to build SRTP decrypt context: {e}"),
            }
            let _ = encrypt_tx.send((local_key, local_salt));
        }
        DtlsSrtpOutcome::FingerprintMismatch => {
            error!(
                "DTLS-SRTP: peer certificate did not match the SDP-advertised fingerprint -- possible \
                 man-in-the-middle, stopping this call's media entirely rather than falling back to plaintext"
            );
            let _ = stop_tx.send(true);
        }
        DtlsSrtpOutcome::Failed(reason) => {
            warn!("DTLS-SRTP handshake failed, continuing without encryption: {reason}");
        }
    }
}

pub(crate) fn push_to_jitter(jitter: &PlaybackTx, pcm: &[i16]) {
    let max = FRAME_SAMPLES * 50; // cap at 1 second
    let mut buf = jitter.lock().unwrap();
    for &s in pcm {
        if buf.len() < max {
            buf.push_back(s);
        }
    }
}

/// Shared receive-loop body for both legs' RTP recv tasks: recv -> (ZRTP
/// packet? hand to `zrtp`, if any, and loop) -> SRTP-decrypt (if
/// `decrypt_ctx` is `Some`) -> parse -> drop DTMF payloads -> stats/jitter
/// -> decode (voice, or synthesize comfort noise if `cn_pt` matches) ->
/// push to `playback`. Mirrors `video_engine.rs`'s own `recv_loop`, which
/// already shares one function between both legs the same way.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn recv_loop(
    sock: Arc<RtpSocket>, mut decrypt_ctx: Option<SrtpContext>, mut decoder: AudioDecoder, dtmf_pt: u8,
    cn_pt: Option<u8>, clock_hz: f64, playback: PlaybackTx, stats: SharedStats, leg: Leg,
    mut zrtp: Option<ZrtpRecvState>, mut dtls: Option<DtlsRecvState>, remote_rtp: SocketAddr,
    mut stop_rx: watch::Receiver<bool>, what: &'static str,
) {
    let mut jitter = JitterTracker::default();
    let mut cn_state = ComfortNoiseState::new();
    let mut buf = vec![0u8; 2048];
    // Always constructed (needed unconditionally for `tokio::select!`'s
    // branch below to type-check) but only actually ticks meaningfully when
    // `zrtp.is_some()`, matching the `if zrtp_runtime.is_some()` guard this
    // replaces.
    let mut zrtp_retransmit_tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            Ok((len, _from)) = sock.recv_from(&mut buf) => {
                if is_zrtp_packet(&buf[..len]) {
                    if let Some(z) = zrtp.as_mut() {
                        let outcomes = z.runtime.handle_incoming(&buf[..len]);
                        handle_zrtp_outcomes(outcomes, &sock, remote_rtp, &mut decrypt_ctx, &z.encrypt_tx, &z.sas).await;
                    }
                    continue;
                }
                if is_dtls_packet(&buf[..len]) {
                    if let Some(d) = dtls.as_ref() {
                        let _ = d.inbound_tx.send(buf[..len].to_vec());
                    }
                    continue;
                }
                let decrypted;
                let plain: &[u8] = if let Some(ctx) = decrypt_ctx.as_mut() {
                    match ctx.decrypt_rtp(&buf[..len]) {
                        Ok(b) => { decrypted = b; &decrypted[..] }
                        Err(e) => { warn!("SRTP decrypt ({what}): {e}"); continue; }
                    }
                } else {
                    &buf[..len]
                };
                if let Some(pkt) = RtpPacket::decode(plain) {
                    if pkt.payload_type == DTMF_PAYLOAD_TYPE || pkt.payload_type == dtmf_pt {
                        continue;
                    }
                    {
                        let mut s = stats.lock().unwrap();
                        match leg {
                            Leg::One => {
                                s.leg1.packets_received += 1;
                                s.leg1.bytes_received   += len as u64;
                                jitter.observe(&mut s.leg1, &pkt, clock_hz);
                            }
                            Leg::Two => {
                                if let Some(ls) = s.leg2.as_mut() {
                                    ls.packets_received += 1;
                                    ls.bytes_received   += len as u64;
                                    jitter.observe(ls, &pkt, clock_hz);
                                }
                            }
                        }
                    }
                    let pcm = if cn_pt == Some(pkt.payload_type) {
                        // Comfort-noise/SID packet -- see `vad`'s module doc
                        // for the inter-SID-gap simplification this represents.
                        let level = pkt.payload.first().copied().unwrap_or(127);
                        synthesize_comfort_noise(level, FRAME_SAMPLES, &mut cn_state)
                    } else {
                        decoder.decode(&pkt.payload)
                    };
                    push_to_jitter(&playback, &pcm);
                }
            }
            _ = zrtp_retransmit_tick.tick(), if zrtp.is_some() => {
                if let Some(z) = zrtp.as_mut() {
                    let outcomes = z.runtime.tick(Instant::now());
                    handle_zrtp_outcomes(outcomes, &sock, remote_rtp, &mut decrypt_ctx, &z.encrypt_tx, &z.sas).await;
                }
            }
            Some(outcome) = recv_dtls_outcome(&mut dtls) => {
                if let Some(d) = dtls.as_ref() {
                    handle_dtls_outcome(outcome, &mut decrypt_ctx, &d.encrypt_tx, &d.stop_tx).await;
                }
                // One-shot: `run_dtls_handshake` only ever sends a single
                // outcome, so this branch must never be selected again --
                // `recv_dtls_outcome` treats `None` as "nothing to wait for"
                // (see its own doc comment), same as no DTLS-SRTP at all.
                dtls = None;
            }
            Ok(()) = stop_rx.changed() => {
                if *stop_rx.borrow() { break; }
            }
        }
    }
    debug!("RTP recv task ({what}) stopped");
}

// ── Send task ────────────────────────────────────────────────────────────────

/// Everything a conference's second leg needs on the send side -- `None`
/// for an ordinary single-leg call. The encode itself is always shared
/// (one captured frame, one call to `encoder.encode`); this only bundles
/// leg 2's own independent RTP session (fresh SSRC/sequence/timestamp,
/// possibly a different codec) and its own SRTP encrypt context.
pub(crate) struct SendLeg2State {
    pub(crate) sock2: Arc<RtpSocket>,
    pub(crate) remote2: SocketAddr,
    pub(crate) encoder2: AudioEncoder,
    pub(crate) rtp_send2: RtpSender,
    pub(crate) dtmf_ssrc2: u32,
    pub(crate) dtmf_seq2: u16,
    /// Leg 2's own DTMF telephone-event payload type -- may differ from
    /// leg 1's (`SendDtmfState::dtmf_payload_type`), since the two legs
    /// negotiate independently.
    pub(crate) dtmf_payload_type2: u8,
    pub(crate) encrypt_ctx2: Option<SrtpContext>,
}

/// The mic-processing pipeline: echo cancel -> AGC -> gain -> VAD/comfort-
/// noise gating -- independent of RTP/leg concerns.
pub(crate) struct SendDspState {
    pub(crate) echo_canceller: Option<EchoCanceller>,
    pub(crate) echo_ref: Option<EchoRefBuf>,
    pub(crate) agc: Option<AutomaticGainControl>,
    pub(crate) vad: Option<VoiceActivityDetector>,
    pub(crate) cn_pt: Option<u8>,
    pub(crate) muted: Arc<AtomicBool>,
    pub(crate) input_gain: SharedGain,
    /// `Some((digit, frames_remaining))` while a tone is actively
    /// overriding captured mic audio.
    pub(crate) inband_active: Option<(char, u32)>,
    /// Samples-so-far for the current inband press, so consecutive tone
    /// frames don't click at the frame boundary -- reset to 0 on each new
    /// digit press, not carried across presses.
    pub(crate) inband_phase: u32,
}

/// Out-of-band + inband DTMF plumbing.
pub(crate) struct SendDtmfState {
    pub(crate) dtmf_rx: mpsc::UnboundedReceiver<char>,
    pub(crate) inband_dtmf_rx: mpsc::UnboundedReceiver<char>,
    pub(crate) dtmf_ssrc: u32,
    pub(crate) dtmf_seq: u16,
    pub(crate) dtmf_payload_type: u8,
}

/// The local-playback-mix + recording + stats-counter sink, shared
/// regardless of leg count -- single-leg is just leg1's buffer drained
/// with nothing to mix, unchanged in effect from a direct recv-to-playback
/// push.
pub(crate) struct SendPlaybackState {
    pub(crate) leg1_buf: PlaybackTx,
    pub(crate) leg2_buf: Option<PlaybackTx>,
    pub(crate) hw_playback_tx: PlaybackTx,
    pub(crate) recorder: Arc<Mutex<Option<RecordingWriter>>>,
    pub(crate) stats: SharedStats,
}

/// Everything `send_loop` needs to encode/send leg 1 + (if a conference)
/// leg 2, and mix/record/play back the decoded receive side -- constructed
/// once by `MediaEngine::start` right before spawning the send task.
/// Grouped by concern rather than one flat list of ~25 fields: each group
/// here corresponds to one `tokio::select!` arm or pipeline stage in
/// `send_loop`'s own body.
pub(crate) struct SendTaskState {
    pub(crate) sock: Arc<RtpSocket>,
    pub(crate) remote_rtp: SocketAddr,
    pub(crate) rtp_send: RtpSender,
    pub(crate) encoder: AudioEncoder,
    pub(crate) encrypt_ctx: Option<SrtpContext>,
    pub(crate) leg2: Option<SendLeg2State>,
    pub(crate) dsp: SendDspState,
    pub(crate) dtmf: SendDtmfState,
    pub(crate) playback: SendPlaybackState,
    pub(crate) cap_rx: CaptureRx,
    pub(crate) zrtp_encrypt_rx: mpsc::UnboundedReceiver<([u8; 16], [u8; 14])>,
    pub(crate) dtls_encrypt_rx: mpsc::UnboundedReceiver<([u8; 16], [u8; 14])>,
    pub(crate) stop_rx: watch::Receiver<bool>,
}

/// Drain up to `FRAME_SAMPLES` from a per-leg decode buffer, zero-padding on
/// underrun so mixing stays aligned to the near-end's real-time capture
/// cadence (same idiom as `write_recording`'s near/far pairing).
fn drain_leg(buf: &PlaybackTx) -> Vec<i16> {
    let mut b = buf.lock().unwrap();
    (0..FRAME_SAMPLES).map(|_| b.pop_front().unwrap_or(0)).collect()
}

/// Sum two PCM frames sample-by-sample, halving each first so two
/// simultaneously-loud sources (e.g. a live mic vs. a pre-recorded
/// announcement at a different natural level) can't clip and don't have
/// one leg drown out the other by default, then clamping to `i16` range.
fn mix_frames(a: &[i16], b: &[i16]) -> Vec<i16> {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let mixed = (x as i32) / 2 + (y as i32) / 2;
            mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

/// Write one interleaved stereo frame (left = near-end `pcm`, right = the
/// already-mixed far-end audio for this same frame). No-op if recording
/// isn't enabled for this call.
fn write_recording(recorder: &Mutex<Option<RecordingWriter>>, near: &[i16], far: &[i16]) {
    let mut guard = recorder.lock().unwrap();
    let Some(writer) = guard.as_mut() else { return };
    if let Err(e) = writer.write_frame(near, far) {
        warn!("Failed to write call recording frame: {e}");
    }
}

/// Captured-frame -> (encode leg 1, encode leg 2 if present) -> RTP send
/// loop, plus DTMF (out-of-band and inband), ZRTP/DTLS-SRTP key-switchover,
/// and stop-signal handling. Mirrors `recv_loop`'s shape: a free function
/// taking its whole state as an explicit parameter (here, `SendTaskState`,
/// since ~25 individually-named captures no longer fit a flat argument
/// list) rather than an inline closure -- `MediaEngine::start` constructs
/// `state` once and moves it in via `tokio::spawn(tasks::send_loop(state))`.
pub(crate) async fn send_loop(state: SendTaskState) {
    let SendTaskState {
        sock,
        remote_rtp,
        mut rtp_send,
        mut encoder,
        mut encrypt_ctx,
        mut leg2,
        dsp,
        dtmf,
        playback,
        mut cap_rx,
        mut zrtp_encrypt_rx,
        mut dtls_encrypt_rx,
        mut stop_rx,
    } = state;
    let SendDspState {
        mut echo_canceller,
        echo_ref,
        mut agc,
        mut vad,
        cn_pt,
        muted,
        input_gain,
        mut inband_active,
        mut inband_phase,
    } = dsp;
    let SendDtmfState { mut dtmf_rx, mut inband_dtmf_rx, dtmf_ssrc, mut dtmf_seq, dtmf_payload_type } = dtmf;
    let SendPlaybackState { leg1_buf, leg2_buf, hw_playback_tx, recorder, stats } = playback;

    loop {
        tokio::select! {
            Some(pcm) = cap_rx.recv() => {
                let is_dtmf_tone = inband_active.is_some();
                let pcm = if let Some((digit, remaining)) = inband_active {
                    let tone = dtmf_tone_frame(digit, inband_phase)
                        .expect("inband_active only ever holds a valid DTMF character");
                    inband_phase = inband_phase.wrapping_add(FRAME_SAMPLES as u32);
                    inband_active = if remaining <= 1 { None } else { Some((digit, remaining - 1)) };
                    tone
                } else {
                    let pcm = if muted.load(Ordering::Relaxed) {
                        vec![0i16; pcm.len()]
                    } else {
                        pcm
                    };
                    let pcm = match (echo_canceller.as_mut(), echo_ref.as_ref()) {
                        (Some(canceller), Some(echo_ref)) => canceller.process(&pcm, echo_ref),
                        _ => pcm,
                    };
                    let mut pcm = match agc.as_mut() {
                        Some(agc) => agc.process(&pcm),
                        None => pcm,
                    };
                    // Scale in place (pcm is already owned) rather
                    // than collecting into a fresh Vec.
                    let g = crate::audio::load_gain(&input_gain);
                    if g != 1.0 {
                        for s in pcm.iter_mut() {
                            *s = (*s as f32 * g).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                        }
                    }
                    pcm
                };

                // A DTMF tone is never subject to VAD gating -- it's
                // deliberately generated audio, not silence to detect.
                let vad_decision = if is_dtmf_tone {
                    VadDecision::Talking
                } else {
                    match vad.as_mut() {
                        Some(v) => v.process(&pcm),
                        None => VadDecision::Talking,
                    }
                };

                // Leg 1: encode + send (or send comfort noise, or
                // skip entirely -- see `vad_decision`).
                match vad_decision {
                    VadDecision::Talking => {
                        let encoded = encoder.encode(&pcm);
                        let bytes = rtp_send.next_packet(encoded).encode();
                        if encrypt_and_send(
                            encrypt_ctx.as_mut(), &bytes, &sock, remote_rtp, &stats, Leg::One, "voice",
                        )
                        .await
                        .is_err()
                        {
                            continue;
                        }
                    }
                    VadDecision::SendComfortNoise(level) => {
                        let pt = cn_pt.expect("vad is only Some when cn_pt is Some");
                        let bytes = rtp_send.next_packet_with_pt(pt, vec![level]).encode();
                        if encrypt_and_send(
                            encrypt_ctx.as_mut(), &bytes, &sock, remote_rtp, &stats, Leg::One,
                            "comfort noise",
                        )
                        .await
                        .is_err()
                        {
                            continue;
                        }
                    }
                    VadDecision::Skip => rtp_send.skip_tick(),
                }

                // Leg 2 (conference): encode independently (may differ
                // codec) + send, without aborting leg 1's already-sent
                // packet or the mix/playback step below on failure.
                if let Some(l2) = leg2.as_mut() {
                    let encoded2 = l2.encoder2.encode(&pcm);
                    let bytes2 = l2.rtp_send2.next_packet(encoded2).encode();
                    let _ = encrypt_and_send(
                        l2.encrypt_ctx2.as_mut(), &bytes2, &l2.sock2, l2.remote2, &stats, Leg::Two, "leg2",
                    )
                    .await;
                }

                // Drain + mix decoded audio for local playback/recording
                // -- single-leg case is just leg1's buffer, unchanged
                // in effect from the old direct recv-to-playback push.
                let leg1_frame = drain_leg(&leg1_buf);
                let mixed = if let Some(buf2) = leg2_buf.as_ref() {
                    mix_frames(&leg1_frame, &drain_leg(buf2))
                } else {
                    leg1_frame
                };
                write_recording(&recorder, &pcm, &mixed);
                push_to_jitter(&hw_playback_tx, &mixed);
            }
            Some(ch) = dtmf_rx.recv() => {
                if let Some(event) = char_to_event(ch) {
                    let base_ts = rtp_send.timestamp;
                    let pkts = build_dtmf_burst(
                        event, dtmf_ssrc, &mut dtmf_seq,
                        base_ts, dtmf_payload_type,
                    );
                    for pkt in &pkts {
                        let _ = encrypt_and_send(
                            encrypt_ctx.as_mut(), pkt, &sock, remote_rtp, &stats, Leg::One, "DTMF",
                        )
                        .await;
                    }
                    // Broadcast DTMF to both legs during a conference.
                    if let Some(l2) = leg2.as_mut() {
                        let base_ts2 = l2.rtp_send2.timestamp;
                        let pkts2 = build_dtmf_burst(
                            event, l2.dtmf_ssrc2, &mut l2.dtmf_seq2,
                            base_ts2, l2.dtmf_payload_type2,
                        );
                        for pkt in &pkts2 {
                            let _ = encrypt_and_send(
                                l2.encrypt_ctx2.as_mut(), pkt, &l2.sock2, l2.remote2, &stats, Leg::Two,
                                "DTMF leg2",
                            )
                            .await;
                        }
                    }
                }
            }
            Some(ch) = inband_dtmf_rx.recv() => {
                if char_to_event(ch).is_some() {
                    inband_active = Some((ch, INBAND_FRAME_COUNT));
                    inband_phase = 0;
                }
            }
            Some((key, salt)) = zrtp_encrypt_rx.recv() => {
                match SrtpContext::new(&key, &salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None) {
                    Ok(ctx) => {
                        debug!("ZRTP: switching to SRTP-encrypted send");
                        encrypt_ctx = Some(ctx);
                    }
                    Err(e) => error!("ZRTP: failed to build SRTP encrypt context: {e}"),
                }
            }
            Some((key, salt)) = dtls_encrypt_rx.recv() => {
                match SrtpContext::new(&key, &salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None) {
                    Ok(ctx) => {
                        debug!("DTLS-SRTP: switching to SRTP-encrypted send");
                        encrypt_ctx = Some(ctx);
                    }
                    Err(e) => error!("DTLS-SRTP: failed to build SRTP encrypt context: {e}"),
                }
            }
            Ok(()) = stop_rx.changed() => {
                if *stop_rx.borrow() { break; }
            }
        }
    }
    debug!("RTP send task stopped");
}

#[cfg(test)]
#[path = "../tests/unit/tasks.rs"]
mod tests;
