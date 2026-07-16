use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;
use webrtc_util::Conn;

use deelip_config::RecordingFormat;
use deelip_sip::{AudioCodec, SrtpSession};

use crate::aec::EchoCanceller;
use crate::agc::AutomaticGainControl;
use crate::audio::{AudioStreams, PlaybackTx, open_streams};
use crate::codec_dispatch::{AudioDecoder, AudioEncoder, clock_hz_for, ts_increment_for};
use crate::dtls_demux::DemuxConn;
use crate::dtls_srtp_session::{DtlsSrtpParams, run_dtls_handshake};
use crate::dtmf::DTMF_PAYLOAD_TYPE;
use crate::recording::{RecordingOptions, RecordingWriter};
use crate::rtp::RtpSender;
use crate::stats::{CallStatsSnapshot, LegStats, SharedStats};
use crate::tasks::{
    DtlsRecvState, Leg, SendDspState, SendDtmfState, SendLeg2State, SendPlaybackState, SendTaskState, ZrtpRecvState,
    recv_loop, send_loop,
};
use crate::vad::VoiceActivityDetector;
use crate::zrtp_session::{ZrtpOutcome, ZrtpParams, ZrtpRuntime, client_id as zrtp_client_id};

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
    /// Attempt RFC 5763/5764 DTLS-SRTP key agreement on leg 1's RTP socket
    /// -- `None` for a plain, SDES-SRTP, or ZRTP call (today's behavior for
    /// those). Mutually exclusive with `zrtp` in practice (an account's
    /// `MediaEncryption` selects at most one), and not supported alongside
    /// `second_leg`, same restriction/reasoning as `zrtp`.
    pub dtls_srtp: Option<DtlsSrtpParams>,
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
            dtls_srtp,
        } = opts;

        let (audio_streams, cap_rx, hw_playback_tx, echo_ref, output_gain) =
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
        let encrypt_ctx: Option<SrtpContext> = srtp
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
        let (zrtp_encrypt_tx, zrtp_encrypt_rx) = mpsc::unbounded_channel::<([u8; 16], [u8; 14])>();
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

        // Must be created before the DTLS-SRTP block below, which needs a
        // `stop_tx` clone (see `handle_dtls_outcome`).
        let (stop_tx, stop_rx) = watch::channel(false);

        // ── DTLS-SRTP (leg 1 only -- see `start`'s doc comment) ──────────────────
        // Spawned as a background task, run concurrently with `recv_loop` --
        // see docs/crates/media-engine.md's "DTLS-SRTP session driving"
        // section for why this can't run synchronously first the way ZRTP's
        // initial Hello does. `dtls_encrypt_tx`/`rx` are always constructed
        // (mirroring `zrtp_encrypt_tx`/`rx` above) so the send task's
        // `tokio::select!` arm below type-checks unconditionally.
        let (dtls_encrypt_tx, dtls_encrypt_rx) = mpsc::unbounded_channel::<([u8; 16], [u8; 14])>();
        let dtls_recv_state = dtls_srtp.map(|params| {
            let (demux, inbound_tx) = DemuxConn::new(socket.clone(), remote_rtp);
            let (outcome_tx, outcome_rx) = mpsc::unbounded_channel();
            tokio::spawn(run_dtls_handshake(params, demux, outcome_tx));
            DtlsRecvState { inbound_tx, outcome_rx, encrypt_tx: dtls_encrypt_tx, stop_tx: stop_tx.clone() }
        });

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

        let stop_send = stop_rx.clone();
        let stop_recv = stop_rx;

        let (dtmf_tx, dtmf_rx) = mpsc::unbounded_channel::<char>();
        let (inband_dtmf_tx, inband_dtmf_rx) = mpsc::unbounded_channel::<char>();

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
        let rtp_send = RtpSender::new(payload_type, ts_increment_for(codec));
        let dtmf_ssrc = rtp_send.ssrc;
        let encoder = AudioEncoder::new(codec, "")?;

        let leg2_send = match (&leg2_socket, leg2_remote, leg2_codec) {
            (Some(sock2), Some(remote2), Some(c2)) => {
                let rtp_send2 = RtpSender::new(c2.payload_type(), ts_increment_for(c2));
                Some(SendLeg2State {
                    sock2: sock2.clone(),
                    remote2,
                    encoder2: AudioEncoder::new(c2, " (leg2)")?,
                    dtmf_ssrc2: rtp_send2.ssrc,
                    rtp_send2,
                    dtmf_seq2: 0,
                    dtmf_payload_type2: leg2_dtmf_pt.unwrap_or(DTMF_PAYLOAD_TYPE),
                    encrypt_ctx2: leg2_encrypt_ctx,
                })
            }
            _ => None,
        };

        let echo_canceller = echo_ref.as_ref().map(|_| EchoCanceller::new());
        let agc = agc_enabled.then(AutomaticGainControl::new);
        let vad = cn_pt.map(|_| VoiceActivityDetector::new());
        let muted = Arc::new(AtomicBool::new(false));

        let send_task = tokio::spawn(send_loop(SendTaskState {
            sock: socket.clone(),
            remote_rtp,
            rtp_send,
            encoder,
            encrypt_ctx,
            leg2: leg2_send,
            dsp: SendDspState {
                echo_canceller,
                echo_ref,
                agc,
                vad,
                cn_pt,
                muted: muted.clone(),
                input_gain: input_gain.clone(),
                inband_active: None,
                inband_phase: 0,
            },
            dtmf: SendDtmfState { dtmf_rx, inband_dtmf_rx, dtmf_ssrc, dtmf_seq: 0, dtmf_payload_type },
            playback: SendPlaybackState {
                leg1_buf: leg1_buf.clone(),
                leg2_buf: leg2_buf.clone(),
                hw_playback_tx,
                recorder: recorder.clone(),
                stats: stats.clone(),
            },
            cap_rx,
            zrtp_encrypt_rx,
            dtls_encrypt_rx,
            stop_rx: stop_send,
        }));

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
            dtls_recv_state,
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
                None, // leg 2 is never DTLS-SRTP either -- same restriction/reasoning
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
    /// engine was started with. See docs/crates/media-engine.md's "Manual
    /// record toggle" for the on/off semantics.
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
        // Dispatched onto spawn_blocking (finalize() is blocking disk I/O,
        // and stop() itself is commonly awaited via rt.block_on directly on
        // the UI/render thread) -- see this function's own doc comment /
        // docs/crates/media-engine.md for why.
        if let Some(writer) = self.recorder.lock().unwrap().take() {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = writer.finalize() {
                    error!("Failed to finalize call recording: {e}");
                }
            });
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/engine.rs"]
mod tests;
