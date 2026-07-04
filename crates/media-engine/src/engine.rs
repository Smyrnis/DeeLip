use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;
use webrtc_util::Conn;

use deelip_config::recordings_dir;
use deelip_sip::{AudioCodec, SrtpSession};

use crate::aec::EchoCanceller;
use crate::audio::{open_streams, AudioStreams, PlaybackTx, FRAME_SAMPLES, SAMPLE_RATE};
use crate::codec::{
    decode_pcma, decode_pcmu, encode_pcma, encode_pcmu, G722Decoder, G722Encoder, OpusDecoder,
    OpusEncoder,
};
use crate::dtmf::{build_dtmf_burst, char_to_event, DTMF_PAYLOAD_TYPE};
use crate::rtp::{RtpPacket, RtpSender};

type WavWriter = hound::WavWriter<BufWriter<File>>;

/// Replace anything outside `[A-Za-z0-9._-]` with `_` (SIP Call-IDs can
/// contain `@` and other characters not safe verbatim in a filename).
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

/// Open a stereo (left=near-end mic, right=far-end received/mixed) WAV
/// writer for this call, under `recordings_dir()`.
fn open_recorder(call_id: &str) -> anyhow::Result<WavWriter> {
    let dir = recordings_dir().context("Resolving recordings dir")?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{timestamp}_{}.wav", sanitize_filename(call_id)));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::create(&path, spec)
        .with_context(|| format!("Creating recording at {}", path.display()))
}

/// Per-packet RTP timestamp increment for a 20ms frame, in units of the
/// codec's declared RTP clock rate. G.711's clock is 8000 Hz; Opus's RTP
/// clock is always 48000 Hz regardless of the audio's actual sample rate
/// (RFC 7587), even though our pipeline encodes/decodes Opus at 8 kHz.
fn ts_increment_for(codec: AudioCodec) -> u32 {
    match codec {
        AudioCodec::Opus => 960,
        AudioCodec::Pcmu | AudioCodec::Pcma | AudioCodec::G722 => 160,
    }
}

/// The RTP wire transport: a plain local UDP socket, or a TURN-relayed `Conn`
/// (see `deelip_nat::allocate_relay`). Both shapes speak send_to/recv_from, so
/// everything downstream (codec dispatch, SRTP, DTMF, jitter buffer) is
/// identical regardless of which one is active.
enum RtpSocket {
    Direct(UdpSocket),
    Relay(Arc<dyn Conn + Send + Sync>),
}

impl RtpSocket {
    async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> anyhow::Result<()> {
        match self {
            Self::Direct(s) => { s.send_to(buf, addr).await?; }
            Self::Relay(c)  => { c.send_to(buf, addr).await?; }
        }
        Ok(())
    }

    async fn recv_from(&self, buf: &mut [u8]) -> anyhow::Result<(usize, SocketAddr)> {
        match self {
            Self::Direct(s) => Ok(s.recv_from(buf).await?),
            Self::Relay(c)  => Ok(c.recv_from(buf).await?),
        }
    }
}

/// A second RTP leg for 3-way conferencing — bundles everything a leg needs
/// beyond what `MediaEngine::start`'s existing params already cover for
/// leg 1. Reuses the leg's own already-allocated RTP port/relay (nothing
/// new to allocate); the two legs may have negotiated different codecs.
pub struct ConferenceLeg {
    pub local_rtp_port: u16,
    pub remote_rtp:     SocketAddr,
    pub codec:          AudioCodec,
    pub dtmf_pt:        Option<u8>,
    pub srtp:           Option<SrtpSession>,
    pub relay:          Option<Arc<dyn Conn + Send + Sync>>,
}

// ── Call statistics ───────────────────────────────────────────────────────────

/// Local-only RTP stats for one leg — there's no RTCP in this codebase, so
/// loss/jitter reflect what *we* observe receiving, not what the remote
/// reports observing from us (the usual "local stats panel" scope, same as
/// what most softphones show without a full RTCP implementation).
#[derive(Debug, Clone, Default)]
pub struct LegStats {
    pub packets_sent:     u64,
    pub bytes_sent:       u64,
    pub packets_received: u64,
    pub bytes_received:   u64,
    /// Best-effort count of missing RTP sequence numbers on the receive
    /// side (gaps > 1000 are treated as reordering/restart noise, not loss).
    pub packets_lost:     u64,
    /// RFC 3550 §6.4.1 interarrival jitter estimate, in milliseconds.
    pub jitter_ms:        f64,
}

