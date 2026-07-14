use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;
use webrtc_util::Conn;

use deelip_config::RecordingFormat;
use deelip_sip::zrtp::{Role, is_zrtp_packet};
use deelip_sip::{AudioCodec, SrtpSession};

use crate::aec::EchoCanceller;
use crate::agc::AutomaticGainControl;
use crate::audio::{AudioStreams, FRAME_SAMPLES, PlaybackTx, open_streams};
use crate::codec_dispatch::{AudioDecoder, AudioEncoder, clock_hz_for, ts_increment_for};
use crate::dtmf::{DTMF_PAYLOAD_TYPE, INBAND_FRAME_COUNT, build_dtmf_burst, char_to_event, dtmf_tone_frame};
use crate::recording::{RecordingOptions, RecordingWriter};
use crate::rtp::{RtpPacket, RtpSender};
use crate::stats::{CallStatsSnapshot, JitterTracker, LegStats, SharedStats};
use crate::vad::{ComfortNoiseState, VadDecision, VoiceActivityDetector, synthesize_comfort_noise};
use crate::zrtp_session::{ZrtpOutcome, ZrtpParams, ZrtpRuntime, client_id as zrtp_client_id};

/// Which leg a send/stats update belongs to -- `Two` is a no-op on the
/// stats side when `CallStatsSnapshot.leg2` is `None` (single-call, not a
/// conference), matching every call site's previous individual handling.
enum Leg {
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
async fn encrypt_and_send(
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
async fn handle_zrtp_outcomes(
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
struct ZrtpRecvState {
    runtime: ZrtpRuntime,
    encrypt_tx: mpsc::UnboundedSender<([u8; 16], [u8; 14])>,
    sas: Arc<Mutex<Option<String>>>,
}

/// Shared receive-loop body for both legs' RTP recv tasks: recv -> (ZRTP
/// packet? hand to `zrtp`, if any, and loop) -> SRTP-decrypt (if
/// `decrypt_ctx` is `Some`) -> parse -> drop DTMF payloads -> stats/jitter
/// -> decode (voice, or synthesize comfort noise if `cn_pt` matches) ->
/// push to `playback`. Mirrors `video_engine.rs`'s own `recv_loop`, which
/// already shares one function between both legs the same way -- this was
/// previously two hand-duplicated ~35-75 line async blocks, one per leg.
#[allow(clippy::too_many_arguments)]
async fn recv_loop(
    sock: Arc<RtpSocket>, mut decrypt_ctx: Option<SrtpContext>, mut decoder: AudioDecoder, dtmf_pt: u8,
    cn_pt: Option<u8>, clock_hz: f64, playback: PlaybackTx, stats: SharedStats, leg: Leg,
    mut zrtp: Option<ZrtpRecvState>, remote_rtp: SocketAddr, mut stop_rx: watch::Receiver<bool>, what: &'static str,
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
            Ok(()) = stop_rx.changed() => {
                if *stop_rx.borrow() { break; }
            }
        }
    }
    debug!("RTP recv task ({what}) stopped");
}

/// The RTP wire transport: a plain local UDP socket, or a TURN-relayed `Conn`
/// (see `deelip_nat::allocate_relay`). Both shapes speak send_to/recv_from, so
/// everything downstream (codec dispatch, SRTP, DTMF, jitter buffer) is
/// identical regardless of which one is active.
pub(crate) enum RtpSocket {
    Direct(UdpSocket),
    Relay(Arc<dyn Conn + Send + Sync>),
}

impl RtpSocket {
    pub(crate) async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> anyhow::Result<()> {
        match self {
            Self::Direct(s) => {
                s.send_to(buf, addr).await?;
            }
            Self::Relay(c) => {
                c.send_to(buf, addr).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn recv_from(&self, buf: &mut [u8]) -> anyhow::Result<(usize, SocketAddr)> {
        match self {
            Self::Direct(s) => Ok(s.recv_from(buf).await?),
            Self::Relay(c) => Ok(c.recv_from(buf).await?),
        }
    }
}

/// A second RTP leg for 3-way conferencing — bundles everything a leg needs
/// beyond what `MediaEngine::start`'s existing params already cover for
/// leg 1. Reuses the leg's own already-allocated RTP port/relay (nothing
/// new to allocate); the two legs may have negotiated different codecs.
pub struct ConferenceLeg {
    pub local_rtp_port: u16,
    pub remote_rtp: SocketAddr,
    pub codec: AudioCodec,
    pub dtmf_pt: Option<u8>,
    pub srtp: Option<SrtpSession>,
    pub relay: Option<Arc<dyn Conn + Send + Sync>>,
}

// ── MediaEngine ───────────────────────────────────────────────────────────────

/// Manages the audio ↔ RTP pipeline for a single active call, optionally
/// bridging a second RTP leg for a 3-way conference (see `ConferenceLeg`).
pub struct MediaEngine {
    _audio: AudioStreams,
    send_task: tokio::task::JoinHandle<()>,
    recv_task: tokio::task::JoinHandle<()>,
    /// Only `Some` when started with a `second_leg` (conference mode).
    recv_task2: Option<tokio::task::JoinHandle<()>>,
    stop_tx: watch::Sender<bool>,
    dtmf_tx: mpsc::UnboundedSender<char>,
    inband_dtmf_tx: mpsc::UnboundedSender<char>,
    muted: Arc<AtomicBool>,
    /// Owned by `MediaEngine` itself, not just the send task's closure, so
    /// `stop()` can finalize it deterministically -- see `docs/crates/media-engine.md`.
    recorder: Arc<Mutex<Option<RecordingWriter>>>,
    /// Kept so `set_recording(true)` can lazily open a `RecordingWriter`
    /// on demand (manual per-call Record button) using the same
    /// name/format/directory this call would have used had
    /// `RecordingOptions::enabled` been true from the start.
    call_id: String,
    recording_format: RecordingFormat,
    recordings_dir_override: Option<String>,
    /// Live in-call speaker/mic level controls -- see `crate::audio::SharedGain`.
    output_gain: crate::audio::SharedGain,
    input_gain: crate::audio::SharedGain,
    stats: SharedStats,
    /// Set once the ZRTP handshake (if `ZrtpParams` was given to `start`)
    /// completes -- `None` throughout an unencrypted or SDES-SRTP call, or
    /// until the handshake finishes.
    zrtp_sas: Arc<Mutex<Option<String>>>,
}

/// Parameters for `MediaEngine::start` -- a named-field struct instead of a
/// 15-argument positional list, so a call site reads its own intent and a
/// newly-added field can't silently shift the meaning of the ones after it.
pub struct MediaEngineOptions<'a> {
    /// Local UDP port for RTP.
    pub local_rtp_port: u16,
    /// Remote RTP endpoint.
    pub remote_rtp: SocketAddr,
    /// Negotiated voice codec (PCMU/PCMA/Opus/...).
    pub codec: AudioCodec,
    /// DTMF telephone-event payload type (typically `Some(101)`).
    pub dtmf_pt: Option<u8>,
    /// RFC 3389 comfort-noise payload type the remote signaled, if any --
    /// enables VAD-gated silence suppression on leg 1 only (a conference's
    /// second leg always sends continuously, see `crate::vad`'s module doc
    /// for the inter-SID-gap simplification this makes).
    pub cn_pt: Option<u8>,
    /// Local/remote SDES-SRTP keys, if the call negotiated encrypted media.
    pub srtp: Option<SrtpSession>,
    /// A TURN-allocated relay `Conn` (see `deelip_nat::allocate_relay`), if
    /// the call is relaying media instead of using a direct local socket.
    pub relay: Option<Arc<dyn Conn + Send + Sync>>,
    /// Run acoustic echo cancellation on the mic path (see `crate::aec`) --
    /// only useful on speakers/mic, not headsets.
    pub echo_cancellation: bool,
    /// Run adaptive microphone gain control on the mic path (see `crate::agc`).
    pub agc_enabled: bool,
    /// Specific cpal input device name, falling back to the system default
    /// if unset or not found.
    pub input_device: Option<&'a str>,
    /// Specific cpal output device name, falling back to the system default
    /// if unset or not found.
    pub output_device: Option<&'a str>,
    /// Whether/how/where to record this call to a stereo file (left =
    /// near-end mic, right = far-end/mixed received audio) -- see
    /// `RecordingOptions`.
    pub recording: RecordingOptions,
    /// Used to name the recording file; ignored if recording is disabled.
    pub call_id: &'a str,
    /// `Some` to bridge a second remote party into a 3-way conference,
    /// sharing this same mic/speaker pair -- `None` (every existing call
    /// site) is the ordinary single-leg call, unchanged.
    pub second_leg: Option<ConferenceLeg>,
    /// Attempt RFC 6189 ZRTP key agreement on leg 1's RTP socket instead of
    /// (or alongside a no-op) `srtp` -- `None` for a plain or SDES-SRTP call
    /// (today's behavior). Not supported alongside `second_leg` (conference
    /// legs stay SDES/plain-only).
    pub zrtp: Option<ZrtpParams>,
}

impl MediaEngine {
    /// Start the media engine -- see `MediaEngineOptions`'s own field docs
    /// for what each option controls.
    pub async fn start(opts: MediaEngineOptions<'_>) -> anyhow::Result<Self> {
        let MediaEngineOptions {
            local_rtp_port,
            remote_rtp,
            codec,
            dtmf_pt,
            cn_pt,
            srtp,
            relay,
            echo_cancellation,
            agc_enabled,
            input_device,
            output_device,
            recording,
            call_id,
            second_leg,
            zrtp,
        } = opts;

        let (audio_streams, mut cap_rx, hw_playback_tx, echo_ref, output_gain) =
            open_streams(input_device, output_device, echo_cancellation).context("Opening audio streams")?;
        let input_gain = crate::audio::new_shared_gain();

        let stats: SharedStats = Arc::new(Mutex::new(CallStatsSnapshot {
            leg1: LegStats::default(),
            leg2: second_leg.as_ref().map(|_| LegStats::default()),
        }));

        // ── Leg 1 ─────────────────────────────────────────────────────────────
        let socket = Arc::new(match relay {
            Some(conn) => RtpSocket::Relay(conn),
            None => RtpSocket::Direct(
                UdpSocket::bind(format!("0.0.0.0:{local_rtp_port}"))
                    .await
                    .with_context(|| format!("Binding RTP on :{local_rtp_port}"))?,
            ),
        });

        let dtmf_payload_type = dtmf_pt.unwrap_or(DTMF_PAYLOAD_TYPE);
        let payload_type = codec.payload_type();
        // Defense-in-depth against `CN_PAYLOAD_TYPE`'s clock-mismatch hazard
        // (see its doc comment in sip-core) even if negotiation somehow
        // still signaled CN alongside Opus.
        let cn_pt = cn_pt.filter(|_| codec != AudioCodec::Opus);

        // Per RFC 4568, each side's a=crypto line declares the key THAT SIDE uses to
        // encrypt what it sends; the peer decrypts with that same key. So we encrypt
        // outgoing traffic with our OWN declared (local) key, and decrypt incoming
        // traffic with the REMOTE's declared key.
        let mut encrypt_ctx: Option<SrtpContext> = srtp
            .as_ref()
            .map(|s| SrtpContext::new(&s.local.key, &s.local.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None))
            .transpose()
            .context("Creating SRTP encrypt context")?;
        let decrypt_ctx: Option<SrtpContext> = srtp
            .as_ref()
            .map(|s| {
                SrtpContext::new(
                    &s.remote.key,
                    &s.remote.salt,
                    ProtectionProfile::Aes128CmHmacSha1_80,
                    Some(srtp_replay_protection(64)),
                    None,
                )
            })
            .transpose()
            .context("Creating SRTP decrypt context")?;

        // ── ZRTP (leg 1 only -- see `start`'s doc comment) ───────────────────────
        let zrtp_sas: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (zrtp_encrypt_tx, mut zrtp_encrypt_rx) = mpsc::unbounded_channel::<([u8; 16], [u8; 14])>();
        let mut zrtp_runtime: Option<ZrtpRuntime> = None;
        let mut zrtp_pending_sends: Vec<Vec<u8>> = Vec::new();
        if let Some(params) = zrtp {
            match ZrtpRuntime::new(
                params.role,
                params.local_zid,
                zrtp_client_id(),
                &deelip_config::default_db_path().context("Resolving DB path for ZRTP cache")?,
            ) {
                Ok((runtime, initial_outcomes)) => {
                    zrtp_runtime = Some(runtime);
                    for outcome in initial_outcomes {
                        if let ZrtpOutcome::SendBytes(bytes) = outcome {
                            zrtp_pending_sends.push(bytes);
                        }
                    }
                }
                Err(e) => error!("Failed to start ZRTP session: {e:#}"),
            }
        }
        // Send ZRTP's initial Hello now, before the send/recv tasks exist --
        // reuses the same socket they'll each get their own clone of.
        for bytes in zrtp_pending_sends {
            if let Err(e) = socket.send_to(&bytes, remote_rtp).await {
                warn!("Failed to send initial ZRTP packet: {e:#}");
            }
        }

        // ── Leg 2 (conference), if present ────────────────────────────────────
        let mut leg2_socket: Option<Arc<RtpSocket>> = None;
        let mut leg2_remote: Option<SocketAddr> = None;
        let mut leg2_codec: Option<AudioCodec> = None;
        let mut leg2_dtmf_pt: Option<u8> = None;
        let mut leg2_encrypt_ctx: Option<SrtpContext> = None;
        let mut leg2_decrypt_ctx: Option<SrtpContext> = None;
        if let Some(leg) = &second_leg {
            let socket2 = Arc::new(match &leg.relay {
                Some(conn) => RtpSocket::Relay(conn.clone()),
                None => RtpSocket::Direct(
                    UdpSocket::bind(format!("0.0.0.0:{}", leg.local_rtp_port))
                        .await
                        .with_context(|| format!("Binding RTP on :{}", leg.local_rtp_port))?,
                ),
            });
            leg2_encrypt_ctx = leg
                .srtp
                .as_ref()
                .map(|s| {
                    SrtpContext::new(&s.local.key, &s.local.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None)
                })
                .transpose()
                .context("Creating leg2 SRTP encrypt context")?;
            leg2_decrypt_ctx = leg
                .srtp
                .as_ref()
                .map(|s| {
                    SrtpContext::new(
                        &s.remote.key,
                        &s.remote.salt,
                        ProtectionProfile::Aes128CmHmacSha1_80,
                        Some(srtp_replay_protection(64)),
                        None,
                    )
                })
                .transpose()
                .context("Creating leg2 SRTP decrypt context")?;
            leg2_socket = Some(socket2);
            leg2_remote = Some(leg.remote_rtp);
            leg2_codec = Some(leg.codec);
            leg2_dtmf_pt = Some(leg.dtmf_pt.unwrap_or(DTMF_PAYLOAD_TYPE));
            debug!("Conference mode: leg1={remote_rtp} ({codec:?}), leg2={} ({:?})", leg.remote_rtp, leg.codec);
        }

        // Per-leg decode buffers, mixed together (if leg2 present) once per
        // captured frame by the send task before reaching the speaker/recording
        // -- see `drain_leg`/`mix_frames` below. Single-leg case is just one
        // buffer drained with nothing to mix, functionally identical to the
        // old direct recv-task-to-playback push.
        let leg1_buf: PlaybackTx = Arc::new(Mutex::new(VecDeque::new()));
        let leg2_buf: Option<PlaybackTx> = second_leg.as_ref().map(|_| Arc::new(Mutex::new(VecDeque::new())));

        let (stop_tx, stop_rx) = watch::channel(false);
        let mut stop_send = stop_rx.clone();
        let stop_recv = stop_rx;

        let (dtmf_tx, mut dtmf_rx) = mpsc::unbounded_channel::<char>();
        let (inband_dtmf_tx, mut inband_dtmf_rx) = mpsc::unbounded_channel::<char>();

        let recorder: Arc<Mutex<Option<RecordingWriter>>> = Arc::new(Mutex::new(if recording.enabled {
            match RecordingWriter::create(call_id, recording.dir_override.as_deref(), recording.format) {
                Ok(w) => Some(w),
                Err(e) => {
                    error!("Failed to start call recording: {e}");
                    None
                }
            }
        } else {
            None
        }));

        // ── Send task ─────────────────────────────────────────────────────────
        let send_sock = socket.clone();
        let send_sock2 = leg2_socket.clone();
        let mut rtp_send = RtpSender::new(payload_type, ts_increment_for(codec));
        let dtmf_ssrc = rtp_send.ssrc;
        let mut dtmf_seq = 0u16;
        let mut encoder = AudioEncoder::new(codec, "")?;
        let mut encoder2 = leg2_codec.map(|c| AudioEncoder::new(c, " (leg2)")).transpose()?;
        let mut rtp_send2 = leg2_codec.map(|c| RtpSender::new(c.payload_type(), ts_increment_for(c)));
        let dtmf_ssrc2 = rtp_send2.as_ref().map(|r| r.ssrc);
        let mut dtmf_seq2 = 0u16;

        let mut echo_canceller = echo_ref.as_ref().map(|_| EchoCanceller::new());
        let mut agc = agc_enabled.then(AutomaticGainControl::new);
        let mut vad = cn_pt.map(|_| VoiceActivityDetector::new());
        let muted = Arc::new(AtomicBool::new(false));
        let send_muted = muted.clone();
        let send_input_gain = input_gain.clone();
        let send_recorder = recorder.clone();
        let send_leg1_buf = leg1_buf.clone();
        let send_leg2_buf = leg2_buf.clone();
        let send_stats = stats.clone();

        // Inband-DTMF state: `Some((digit, frames_remaining))` while a tone
        // is actively overriding captured mic audio; `inband_phase` tracks
        // samples-so-far for this press so consecutive tone frames don't
        // click at the frame boundary (reset to 0 on each new digit press,
        // not carried across presses -- keeps it small/safe over a long call).
        let mut inband_active: Option<(char, u32)> = None;
        let mut inband_phase: u32 = 0;

        let send_task = tokio::spawn(async move {
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
                            let pcm = if send_muted.load(Ordering::Relaxed) {
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
                            let g = crate::audio::load_gain(&send_input_gain);
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
                                    encrypt_ctx.as_mut(), &bytes, &send_sock, remote_rtp, &send_stats, Leg::One, "voice",
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
                                    encrypt_ctx.as_mut(), &bytes, &send_sock, remote_rtp, &send_stats, Leg::One,
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
                        if let (Some(sock2), Some(remote2), Some(enc2), Some(sender2)) =
                            (send_sock2.as_ref(), leg2_remote, encoder2.as_mut(), rtp_send2.as_mut())
                        {
                            let encoded2 = enc2.encode(&pcm);
                            let bytes2 = sender2.next_packet(encoded2).encode();
                            let _ = encrypt_and_send(
                                leg2_encrypt_ctx.as_mut(), &bytes2, sock2, remote2, &send_stats, Leg::Two, "leg2",
                            )
                            .await;
                        }

                        // Drain + mix decoded audio for local playback/recording
                        // -- single-leg case is just leg1's buffer, unchanged
                        // in effect from the old direct recv-to-playback push.
                        let leg1_frame = drain_leg(&send_leg1_buf);
                        let mixed = if let Some(buf2) = send_leg2_buf.as_ref() {
                            mix_frames(&leg1_frame, &drain_leg(buf2))
                        } else {
                            leg1_frame
                        };
                        write_recording(&send_recorder, &pcm, &mixed);
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
                                    encrypt_ctx.as_mut(), pkt, &send_sock, remote_rtp, &send_stats, Leg::One, "DTMF",
                                )
                                .await;
                            }
                            // Broadcast DTMF to both legs during a conference.
                            if let (Some(sock2), Some(remote2), Some(ssrc2), Some(pt2), Some(sender2)) =
                                (send_sock2.as_ref(), leg2_remote, dtmf_ssrc2, leg2_dtmf_pt, rtp_send2.as_ref())
                            {
                                let base_ts2 = sender2.timestamp;
                                let pkts2 = build_dtmf_burst(
                                    event, ssrc2, &mut dtmf_seq2,
                                    base_ts2, pt2,
                                );
                                for pkt in &pkts2 {
                                    let _ = encrypt_and_send(
                                        leg2_encrypt_ctx.as_mut(), pkt, sock2, remote2, &send_stats, Leg::Two,
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
                    Ok(()) = stop_send.changed() => {
                        if *stop_send.borrow() { break; }
                    }
                }
            }
            debug!("RTP send task stopped");
        });

        // ── Recv task (leg 1) ─────────────────────────────────────────────────
        let decoder = AudioDecoder::new(codec, "")?;
        let recv_clock_hz = clock_hz_for(codec);
        let zrtp_recv_state = zrtp_runtime.take().map(|runtime| ZrtpRecvState {
            runtime,
            encrypt_tx: zrtp_encrypt_tx,
            sas: zrtp_sas.clone(),
        });

        let recv_task = tokio::spawn(recv_loop(
            socket,
            decrypt_ctx,
            decoder,
            dtmf_payload_type,
            cn_pt,
            recv_clock_hz,
            leg1_buf.clone(),
            stats.clone(),
            Leg::One,
            zrtp_recv_state,
            remote_rtp,
            stop_recv,
            "leg1",
        ));

        // ── Recv task (leg 2, conference only) ────────────────────────────────
        let recv_task2 = if let Some(leg) = &second_leg {
            let recv_sock2 = leg2_socket.clone().unwrap();
            let decoder2 = AudioDecoder::new(leg.codec, " (leg2)")?;
            let pt2 = leg2_dtmf_pt.unwrap();
            let leg2_buf_recv = leg2_buf.clone().unwrap();
            let recv_clock_hz2 = clock_hz_for(leg.codec);

            Some(tokio::spawn(recv_loop(
                recv_sock2,
                leg2_decrypt_ctx,
                decoder2,
                pt2,
                None, // leg 2 never does comfort noise -- unchanged from before this refactor
                recv_clock_hz2,
                leg2_buf_recv,
                stats.clone(),
                Leg::Two,
                None, // leg 2 is never ZRTP -- unchanged from before this refactor
                leg.remote_rtp,
                stop_tx.subscribe(),
                "leg2",
            )))
        } else {
            None
        };

        Ok(Self {
            _audio: audio_streams,
            send_task,
            recv_task,
            recv_task2,
            stop_tx,
            dtmf_tx,
            inband_dtmf_tx,
            muted,
            recorder,
            call_id: call_id.to_string(),
            recording_format: recording.format,
            recordings_dir_override: recording.dir_override.clone(),
            output_gain,
            input_gain,
            stats,
            zrtp_sas,
        })
    }

    /// Live-verification string ("SAS") for this call's ZRTP-derived key
    /// agreement -- read out over the phone to each side's user to confirm
    /// neither is mid-conversation with a MITM; `None` until the handshake
    /// completes, or for the whole call if ZRTP wasn't attempted.
    pub fn zrtp_sas(&self) -> Option<String> {
        self.zrtp_sas.lock().unwrap().clone()
    }

    /// Queue a DTMF digit for immediate out-of-band RTP transmission.
    pub fn send_dtmf(&self, digit: char) {
        let _ = self.dtmf_tx.send(digit);
    }

    /// Queue a DTMF digit to be sent as real inband dual-tone audio, mixed
    /// into the outgoing RTP stream in place of captured mic audio for the
    /// next `INBAND_FRAME_COUNT` frames.
    pub fn send_dtmf_inband(&self, digit: char) {
        let _ = self.inband_dtmf_tx.send(digit);
    }

    /// Snapshot of this call's current RTP stats (packets/bytes sent and
    /// received, best-effort loss count, jitter estimate) — cheap to call
    /// every UI frame, since it's just a mutex-guarded struct clone.
    pub fn stats(&self) -> CallStatsSnapshot {
        self.stats.lock().unwrap().clone()
    }

    /// Mute/unmute the local microphone — captured audio is replaced with
    /// silence before encoding (RTP keeps flowing, so the remote side and
    /// any NAT bindings aren't affected, only the audio content is).
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Start/stop recording this call on demand (manual per-call Record
    /// button) -- independent of whatever `RecordingOptions::enabled` this
    /// engine was started with. Turning on lazily opens a fresh
    /// `RecordingWriter` (so a manually-started recording only captures
    /// audio from this point forward, not the part of the call already
    /// missed); turning off finalizes and drops it immediately rather than
    /// waiting for `stop()`. A failure to open the file is logged and
    /// leaves recording off, same as the auto-record path at `start()`.
    pub fn set_recording(&self, on: bool) {
        let mut guard = self.recorder.lock().unwrap();
        if on {
            if guard.is_some() {
                return;
            }
            match RecordingWriter::create(&self.call_id, self.recordings_dir_override.as_deref(), self.recording_format)
            {
                Ok(w) => *guard = Some(w),
                Err(e) => error!("Failed to start call recording: {e}"),
            }
        } else if let Some(writer) = guard.take()
            && let Err(e) = writer.finalize()
        {
            error!("Failed to finalize call recording: {e}");
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.lock().unwrap().is_some()
    }

    /// Live in-call speaker volume -- `1.0` is unchanged/unity gain.
    pub fn set_output_gain(&self, gain: f32) {
        crate::audio::store_gain(&self.output_gain, gain);
    }
    pub fn output_gain(&self) -> f32 {
        crate::audio::load_gain(&self.output_gain)
    }

    /// Live in-call microphone gain, applied after AGC (if enabled) as a
    /// final user-adjustable trim -- `1.0` is unchanged/unity gain.
    pub fn set_input_gain(&self, gain: f32) {
        crate::audio::store_gain(&self.input_gain, gain);
    }
    pub fn input_gain(&self) -> f32 {
        crate::audio::load_gain(&self.input_gain)
    }

    /// Async so callers can wait for the send/recv tasks to actually finish,
    /// not just be scheduled for cancellation -- a real relay-`Conn`-reuse
    /// race (conference merge) depends on this; see `docs/crates/media-engine.md`
    /// before changing this to a fire-and-forget shape.
    pub async fn stop(self) {
        let _ = self.stop_tx.send(true);
        self.send_task.abort();
        self.recv_task.abort();
        if let Some(t2) = &self.recv_task2 {
            t2.abort();
        }
        let _ = self.send_task.await;
        let _ = self.recv_task.await;
        if let Some(t2) = self.recv_task2 {
            let _ = t2.await;
        }
        // Take the writer here (not inside the send task) so finalization
        // happens deterministically regardless of the abort race above --
        // both WAV and MP3 need an explicit finalize() (RIFF header sizes /
        // final encoder flush); Drop alone would leave either format
        // malformed/truncated. The actual finalize() call is blocking disk
        // I/O, though, and `stop()` itself is commonly awaited via
        // `rt.block_on` directly on the UI/render thread (hangup/hold/swap)
        // -- so it's dispatched onto `spawn_blocking` rather than run
        // inline here, keeping this UI-thread-visible `stop()` fast even on
        // a slow or antivirus-intercepted disk. Recording is already
        // best-effort (see `AppConfig::recording_enabled`'s doc comment),
        // so a finalize that completes a moment after `stop()` itself
        // returns is an acceptable tradeoff -- unlike the task-await above,
        // which this function's own doc comment requires stay synchronous.
        if let Some(writer) = self.recorder.lock().unwrap().take() {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = writer.finalize() {
                    error!("Failed to finalize call recording: {e}");
                }
            });
        }
    }
}

fn push_to_jitter(jitter: &PlaybackTx, pcm: &[i16]) {
    let max = FRAME_SAMPLES * 50; // cap at 1 second
    let mut buf = jitter.lock().unwrap();
    for &s in pcm {
        if buf.len() < max {
            buf.push_back(s);
        }
    }
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

#[cfg(test)]
#[path = "../tests/unit/engine.rs"]
mod tests;
