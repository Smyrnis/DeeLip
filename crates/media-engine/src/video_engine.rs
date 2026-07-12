//! Standalone video RTP engine: capture-frame → H.264 encode → RTP send,
//! and RTP recv → H.264 decode → latest-decoded-frame -- its own
//! independent construct, deliberately *not* part of `MediaEngine`. Full
//! picture: `docs/crates/media-engine.md`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::warn;
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;
use webrtc_util::Conn;

use deelip_sip::sdp::H264_PAYLOAD_TYPE;
use deelip_sip::SrtpSession;

use crate::engine::RtpSocket;
use crate::rtp::{RtpPacket, RtpSender};
use crate::stats::LegStats;
use crate::video_codec::{H264Decoder, H264Encoder, Yuv420Frame};
use crate::video_rtp::{fragment_nal_units, reassemble_nal_units};

/// RTP payload fragmentation MTU -- conservative, safely under a typical
/// 1500-byte Ethernet MTU once IP/UDP/(S)RTP headers are accounted for.
const RTP_MTU: usize = 1200;
/// H.264's RTP clock is always 90kHz (RFC 6184), regardless of the actual
/// capture/encode frame rate.
const VIDEO_CLOCK_HZ: u32 = 90_000;

/// A second RTP leg for the local 3-way conference -- bundles what a video
/// leg needs beyond `VideoEngine::start`'s existing leg-1 params. Unlike
/// audio's `ConferenceLeg`, there's no `codec`/`dtmf_pt`: H.264 is the only
/// video codec and video has no DTMF concept, so this is just the
/// RTP/SRTP/relay identity of the second remote party's already-negotiated
/// video leg.
pub struct VideoConferenceLeg {
    pub local_rtp_port: u16,
    pub remote_rtp: SocketAddr,
    pub srtp: Option<SrtpSession>,
    pub relay: Option<Arc<dyn Conn + Send + Sync>>,
}

pub struct VideoEngine {
    send_task: tokio::task::JoinHandle<()>,
    recv_task: tokio::task::JoinHandle<()>,
    /// Only `Some` when started with a `second_leg` (conference mode) --
    /// mirrors `MediaEngine::recv_task2`.
    recv_task2: Option<tokio::task::JoinHandle<()>>,
    stop_tx: watch::Sender<bool>,
    latest_decoded_frame: Arc<Mutex<Option<Yuv420Frame>>>,
    latest_decoded_frame2: Arc<Mutex<Option<Yuv420Frame>>>,
    stats: Arc<Mutex<LegStats>>,
    /// Local camera on/off -- mirrors `MediaEngine::muted`'s naming, but
    /// checked in `send_loop` *before* touching `frame_source`/the encoder
    /// at all, so muting skips encode+send entirely rather than substituting
    /// a blank frame. `recv_loop` (decode/display of the remote party) is
    /// completely unaffected either way.
    video_muted: Arc<AtomicBool>,
}

