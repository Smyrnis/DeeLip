use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use webrtc_srtp::context::Context as SrtpContext;
use webrtc_srtp::option::srtp_replay_protection;
use webrtc_srtp::protection_profile::ProtectionProfile;
use webrtc_util::Conn;

use deelip_sip::{AudioCodec, SrtpSession};

use crate::aec::EchoCanceller;
use crate::audio::{open_streams, AudioStreams, PlaybackTx, FRAME_SAMPLES};
use crate::codec::{decode_pcma, decode_pcmu, encode_pcma, encode_pcmu, OpusDecoder, OpusEncoder};
use crate::dtmf::{build_dtmf_burst, char_to_event, DTMF_PAYLOAD_TYPE};
use crate::rtp::{RtpPacket, RtpSender};

/// Per-packet RTP timestamp increment for a 20ms frame, in units of the
/// codec's declared RTP clock rate. G.711's clock is 8000 Hz; Opus's RTP
/// clock is always 48000 Hz regardless of the audio's actual sample rate
/// (RFC 7587), even though our pipeline encodes/decodes Opus at 8 kHz.
fn ts_increment_for(codec: AudioCodec) -> u32 {
    match codec {
        AudioCodec::Opus => 960,
        AudioCodec::Pcmu | AudioCodec::Pcma => 160,
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

// ── MediaEngine ───────────────────────────────────────────────────────────────

/// Manages the audio ↔ RTP pipeline for a single active call.
pub struct MediaEngine {
    _audio:    AudioStreams,
    send_task: tokio::task::JoinHandle<()>,
    recv_task: tokio::task::JoinHandle<()>,
    stop_tx:   watch::Sender<bool>,
    dtmf_tx:   mpsc::UnboundedSender<char>,
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
    pub async fn start(
        local_rtp_port: u16,
        remote_rtp:     SocketAddr,
        codec:          AudioCodec,
        dtmf_pt:        Option<u8>,
        srtp:           Option<SrtpSession>,
        relay:          Option<Arc<dyn Conn + Send + Sync>>,
        echo_cancellation: bool,
    ) -> anyhow::Result<Self> {
        let (audio_streams, mut cap_rx, playback_tx, echo_ref) =
            open_streams(echo_cancellation).context("Opening audio streams")?;

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

        let (stop_tx, stop_rx)   = watch::channel(false);
        let mut stop_send = stop_rx.clone();
        let mut stop_recv = stop_rx;

        let (dtmf_tx, mut dtmf_rx) = mpsc::unbounded_channel::<char>();

        // ── Send task ─────────────────────────────────────────────────────────
        let send_sock    = socket.clone();
        let mut rtp_send = RtpSender::new(payload_type, ts_increment_for(codec));
        let dtmf_ssrc    = rtp_send.ssrc;
        let mut dtmf_seq = 0u16;
        let mut opus_enc = if codec == AudioCodec::Opus {
            Some(OpusEncoder::new().context("Creating Opus encoder")?)
        } else {
            None
        };
        let mut echo_canceller = echo_ref.as_ref().map(|_| EchoCanceller::new());

        let send_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(pcm) = cap_rx.recv() => {
                        let pcm = match (echo_canceller.as_mut(), echo_ref.as_ref()) {
                            (Some(canceller), Some(echo_ref)) => canceller.process(&pcm, echo_ref),
                            _ => pcm,
                        };
                        let encoded = match codec {
                            AudioCodec::Opus => opus_enc.as_mut().unwrap().encode(&pcm),
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
                        if let Err(e) = send_sock.send_to(&out, remote_rtp).await {
                            error!("RTP send: {e}");
                        }
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
                        }
                    }
                    Ok(()) = stop_send.changed() => {
                        if *stop_send.borrow() { break; }
                    }
                }
            }
            debug!("RTP send task stopped");
        });

        // ── Recv task ─────────────────────────────────────────────────────────
        let recv_sock = socket;
        let mut opus_dec = if codec == AudioCodec::Opus {
            Some(OpusDecoder::new().context("Creating Opus decoder")?)
        } else {
            None
        };

        let recv_task = tokio::spawn(async move {
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
                            let pcm = match codec {
                                AudioCodec::Opus => opus_dec.as_mut().unwrap().decode(&pkt.payload),
                                AudioCodec::Pcma => decode_pcma(&pkt.payload),
                                AudioCodec::Pcmu => decode_pcmu(&pkt.payload),
                            };
                            push_to_jitter(&playback_tx, &pcm);
                        }
                    }
                    Ok(()) = stop_recv.changed() => {
                        if *stop_recv.borrow() { break; }
                    }
                }
            }
            debug!("RTP recv task stopped");
        });

        Ok(Self { _audio: audio_streams, send_task, recv_task, stop_tx, dtmf_tx })
    }

    /// Queue a DTMF digit for immediate out-of-band RTP transmission.
    pub fn send_dtmf(&self, digit: char) {
        let _ = self.dtmf_tx.send(digit);
    }

    pub fn stop(self) {
        let _ = self.stop_tx.send(true);
        self.send_task.abort();
        self.recv_task.abort();
    }
}

fn push_to_jitter(jitter: &PlaybackTx, pcm: &[i16]) {
    let max = FRAME_SAMPLES * 50; // cap at 1 second
    let mut buf = jitter.lock().unwrap();
    for &s in pcm {
        if buf.len() < max { buf.push_back(s); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deelip_sip::SrtpParams;

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
}
