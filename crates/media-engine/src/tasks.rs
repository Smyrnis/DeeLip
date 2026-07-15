//! Background send/recv RTP tasks for `engine::MediaEngine` -- split out of
//! `engine.rs` purely for file size (same precedent as `views/settings/`,
//! `views/dialer/`, `sip-core/src/call/lifecycle/`), not a behavior change.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;

use deelip_sip::zrtp::{Role, is_zrtp_packet};

use crate::audio::{FRAME_SAMPLES, PlaybackTx};
use crate::codec_dispatch::AudioDecoder;
use crate::dtls_demux::is_dtls_packet;
use crate::dtls_srtp_session::DtlsSrtpOutcome;
use crate::dtmf::DTMF_PAYLOAD_TYPE;
use crate::engine::RtpSocket;
use crate::rtp::RtpPacket;
use crate::stats::{JitterTracker, SharedStats};
use crate::vad::{ComfortNoiseState, synthesize_comfort_noise};
use crate::zrtp_session::{ZrtpOutcome, ZrtpRuntime};

/// Which leg a send/stats update belongs to -- `Two` is a no-op on the
/// stats side when `CallStatsSnapshot.leg2` is `None` (single-call, not a
/// conference), matching every call site's previous individual handling.
pub(crate) enum Leg {
    One,
    Two,
}

/// Encrypts `bytes` via `ctx` (if `Some` -- SRTP negotiated) and sends the
/// result to `remote` over `sock`, updating `stats`' leg1/leg2
/// packet/byte counters on a successful send. Returns `Err(())` if either
/// encryption or the send itself failed (already logged via
/// `tracing::error!`, tagged with `what`) -- callers decide what "skip this
/// frame/packet/leg" means for their own loop; this only collapses the
/// encrypt-log-send-count sequence that was previously copy-pasted 5 times
/// across the send task below. `ctx.encrypt_rtp` already returns a cheap
/// refcounted `bytes::Bytes` (not owned `Vec<u8>`) -- borrowed here via
/// `out`, not `.to_vec()`'d, so this is also one fewer allocation+copy per
/// encrypted packet than the code it replaces. Note for the DTMF call
/// sites specifically: unlike the code being replaced, DTMF packets now
/// also update `stats` on a successful send (previously only voice/comfort-
/// noise packets did) -- a deliberate, minor side effect of sharing one
/// path, not an oversight: DTMF is real data on the wire and counting it
/// makes the leg stats more accurate, not less.
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
/// leg 2, same restriction/reasoning as `ZrtpRecvState`. Unlike ZRTP (an
/// ongoing in-band exchange `recv_loop` itself drives via
/// `handle_incoming`/`tick`), the handshake runs on its own background task
/// (`run_dtls_handshake`, spawned by `MediaEngine::start`) and reports back
/// exactly once via `outcome_rx` -- see `recv_dtls_outcome`.
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

/// Acts on `run_dtls_handshake`'s one-shot outcome. `FingerprintMismatch` is
/// deliberately handled differently from ZRTP's `Failed` precedent (which
/// just logs and continues unencrypted): a mismatch means the peer's actual
/// certificate didn't match what SDP advertised -- an active-attack
/// indicator, not an ordinary negotiation failure -- so this actively tears
/// down the whole call's media via `stop_tx` rather than ever letting
/// plaintext RTP flow. An ordinary `Failed` (e.g. a network hiccup during
/// the handshake) falls back to unencrypted media exactly like ZRTP does.
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
/// already shares one function between both legs the same way -- this was
/// previously two hand-duplicated ~35-75 line async blocks, one per leg.
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
