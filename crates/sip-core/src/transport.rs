use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf, split};
use tokio::net::{TcpSocket, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tracing::{debug, warn};

use deelip_config::TransportProtocol;

use crate::wire::framing::MessageFramer;

/// Bounds every TCP connect / TLS handshake below -- matches STUN's existing
/// 5s timeout (crates/nat/src/stun.rs). Without this, a firewalled or
/// silently-dropping peer can hang the caller indefinitely; these calls sit
/// on main()'s startup path (via SipStack::new) before the app window exists.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Wraps `fut` in `CONNECT_TIMEOUT`, distinguishing the timeout itself from
/// a normal `io::Error` in the `.context(...)` message so a hang and a real
/// connect failure still read differently in logs. Shared by every
/// connect-shaped call in this file (plain TCP connect, TLS's TCP connect,
/// TLS handshake) -- previously each hand-wrapped the same
/// `timeout(...).await.context(...)?.context(...)?` shape independently.
async fn connect_with_timeout<T>(
    fut: impl std::future::Future<Output = std::io::Result<T>>, what: &str,
) -> anyhow::Result<T> {
    timeout(CONNECT_TIMEOUT, fut).await.with_context(|| format!("{what} timed out"))?.with_context(|| what.to_string())
}

/// Unifies UDP (datagram), plain TCP, and TLS (both persistent streams) SIP
/// transports behind one API.
pub enum SipTransport {
    Udp(UdpSocket),
    Tcp(TcpConn),
    Tls(TlsConn),
}

impl SipTransport {
    pub async fn connect(
        proto: TransportProtocol, bind_addr: SocketAddr, server_addr: SocketAddr, server_name: &str,
        insecure_skip_verify: bool,
    ) -> anyhow::Result<Self> {
        match proto {
            TransportProtocol::Tls => {
                let conn = TlsConn::connect(bind_addr, server_addr, server_name, insecure_skip_verify)
                    .await
                    .context("Connecting SIP-over-TLS transport")?;
                Ok(Self::Tls(conn))
            }
            TransportProtocol::Tcp => {
                let conn =
                    TcpConn::connect(bind_addr, server_addr).await.context("Connecting SIP-over-TCP transport")?;
                Ok(Self::Tcp(conn))
            }
            TransportProtocol::Udp => {
                let socket = UdpSocket::bind(bind_addr).await?;
                debug!("SIP transport (UDP) bound to {}", socket.local_addr()?);
                Ok(Self::Udp(socket))
            }
            TransportProtocol::Auto => {
                unreachable!("Auto must be resolved to a concrete transport before SipTransport::connect")
            }
        }
    }

    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        match self {
            Self::Udp(s) => Ok(s.local_addr()?),
            Self::Tcp(t) => Ok(t.halves.local_addr),
            Self::Tls(t) => Ok(t.halves.local_addr),
        }
    }

    /// Send a message. For the `Tcp`/`Tls` variants `to` is ignored — all
    /// traffic funnels through the one persistent connection to the server.
    pub async fn send(&self, data: &[u8], to: SocketAddr) -> anyhow::Result<()> {
        match self {
            Self::Udp(s) => {
                s.send_to(data, to).await?;
                Ok(())
            }
            Self::Tcp(t) => t.halves.send(data).await,
            Self::Tls(t) => t.halves.send(data).await,
        }
    }

    /// Receive one complete SIP message; for `Tcp`/`Tls` the returned
    /// address is always the server's address (neither has a
    /// per-datagram sender).
    pub async fn recv(&self) -> anyhow::Result<(Vec<u8>, SocketAddr)> {
        match self {
            Self::Udp(s) => {
                let mut buf = vec![0u8; 65_535];
                let (len, from) = s.recv_from(&mut buf).await?;
                buf.truncate(len);
                Ok((buf, from))
            }
            Self::Tcp(t) => t.halves.recv(t.server_addr).await,
            Self::Tls(t) => t.halves.recv(t.server_addr).await,
        }
    }
}

// ── Shared stream (TCP/TLS) send/recv/framing ────────────────────────────────
// Both plain TCP and TLS-over-TCP are a persistent byte stream needing the
// same split-read/write-plus-framer plumbing -- only how the stream itself
// gets established differs (a bare `TcpStream` vs. a TLS handshake wrapping
// one), so that plumbing lives here once instead of duplicated per variant.