impl VideoEngine {
    /// - `frame_source`: the latest captured frame to encode/send, polled
    ///   once per `target_fps` tick -- typically
    ///   `video_capture::CaptureHandle::frame_slot()` for a real camera, or
    ///   a plain `Arc::new(Mutex::new(...))` fed synthetically (tests, or
    ///   any other frame producer).
    /// - `bitrate_bps`: target H.264 encode bitrate.
    /// - `second_leg`: `Some` to fan this same encoded local stream out to a
    ///   second remote party too (local 3-way conference), decoding/
    ///   displaying each leg's incoming video independently -- mirrors
    ///   `MediaEngine`'s "encode once per real source, decode per leg"
    ///   shape. `None` (every non-conference call) is unchanged.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        local_rtp_port: u16, remote_rtp: SocketAddr, srtp: Option<SrtpSession>,
        relay: Option<Arc<dyn Conn + Send + Sync>>, frame_source: Arc<Mutex<Option<Yuv420Frame>>>, target_fps: u32,
        bitrate_bps: u32, second_leg: Option<VideoConferenceLeg>,
    ) -> anyhow::Result<Self> {
        let socket = Arc::new(match relay {
            Some(conn) => RtpSocket::Relay(conn),
            None => RtpSocket::Direct(
                UdpSocket::bind(format!("0.0.0.0:{local_rtp_port}"))
                    .await
                    .with_context(|| format!("Binding video RTP on :{local_rtp_port}"))?,
            ),
        });

        // Same RFC 4568 key-direction convention as `MediaEngine::start`'s
        // leg 1: encrypt outgoing with our own declared key, decrypt
        // incoming with the remote's declared key.
        let encrypt_ctx: Option<SrtpContext> = srtp
            .as_ref()
            .map(|s| SrtpContext::new(&s.local.key, &s.local.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None))
            .transpose()
            .context("Creating video SRTP encrypt context")?;
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
            .context("Creating video SRTP decrypt context")?;

        // ── Leg 2 (conference), if present ────────────────────────────────────
        let mut leg2_send: Option<(Arc<RtpSocket>, SocketAddr, Option<SrtpContext>)> = None;
        let mut leg2_recv: Option<(Arc<RtpSocket>, Option<SrtpContext>)> = None;
        if let Some(leg) = second_leg {
            let socket2 = Arc::new(match leg.relay {
                Some(conn) => RtpSocket::Relay(conn),
                None => RtpSocket::Direct(
                    UdpSocket::bind(format!("0.0.0.0:{}", leg.local_rtp_port))
                        .await
                        .with_context(|| format!("Binding video RTP (leg2) on :{}", leg.local_rtp_port))?,
                ),
            });
            let leg2_encrypt_ctx: Option<SrtpContext> = leg
                .srtp
                .as_ref()
                .map(|s| {
                    SrtpContext::new(&s.local.key, &s.local.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None)
                })
                .transpose()
                .context("Creating video SRTP encrypt context (leg2)")?;
            let leg2_decrypt_ctx: Option<SrtpContext> = leg
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
                .context("Creating video SRTP decrypt context (leg2)")?;
            leg2_send = Some((socket2.clone(), leg.remote_rtp, leg2_encrypt_ctx));
            leg2_recv = Some((socket2, leg2_decrypt_ctx));
        }

        let (stop_tx, stop_rx) = watch::channel(false);
        let stats: Arc<Mutex<LegStats>> = Arc::new(Mutex::new(LegStats::default()));
        let latest_decoded_frame: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
        let latest_decoded_frame2: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
        let video_muted = Arc::new(AtomicBool::new(false));

        let send_task = tokio::spawn(Self::send_loop(
            socket.clone(),
            remote_rtp,
            frame_source,
            target_fps,
            bitrate_bps,
            encrypt_ctx,
            stats.clone(),
            stop_rx.clone(),
            video_muted.clone(),
            leg2_send,
        ));
        let recv_task = tokio::spawn(Self::recv_loop(
            socket,
            decrypt_ctx,
            latest_decoded_frame.clone(),
            stats.clone(),
            stop_rx.clone(),
        ));
        let recv_task2 = leg2_recv.map(|(socket2, decrypt_ctx2)| {
            tokio::spawn(Self::recv_loop(socket2, decrypt_ctx2, latest_decoded_frame2.clone(), stats.clone(), stop_rx))
        });

        Ok(Self {
            send_task,
            recv_task,
            recv_task2,
            stop_tx,
            latest_decoded_frame,
            latest_decoded_frame2,
            stats,
            video_muted,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_loop(
        socket: Arc<RtpSocket>, remote_rtp: SocketAddr, frame_source: Arc<Mutex<Option<Yuv420Frame>>>, target_fps: u32,
        bitrate_bps: u32, mut encrypt_ctx: Option<SrtpContext>, stats: Arc<Mutex<LegStats>>,
        mut stop_rx: watch::Receiver<bool>, video_muted: Arc<AtomicBool>,
        mut leg2: Option<(Arc<RtpSocket>, SocketAddr, Option<SrtpContext>)>,
    ) {
        let mut encoder = match H264Encoder::new(bitrate_bps) {
            Ok(e) => e,
            Err(e) => {
                warn!("Video send task: failed to create H.264 encoder, giving up: {e:#}");
                return;
            }
        };
        let fps = target_fps.max(1);
        // `ts_increment: 0` is deliberate -- see `docs/crates/media-engine.md`'s
        // "Video RTP timestamping" section for why.
        let mut sender = RtpSender::new(H264_PAYLOAD_TYPE, 0);
        // Leg 2 (conference) gets its own independent `RtpSender` -- a fresh
        // random SSRC and its own sequence/timestamp counters, a distinct
        // RTP session from the receiving party's point of view -- fed the
        // same encoded fragments as leg 1 every tick, since the encode
        // itself is shared (one camera, one `H264Encoder`).
        let mut sender2 = leg2.is_some().then(|| RtpSender::new(H264_PAYLOAD_TYPE, 0));
        let ticks_per_frame = VIDEO_CLOCK_HZ / fps;
        let mut interval = tokio::time::interval(Duration::from_secs_f64(1.0 / fps as f64));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if video_muted.load(Ordering::Relaxed) { continue }
                    let Some(frame) = frame_source.lock().unwrap().clone() else { continue };
                    let bitstream = match encoder.encode(&frame) {
                        Ok(b) => b,
                        Err(e) => {
                            warn!("Video encode failed: {e:#}");
                            continue;
                        }
                    };
                    if bitstream.is_empty() {
                        continue;
                    }
                    let packets = fragment_nal_units(&bitstream, RTP_MTU);
                    let last = packets.len().saturating_sub(1);
                    for (i, payload) in packets.into_iter().enumerate() {
                        let marker = i == last;
                        if let (Some(sender2), Some((sock2, remote2, encrypt2))) = (sender2.as_mut(), leg2.as_mut()) {
                            let mut pkt2 = sender2.next_packet(payload.clone());
                            pkt2.marker = marker;
                            let wire2 = match encrypt2 {
                                Some(ctx) => match ctx.encrypt_rtp(&pkt2.encode()) {
                                    Ok(enc) => Some(enc),
                                    Err(e) => {
                                        warn!("Video SRTP encrypt failed (leg2): {e:#}");
                                        None
                                    }
                                },
                                None => Some(pkt2.encode().into()),
                            };
                            if let Some(wire2) = wire2 {
                                match sock2.send_to(&wire2, *remote2).await {
                                    Ok(()) => {
                                        let mut s = stats.lock().unwrap();
                                        s.packets_sent += 1;
                                        s.bytes_sent += wire2.len() as u64;
                                    }
                                    Err(e) => warn!("Video RTP send failed (leg2): {e:#}"),
                                }
                            }
                        }

                        let mut pkt = sender.next_packet(payload);
                        pkt.marker = marker;
                        let wire = match &mut encrypt_ctx {
                            Some(ctx) => match ctx.encrypt_rtp(&pkt.encode()) {
                                Ok(enc) => enc,
                                Err(e) => {
                                    warn!("Video SRTP encrypt failed: {e:#}");
                                    continue;
                                }
                            },
                            None => pkt.encode().into(),
                        };
                        if let Err(e) = socket.send_to(&wire, remote_rtp).await {
                            warn!("Video RTP send failed: {e:#}");
                            continue;
                        }
                        let mut s = stats.lock().unwrap();
                        s.packets_sent += 1;
                        s.bytes_sent += wire.len() as u64;
                    }
                    sender.timestamp = sender.timestamp.wrapping_add(ticks_per_frame);
                    if let Some(sender2) = sender2.as_mut() {
                        sender2.timestamp = sender2.timestamp.wrapping_add(ticks_per_frame);
                    }
                }
                Ok(()) = stop_rx.changed() => {
                    if *stop_rx.borrow() { break; }
                }
            }
        }
    }

    async fn recv_loop(
        socket: Arc<RtpSocket>, mut decrypt_ctx: Option<SrtpContext>,
        latest_decoded_frame: Arc<Mutex<Option<Yuv420Frame>>>, stats: Arc<Mutex<LegStats>>,
        mut stop_rx: watch::Receiver<bool>,
    ) {
        let mut decoder = match H264Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                warn!("Video recv task: failed to create H.264 decoder, giving up: {e:#}");
                return;
            }
        };
        // Accumulates one frame's worth of fragments in arrival order --
        // see this module's doc comment on the no-reordering simplification.
        let mut frame_fragments: Vec<Vec<u8>> = Vec::new();
        let mut jitter = JitterState::default();
        let mut buf = vec![0u8; 65_535];

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    let (len, _from) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("Video RTP recv failed: {e:#}");
                            continue;
                        }
                    };
                    let plain = match &mut decrypt_ctx {
                        Some(ctx) => match ctx.decrypt_rtp(&buf[..len]) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!("Video SRTP decrypt failed: {e:#}");
                                continue;
                            }
                        },
                        None => buf[..len].to_vec().into(),
                    };
                    let Some(pkt) = RtpPacket::decode(&plain) else { continue };
                    {
                        let mut s = stats.lock().unwrap();
                        jitter.observe(&mut s, &pkt);
                        s.packets_received += 1;
                        s.bytes_received += plain.len() as u64;
                    }
                    let marker = pkt.marker;
                    frame_fragments.push(pkt.payload);
                    if marker {
                        let annex_b = reassemble_nal_units(&frame_fragments);
                        frame_fragments.clear();
                        match decoder.decode(&annex_b) {
                            Ok(Some(frame)) => *latest_decoded_frame.lock().unwrap() = Some(frame),
                            Ok(None) => {}
                            Err(e) => warn!("Video decode failed: {e:#}"),
                        }
                    }
                }
                Ok(()) = stop_rx.changed() => {
                    if *stop_rx.borrow() { break; }
                }
            }
        }
    }

    /// The most recently decoded remote video frame, if any.
    pub fn latest_decoded_frame(&self) -> Option<Yuv420Frame> {
        self.latest_decoded_frame.lock().unwrap().clone()
    }

    /// Same as `latest_decoded_frame`, but for the second remote party's
    /// video during a conference -- `None` for a non-conference call (no
    /// `second_leg` was given to `start`).
    pub fn latest_decoded_frame_leg2(&self) -> Option<Yuv420Frame> {
        self.latest_decoded_frame2.lock().unwrap().clone()
    }

    pub fn stats(&self) -> LegStats {
        self.stats.lock().unwrap().clone()
    }

    /// Whether the local camera is currently muted (send-side only --
    /// receiving/decoding/displaying the remote party's video is unaffected).
    pub fn is_muted(&self) -> bool {
        self.video_muted.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.video_muted.store(muted, Ordering::Relaxed);
    }

    /// Mirrors `MediaEngine::stop`'s abort-then-await shape (see its own
    /// doc comment for why awaiting, not just aborting, matters).
    pub async fn stop(self) {
        let _ = self.stop_tx.send(true);
        self.send_task.abort();
        self.recv_task.abort();
        let _ = self.send_task.await;
        let _ = self.recv_task.await;
        if let Some(recv_task2) = self.recv_task2 {
            recv_task2.abort();
            let _ = recv_task2.await;
        }
    }
}

/// Per-recv-task loss/jitter tracking -- mirrors `engine.rs`'s private
/// `JitterTracker` (not shared cross-module since it's ~15 lines and the
/// two engines otherwise have no reason to depend on each other).
#[derive(Default)]
struct JitterState {
    last_seq: Option<u16>,
    last_arrival: Option<Instant>,
    last_rtp_ts: Option<u32>,
}

impl JitterState {
    fn observe(&mut self, stats: &mut LegStats, pkt: &RtpPacket) {
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
            let rtp_diff_ms =
                (pkt.timestamp as i64 - prev_ts as i64).unsigned_abs() as f64 / VIDEO_CLOCK_HZ as f64 * 1000.0;
            let d = (arrival_diff_ms - rtp_diff_ms).abs();
            stats.jitter_ms += (d - stats.jitter_ms) / 16.0;
        }
        self.last_arrival = Some(now);
        self.last_rtp_ts = Some(pkt.timestamp);
    }
}

#[cfg(test)]
#[path = "../tests/unit/video_engine.rs"]
mod tests;
