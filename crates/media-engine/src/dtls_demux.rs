//! Demultiplexes DTLS records off the shared RTP socket for
//! `webrtc_dtls::conn::DTLSConn` (a `webrtc-util 0.11` `Conn` impl). See
//! docs/crates/media-engine.md's "DTLS-SRTP session driving" section for the
//! full rationale, including why this needs a third, isolated `webrtc-util`
//! version alongside the crate's other two.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};
use webrtc_util_dtls::{Conn, Error as UtilError, Result as UtilResult};

use crate::engine::RtpSocket;

/// First byte of the packet the ClassificationValue would be checked
/// against per RFC 5764 §5.1.2: 20-63 is a DTLS record.
pub(crate) fn is_dtls_packet(buf: &[u8]) -> bool {
    matches!(buf.first(), Some(20..=63))
}

pub(crate) struct DemuxConn {
    sock: Arc<RtpSocket>,
    remote: SocketAddr,
    inbound: Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
}

impl DemuxConn {
    /// Returns the `Conn` to hand to `DTLSConn::new`, plus the sender
    /// `recv_loop` uses to forward demuxed DTLS bytes into it.
    pub(crate) fn new(sock: Arc<RtpSocket>, remote: SocketAddr) -> (Self, mpsc::UnboundedSender<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { sock, remote, inbound: Mutex::new(rx) }, tx)
    }
}

#[async_trait]
impl Conn for DemuxConn {
    async fn connect(&self, _addr: SocketAddr) -> UtilResult<()> {
        // No real connect step -- `remote` is already fixed at construction
        // (this codebase never uses ICE alongside DTLS-SRTP, see
        // ROADMAP.md's resolved design question), matching how SDES-SRTP
        // and ZRTP both already work against a fixed `remote_rtp`.
        Ok(())
    }

    async fn recv(&self, buf: &mut [u8]) -> UtilResult<usize> {
        let (n, _) = self.recv_from(buf).await?;
        Ok(n)
    }

    async fn recv_from(&self, buf: &mut [u8]) -> UtilResult<(usize, SocketAddr)> {
        let mut inbound = self.inbound.lock().await;
        match inbound.recv().await {
            Some(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                Ok((n, self.remote))
            }
            None => Err(UtilError::Other("DTLS demux channel closed".to_string())),
        }
    }

    async fn send(&self, buf: &[u8]) -> UtilResult<usize> {
        self.send_to(buf, self.remote).await
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> UtilResult<usize> {
        self.sock.send_to(buf, target).await.map_err(|e| UtilError::Other(e.to_string()))?;
        Ok(buf.len())
    }

    fn local_addr(&self) -> UtilResult<SocketAddr> {
        Err(UtilError::Other("DemuxConn has no distinct local address -- it shares engine.rs's RtpSocket".to_string()))
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }

    async fn close(&self) -> UtilResult<()> {
        // The shared `RtpSocket` outlives this `DemuxConn` (owned by
        // `MediaEngine` itself, not by the DTLS handshake task) -- nothing
        // to actually close here.
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

#[cfg(test)]
#[path = "../tests/unit/dtls_demux.rs"]
mod tests;