#[derive(Debug, Clone, Default)]
pub struct CallStatsSnapshot {
    pub leg1: LegStats,
    /// Only `Some` in conference mode (mirrors `MediaEngine::recv_task2`).
    pub leg2: Option<LegStats>,
}

type SharedStats = Arc<Mutex<CallStatsSnapshot>>;

/// Per-recv-task running state for loss/jitter calculation — deliberately
/// not part of the shared/lockable `LegStats` since only the owning recv
/// task ever touches it.
#[derive(Default)]
struct JitterTracker {
    last_seq:     Option<u16>,
    last_arrival: Option<Instant>,
    last_rtp_ts:  Option<u32>,
}

impl JitterTracker {
    /// Update loss/jitter running state from a newly-received voice packet
    /// and fold the results into `stats`.
    fn observe(&mut self, stats: &mut LegStats, pkt: &RtpPacket, clock_hz: f64) {
        if let Some(prev) = self.last_seq {
            let expected = prev.wrapping_add(1);
            if pkt.sequence != expected {
                let gap = pkt.sequence.wrapping_sub(expected);
                if gap < 1000 {
                    stats.packets_lost += gap as u64;
                }
            }
        }
        self.last_seq = Some(pkt.sequence);

        let now = Instant::now();
        if let (Some(prev_arrival), Some(prev_ts)) = (self.last_arrival, self.last_rtp_ts) {
            let arrival_diff_ms = now.duration_since(prev_arrival).as_secs_f64() * 1000.0;
            let rtp_diff_ms = (pkt.timestamp as i64 - prev_ts as i64).unsigned_abs() as f64 / clock_hz * 1000.0;
            let d = (arrival_diff_ms - rtp_diff_ms).abs();
            stats.jitter_ms += (d - stats.jitter_ms) / 16.0;
        }
        self.last_arrival = Some(now);
        self.last_rtp_ts = Some(pkt.timestamp);
    }
}

/// RTP clock rate for jitter math (RFC 7587: Opus's RTP clock is always
/// 48000 regardless of the audio's actual sample rate; everything else
/// here is 8000 — see `ts_increment_for`'s own doc comment).
fn clock_hz_for(codec: AudioCodec) -> f64 {
    match codec {
        AudioCodec::Opus => 48000.0,
        AudioCodec::Pcmu | AudioCodec::Pcma | AudioCodec::G722 => 8000.0,
    }
}

// ── MediaEngine ───────────────────────────────────────────────────────────────

/// Manages the audio ↔ RTP pipeline for a single active call, optionally
/// bridging a second RTP leg for a 3-way conference (see `ConferenceLeg`).
pub struct MediaEngine {
    _audio:     AudioStreams,
    send_task:  tokio::task::JoinHandle<()>,
    recv_task:  tokio::task::JoinHandle<()>,
    /// Only `Some` when started with a `second_leg` (conference mode).
    recv_task2: Option<tokio::task::JoinHandle<()>>,
    stop_tx:    watch::Sender<bool>,
    dtmf_tx:    mpsc::UnboundedSender<char>,
    muted:      Arc<AtomicBool>,
    /// Owned by `MediaEngine` itself (not just captured by the send task's
    /// closure) so `stop()` can finalize it deterministically from the
    /// synchronous caller side — `stop()` aborts both tasks without awaiting
    /// them, so anything only reachable from inside a task would be subject
    /// to a cancellation race and might never run its cleanup.
    recorder: Arc<Mutex<Option<WavWriter>>>,
    stats:    SharedStats,
}