struct StreamHalves<S> {
    local_addr: SocketAddr,
    write: Mutex<WriteHalf<S>>,
    read: Mutex<(ReadHalf<S>, MessageFramer)>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> StreamHalves<S> {
    fn new(stream: S, local_addr: SocketAddr) -> Self {
        let (read_half, write_half) = split(stream);
        Self { local_addr, write: Mutex::new(write_half), read: Mutex::new((read_half, MessageFramer::new())) }
    }

    async fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        let mut w = self.write.lock().await;
        w.write_all(data).await.context("Stream write")
    }

    async fn recv(&self, server_addr: SocketAddr) -> anyhow::Result<(Vec<u8>, SocketAddr)> {
        let mut guard = self.read.lock().await;
        let (read_half, framer) = &mut *guard;
        loop {
            if let Some(msg) = framer.try_take_message() {
                return Ok((msg, server_addr));
            }
            let mut chunk = [0u8; 4096];
            let n = read_half.read(&mut chunk).await.context("Stream read")?;
            if n == 0 {
                anyhow::bail!("Connection closed by peer");
            }
            framer.push(&chunk[..n]);
        }
    }
}

// ── Plain TCP transport ───────────────────────────────────────────────────────

pub struct TcpConn {
    server_addr: SocketAddr,
    halves: StreamHalves<TcpStream>,
}

impl TcpConn {
    async fn connect(bind_addr: SocketAddr, server_addr: SocketAddr) -> anyhow::Result<Self> {
        let socket = if bind_addr.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
        socket.set_reuseaddr(true)?;
        socket.bind(bind_addr).context("Binding TCP socket")?;
        let stream = connect_with_timeout(socket.connect(server_addr), "Connecting TCP").await?;
        let local_addr = stream.local_addr()?;
        debug!("SIP transport (TCP) connected to {server_addr} (local {local_addr})");

        Ok(Self { server_addr, halves: StreamHalves::new(stream, local_addr) })
    }
}

// ── TLS transport ────────────────────────────────────────────────────────────

pub struct TlsConn {
    server_addr: SocketAddr,
    halves: StreamHalves<TlsStream<TcpStream>>,
}

impl TlsConn {
    async fn connect(
        bind_addr: SocketAddr, server_addr: SocketAddr, server_name: &str, insecure_skip_verify: bool,
    ) -> anyhow::Result<Self> {
        // rustls 0.23 requires an explicit process-wide crypto provider; idempotent.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let config = if insecure_skip_verify {
            warn!(
                "TLS certificate verification DISABLED for {server_name} \
                 (tls_insecure_skip_verify=true) — connection is encrypted but NOT authenticated"
            );
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoCertVerification::new()))
                .with_no_client_auth()
        } else {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(cert);
            }
            rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()
        };

        let connector = TlsConnector::from(Arc::new(config));
        let name =
            rustls::pki_types::ServerName::try_from(server_name.to_string()).context("Invalid TLS server name")?;

        let socket = if bind_addr.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
        socket.set_reuseaddr(true)?;
        socket.bind(bind_addr).context("Binding TCP socket")?;
        let tcp = connect_with_timeout(socket.connect(server_addr), "Connecting TCP").await?;
        let local_addr = tcp.local_addr()?;

        let tls_stream = connect_with_timeout(connector.connect(name, tcp), "TLS handshake").await?;
        debug!("SIP transport (TLS) connected to {server_addr} (local {local_addr})");

        Ok(Self { server_addr, halves: StreamHalves::new(tls_stream, local_addr) })
    }
}

/// Accepts any server certificate. Only used when `tls_insecure_skip_verify` is
/// set. Still delegates signature verification to rustls's own webpki-backed
/// helpers — skipping cert-chain trust is the intended relaxation here, but
/// no-op'ing signature checks too would be a strictly bigger hole.
#[derive(Debug)]
struct NoCertVerification {
    supported_algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl NoCertVerification {
    fn new() -> Self {
        Self { supported_algs: rustls::crypto::ring::default_provider().signature_verification_algorithms }
    }
}

impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self, _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>], _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8], _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self, message: &[u8], cert: &rustls::pki_types::CertificateDer<'_>, dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.supported_algs)
    }

    fn verify_tls13_signature(
        &self, message: &[u8], cert: &rustls::pki_types::CertificateDer<'_>, dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.supported_algs)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}