impl MediaEngine {
    /// Start the media engine.
    /// - `local_rtp_port`: local UDP port for RTP.
    /// - `remote_rtp`:     remote RTP endpoint.
    /// - `codec`:          negotiated voice codec (PCMU/PCMA/Opus).
    /// - `dtmf_pt`:        DTMF telephone-event payload type (typically Some(101)).
    /// - `srtp`:           local/remote SDES-SRTP keys, if the call negotiated encrypted media.
    /// - `relay`:          a TURN-allocated relay `Conn` (see `deelip_nat::allocate_relay`),
    ///   if the call is relaying media instead of using a direct local socket.
    /// - `echo_cancellation`: run acoustic echo cancellation on the mic path
    ///   (see `crate::aec`) — only useful on speakers/mic, not headsets.
    /// - `input_device`/`output_device`: specific cpal device names to use,
    ///   falling back to the system default if unset or not found.
    /// - `recording_enabled`: record this call to a stereo WAV file (left =
    ///   near-end mic, right = far-end/mixed received audio) under
    ///   `deelip_config::recordings_dir()`.
    /// - `call_id`: used to name the recording file; ignored if recording is disabled.
    /// - `second_leg`: `Some` to bridge a second remote party into a 3-way
    ///   conference, sharing this same mic/speaker pair — `None` (every
    ///   existing call site) is the ordinary single-leg call, unchanged.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        local_rtp_port: u16,
        remote_rtp:     SocketAddr,
        codec:          AudioCodec,
        dtmf_pt:        Option<u8>,
        srtp:           Option<SrtpSession>,
        relay:          Option<Arc<dyn Conn + Send + Sync>>,
        echo_cancellation: bool,
        input_device:   Option<&str>,
        output_device:  Option<&str>,
        recording_enabled: bool,
        call_id:        &str,
        second_leg:     Option<ConferenceLeg>,
    ) -> anyhow::Result<Self> {
        let (audio_streams, mut cap_rx, hw_playback_tx, echo_ref) =
            open_streams(input_device, output_device, echo_cancellation)
                .context("Opening audio streams")?;

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
        let payload_type      = codec.payload_type();

        // Per RFC 4568, each side's a=crypto line declares the key THAT SIDE uses to
        // encrypt what it sends; the peer decrypts with that same key. So we encrypt
        // outgoing traffic with our OWN declared (local) key, and decrypt incoming
        // traffic with the REMOTE's declared key.
        let mut encrypt_ctx: Option<SrtpContext> = srtp.as_ref().map(|s| {
            SrtpContext::new(&s.local.key, &s.local.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None)
        }).transpose().context("Creating SRTP encrypt context")?;
        let mut decrypt_ctx: Option<SrtpContext> = srtp.as_ref().map(|s| {
            SrtpContext::new(&s.remote.key, &s.remote.salt, ProtectionProfile::Aes128CmHmacSha1_80, Some(srtp_replay_protection(64)), None)
        }).transpose().context("Creating SRTP decrypt context")?;

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
            leg2_encrypt_ctx = leg.srtp.as_ref().map(|s| {
                SrtpContext::new(&s.local.key, &s.local.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None)
            }).transpose().context("Creating leg2 SRTP encrypt context")?;
            leg2_decrypt_ctx = leg.srtp.as_ref().map(|s| {
                SrtpContext::new(&s.remote.key, &s.remote.salt, ProtectionProfile::Aes128CmHmacSha1_80, Some(srtp_replay_protection(64)), None)
            }).transpose().context("Creating leg2 SRTP decrypt context")?;
            leg2_socket  = Some(socket2);
            leg2_remote  = Some(leg.remote_rtp);
            leg2_codec   = Some(leg.codec);
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

        let (stop_tx, stop_rx)   = watch::channel(false);
        let mut stop_send = stop_rx.clone();
        let mut stop_recv = stop_rx;

        let (dtmf_tx, mut dtmf_rx) = mpsc::unbounded_channel::<char>();

        let recorder: Arc<Mutex<Option<WavWriter>>> = Arc::new(Mutex::new(
            if recording_enabled {
                match open_recorder(call_id) {
                    Ok(w) => Some(w),
                    Err(e) => { error!("Failed to start call recording: {e}"); None }
                }
            } else {
                None
            }
        ));

        // ── Send task ─────────────────────────────────────────────────────────
        let send_sock    = socket.clone();
        let send_sock2   = leg2_socket.clone();
        let mut rtp_send = RtpSender::new(payload_type, ts_increment_for(codec));
        let dtmf_ssrc    = rtp_send.ssrc;
        let mut dtmf_seq = 0u16;
        let mut opus_enc = if codec == AudioCodec::Opus {
            Some(OpusEncoder::new().context("Creating Opus encoder")?)
        } else {
            None
        };
        let mut g722_enc = if codec == AudioCodec::G722 { Some(G722Encoder::new()) } else { None };
        let mut opus_enc2 = match leg2_codec {
            Some(AudioCodec::Opus) => Some(OpusEncoder::new().context("Creating Opus encoder (leg2)")?),
            _ => None,
        };
        let mut g722_enc2 = match leg2_codec {
            Some(AudioCodec::G722) => Some(G722Encoder::new()),
            _ => None,
        };
        let mut rtp_send2 = leg2_codec.map(|c| RtpSender::new(c.payload_type(), ts_increment_for(c)));
        let dtmf_ssrc2    = rtp_send2.as_ref().map(|r| r.ssrc);
        let mut dtmf_seq2 = 0u16;

        let mut echo_canceller = echo_ref.as_ref().map(|_| EchoCanceller::new());
        let muted = Arc::new(AtomicBool::new(false));
        let send_muted = muted.clone();
        let send_recorder = recorder.clone();
        let send_leg1_buf = leg1_buf.clone();
        let send_leg2_buf = leg2_buf.clone();
        let send_stats = stats.clone();

        let send_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(pcm) = cap_rx.recv() => {
                        let pcm = if send_muted.load(Ordering::Relaxed) {
                            vec![0i16; pcm.len()]
                        } else {
                            pcm
                        };
                        let pcm = match (echo_canceller.as_mut(), echo_ref.as_ref()) {
                            (Some(canceller), Some(echo_ref)) => canceller.process(&pcm, echo_ref),
                            _ => pcm,
                        };

                        // Leg 1: encode + send.
                        let encoded = match codec {
                            AudioCodec::Opus => opus_enc.as_mut().unwrap().encode(&pcm),
                            AudioCodec::G722 => g722_enc.as_mut().unwrap().encode(&pcm),
                            AudioCodec::Pcma => encode_pcma(&pcm),
                            AudioCodec::Pcmu => encode_pcmu(&pcm),
                        };
                        let bytes = rtp_send.next_packet(encoded).encode();
                        let out = match encrypt_ctx.as_mut() {
                            Some(ctx) => match ctx.encrypt_rtp(&bytes) {
                                Ok(b) => b.to_vec(),
                                Err(e) => { error!("SRTP encrypt: {e}"); continue; }
                            },
                            None => bytes,
                        };
                        match send_sock.send_to(&out, remote_rtp).await {
                            Ok(()) => {
                                let mut s = send_stats.lock().unwrap();
                                s.leg1.packets_sent += 1;
                                s.leg1.bytes_sent   += out.len() as u64;
                            }
                            Err(e) => error!("RTP send: {e}"),
                        }

                        // Leg 2 (conference): encode independently (may differ
                        // codec) + send, without aborting leg 1's already-sent
                        // packet or the mix/playback step below on failure.
                        if let (Some(sock2), Some(remote2), Some(c2), Some(sender2)) =
                            (send_sock2.as_ref(), leg2_remote, leg2_codec, rtp_send2.as_mut())
                        {
                            let encoded2 = match c2 {
                                AudioCodec::Opus => opus_enc2.as_mut().unwrap().encode(&pcm),
                                AudioCodec::G722 => g722_enc2.as_mut().unwrap().encode(&pcm),
                                AudioCodec::Pcma => encode_pcma(&pcm),
                                AudioCodec::Pcmu => encode_pcmu(&pcm),
                            };
                            let bytes2 = sender2.next_packet(encoded2).encode();
                            let out2 = match leg2_encrypt_ctx.as_mut() {
                                Some(ctx) => match ctx.encrypt_rtp(&bytes2) {
                                    Ok(b) => Some(b.to_vec()),
                                    Err(e) => { error!("SRTP encrypt (leg2): {e}"); None }
                                },
                                None => Some(bytes2),
                            };
                            if let Some(out2) = out2 {
                                match sock2.send_to(&out2, remote2).await {
                                    Ok(()) => {
                                        let mut s = send_stats.lock().unwrap();
                                        if let Some(leg2) = s.leg2.as_mut() {
                                            leg2.packets_sent += 1;
                                            leg2.bytes_sent   += out2.len() as u64;
                                        }
                                    }
                                    Err(e) => error!("RTP send (leg2): {e}"),
                                }
                            }
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
                            for pkt in pkts {
                                let out = match encrypt_ctx.as_mut() {
                                    Some(ctx) => match ctx.encrypt_rtp(&pkt) {
                                        Ok(b) => b.to_vec(),
                                        Err(e) => { error!("SRTP encrypt (DTMF): {e}"); continue; }
                                    },
                                    None => pkt,
                                };
                                if let Err(e) = send_sock.send_to(&out, remote_rtp).await {
                                    error!("DTMF RTP send: {e}");
                                }
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
                                for pkt in pkts2 {
                                    let out = match leg2_encrypt_ctx.as_mut() {
                                        Some(ctx) => match ctx.encrypt_rtp(&pkt) {
                                            Ok(b) => b.to_vec(),
                                            Err(e) => { error!("SRTP encrypt (DTMF leg2): {e}"); continue; }
                                        },
                                        None => pkt,
                                    };
                                    if let Err(e) = sock2.send_to(&out, remote2).await {
                                        error!("DTMF RTP send (leg2): {e}");
                                    }
                                }
                            }
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
        let recv_sock = socket;
        let mut opus_dec = if codec == AudioCodec::Opus {
            Some(OpusDecoder::new().context("Creating Opus decoder")?)
        } else {
            None
        };
        let mut g722_dec = if codec == AudioCodec::G722 { Some(G722Decoder::new()) } else { None };
        let recv_stats = stats.clone();
        let recv_clock_hz = clock_hz_for(codec);

        let recv_task = tokio::spawn(async move {
            let mut jitter = JitterTracker::default();
            let mut buf = vec![0u8; 2048];
            loop {
                tokio::select! {
                    Ok((len, _from)) = recv_sock.recv_from(&mut buf) => {
                        let decrypted;
                        let plain: &[u8] = if let Some(ctx) = decrypt_ctx.as_mut() {
                            match ctx.decrypt_rtp(&buf[..len]) {
                                Ok(b) => { decrypted = b; &decrypted[..] }
                                Err(e) => { warn!("SRTP decrypt: {e}"); continue; }
                            }
                        } else {
                            &buf[..len]
                        };
                        if let Some(pkt) = RtpPacket::decode(plain) {
                            // Ignore DTMF packets; only decode voice frames
                            if pkt.payload_type == DTMF_PAYLOAD_TYPE
                                || pkt.payload_type == dtmf_payload_type
                            {
                                continue;
                            }
                            {
                                let mut s = recv_stats.lock().unwrap();
                                s.leg1.packets_received += 1;
                                s.leg1.bytes_received   += len as u64;
                                jitter.observe(&mut s.leg1, &pkt, recv_clock_hz);
                            }
                            let pcm = match codec {
                                AudioCodec::Opus => opus_dec.as_mut().unwrap().decode(&pkt.payload),
                                AudioCodec::G722 => g722_dec.as_mut().unwrap().decode(&pkt.payload),
                                AudioCodec::Pcma => decode_pcma(&pkt.payload),
                                AudioCodec::Pcmu => decode_pcmu(&pkt.payload),
                            };
                            push_to_jitter(&leg1_buf, &pcm);
                        }
                    }
                    Ok(()) = stop_recv.changed() => {
                        if *stop_recv.borrow() { break; }
                    }
                }
            }
            debug!("RTP recv task stopped");
        });

        // ── Recv task (leg 2, conference only) ────────────────────────────────
        let recv_task2 = if let Some(leg) = &second_leg {
            let recv_sock2 = leg2_socket.clone().unwrap();
            let mut opus_dec2 = if leg.codec == AudioCodec::Opus {
                Some(OpusDecoder::new().context("Creating Opus decoder (leg2)")?)
            } else {
                None
            };
            let mut g722_dec2 = if leg.codec == AudioCodec::G722 { Some(G722Decoder::new()) } else { None };
            let codec2   = leg.codec;
            let pt2      = leg2_dtmf_pt.unwrap();
            let mut decrypt_ctx2 = leg2_decrypt_ctx;
            let leg2_buf_recv = leg2_buf.clone().unwrap();
            let mut stop_recv2 = stop_tx.subscribe();
            let recv_stats2 = stats.clone();
            let recv_clock_hz2 = clock_hz_for(codec2);

            Some(tokio::spawn(async move {
                let mut jitter2 = JitterTracker::default();
                let mut buf = vec![0u8; 2048];
                loop {
                    tokio::select! {
                        Ok((len, _from)) = recv_sock2.recv_from(&mut buf) => {
                            let decrypted;
                            let plain: &[u8] = if let Some(ctx) = decrypt_ctx2.as_mut() {
                                match ctx.decrypt_rtp(&buf[..len]) {
                                    Ok(b) => { decrypted = b; &decrypted[..] }
                                    Err(e) => { warn!("SRTP decrypt (leg2): {e}"); continue; }
                                }
                            } else {
                                &buf[..len]
                            };
                            if let Some(pkt) = RtpPacket::decode(plain) {
                                if pkt.payload_type == DTMF_PAYLOAD_TYPE || pkt.payload_type == pt2 {
                                    continue;
                                }
                                {
                                    let mut s = recv_stats2.lock().unwrap();
                                    if let Some(leg2) = s.leg2.as_mut() {
                                        leg2.packets_received += 1;
                                        leg2.bytes_received   += len as u64;
                                        jitter2.observe(leg2, &pkt, recv_clock_hz2);
                                    }
                                }
                                let pcm = match codec2 {
                                    AudioCodec::Opus => opus_dec2.as_mut().unwrap().decode(&pkt.payload),
                                    AudioCodec::G722 => g722_dec2.as_mut().unwrap().decode(&pkt.payload),
                                    AudioCodec::Pcma => decode_pcma(&pkt.payload),
                                    AudioCodec::Pcmu => decode_pcmu(&pkt.payload),
                                };
                                push_to_jitter(&leg2_buf_recv, &pcm);
                            }
                        }
                        Ok(()) = stop_recv2.changed() => {
                            if *stop_recv2.borrow() { break; }
                        }
                    }
                }
                debug!("RTP recv task (leg2) stopped");
            }))
        } else {
            None
        };

        Ok(Self { _audio: audio_streams, send_task, recv_task, recv_task2, stop_tx, dtmf_tx, muted, recorder, stats })
    }

    /// Queue a DTMF digit for immediate out-of-band RTP transmission.
    pub fn send_dtmf(&self, digit: char) {
        let _ = self.dtmf_tx.send(digit);
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

    /// Async so callers can actually wait for the send/recv tasks to finish,
    /// not just be scheduled for cancellation -- `abort()` alone doesn't
    /// guarantee the task (and whatever it holds, e.g. a TURN relay `Conn`)
    /// is really gone by the time this returns. That matters once a caller
    /// can immediately reuse the *same* relay `Conn` in a brand new engine
    /// (conference merge does exactly this): if the old task's `recv_from`
    /// is still alive even momentarily, it races the new engine's recv task
    /// for the same incoming packets and can "steal" them, silently
    /// starving the new one. Awaiting here closes that window.
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
        // Finalize here (not inside the send task) so it runs deterministically
        // regardless of the abort race above -- hound needs an explicit
        // finalize() to fix up the RIFF header sizes; Drop alone would leave
        // a malformed file.
        if let Some(writer) = self.recorder.lock().unwrap().take() {
            if let Err(e) = writer.finalize() {
                error!("Failed to finalize call recording: {e}");
            }
        }
    }
}

fn push_to_jitter(jitter: &PlaybackTx, pcm: &[i16]) {
    let max = FRAME_SAMPLES * 50; // cap at 1 second
    let mut buf = jitter.lock().unwrap();
    for &s in pcm {
        if buf.len() < max { buf.push_back(s); }
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
    a.iter().zip(b.iter())
        .map(|(&x, &y)| {
            let mixed = (x as i32) / 2 + (y as i32) / 2;
            mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

/// Write one interleaved stereo frame (left = near-end `pcm`, right = the
/// already-mixed far-end audio for this same frame). No-op if recording
/// isn't enabled for this call.
fn write_recording(recorder: &Mutex<Option<WavWriter>>, near: &[i16], far: &[i16]) {
    let mut guard = recorder.lock().unwrap();
    let Some(writer) = guard.as_mut() else { return };
    for (i, &near_sample) in near.iter().enumerate() {
        let far_sample = far.get(i).copied().unwrap_or(0);
        let _ = writer.write_sample(near_sample);
        let _ = writer.write_sample(far_sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deelip_sip::SrtpParams;

    #[test]
    fn jitter_tracker_counts_missing_sequence_numbers_as_loss() {
        let mut tracker = JitterTracker::default();
        let mut stats = LegStats::default();
        let pkt0 = RtpPacket::new(0, 100, 1600, 1, vec![]);
        let pkt1 = RtpPacket::new(0, 103, 1760, 1, vec![]); // 101, 102 missing
        tracker.observe(&mut stats, &pkt0, 8000.0);
        tracker.observe(&mut stats, &pkt1, 8000.0);
        assert_eq!(stats.packets_lost, 2);
    }

    #[test]
    fn jitter_tracker_ignores_huge_gaps_as_reordering_noise() {
        let mut tracker = JitterTracker::default();
        let mut stats = LegStats::default();
        let pkt0 = RtpPacket::new(0, 100, 1600, 1, vec![]);
        let pkt1 = RtpPacket::new(0, 50_000, 1760, 1, vec![]);
        tracker.observe(&mut stats, &pkt0, 8000.0);
        tracker.observe(&mut stats, &pkt1, 8000.0);
        assert_eq!(stats.packets_lost, 0);
    }

    #[test]
    fn jitter_tracker_reports_zero_jitter_for_perfectly_paced_packets() {
        let mut tracker = JitterTracker::default();
        let mut stats = LegStats::default();
        // Three packets 20ms apart in both RTP-timestamp and (as far as this
        // synchronous test can approximate) wall-clock terms.
        for (seq, ts) in [(1u16, 1600u32), (2, 1760), (3, 1920)] {
            let pkt = RtpPacket::new(0, seq, ts, 1, vec![]);
            tracker.observe(&mut stats, &pkt, 8000.0);
        }
        assert!(stats.jitter_ms < 5.0, "jitter should stay small for evenly-paced packets, got {}", stats.jitter_ms);
    }

    #[test]
    fn srtp_roundtrip_preserves_rtp_payload() {
        let params = SrtpParams::generate();
        let mut enc_ctx = SrtpContext::new(
            &params.key, &params.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None,
        ).unwrap();
        let mut dec_ctx = SrtpContext::new(
            &params.key, &params.salt, ProtectionProfile::Aes128CmHmacSha1_80,
            Some(srtp_replay_protection(64)), None,
        ).unwrap();

        let raw = RtpPacket::new(0, 1, 160, 0xDEAD_BEEF, vec![1, 2, 3, 4, 5]).encode();

        let encrypted = enc_ctx.encrypt_rtp(&raw).unwrap();
        assert!(encrypted.len() > raw.len(), "SRTP appends an auth tag");

        let decrypted = dec_ctx.decrypt_rtp(&encrypted).unwrap();
        assert_eq!(&decrypted[..], &raw[..]);

        let decoded = RtpPacket::decode(&decrypted).unwrap();
        assert_eq!(decoded.payload, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn srtp_decrypt_rejects_wrong_key() {
        let params_a = SrtpParams::generate();
        let params_b = SrtpParams::generate();
        let mut enc_ctx = SrtpContext::new(
            &params_a.key, &params_a.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None,
        ).unwrap();
        let mut dec_ctx = SrtpContext::new(
            &params_b.key, &params_b.salt, ProtectionProfile::Aes128CmHmacSha1_80,
            Some(srtp_replay_protection(64)), None,
        ).unwrap();

        let raw = RtpPacket::new(0, 1, 160, 0xDEAD_BEEF, vec![1, 2, 3]).encode();
        let encrypted = enc_ctx.encrypt_rtp(&raw).unwrap();
        assert!(dec_ctx.decrypt_rtp(&encrypted).is_err());
    }

    #[test]
    fn mix_frames_sums_and_clamps() {
        let a = vec![100i16, -100, i16::MAX, i16::MIN];
        let b = vec![50i16, -50, i16::MAX, i16::MIN];
        let mixed = mix_frames(&a, &b);
        // Each leg halved (integer truncation) before summing:
        // 100/2 + 50/2 = 75; -100/2 + -50/2 = -75;
        // MAX/2 + MAX/2 = 32766 (truncation loses 1); MIN/2 + MIN/2 = MIN exactly.
        assert_eq!(mixed, vec![75, -75, 32766, i16::MIN]);
    }
}
