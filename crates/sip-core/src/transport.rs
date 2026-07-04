use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use tokio::io::{split, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpSocket, UdpSocket};
use tokio::sync::Mutex;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, warn};

use deelip_config::TransportProtocol;

use crate::wire::framing::MessageFramer;

/// Unifies UDP (datagram) and TLS (persistent stream) SIP transports behind one API.
pub enum SipTransport {
    Udp(UdpSocket),
    Tls(TlsConn),
}

impl SipTransport {
    pub async fn connect(
        proto: TransportProtocol,
        bind_addr: SocketAddr,
        server_addr: SocketAddr,
        server_name: &str,
        insecure_skip_verify: bool,
    ) -> anyhow::Result<Self> {
        match proto {
            TransportProtocol::Tls => {
                let conn = TlsConn::connect(bind_addr, server_addr, server_name, insecure_skip_verify)
                    .await
                    .context("Connecting SIP-over-TLS transport")?;
                Ok(Self::Tls(conn))
            }
            // Plain TCP (no TLS) is not implemented; fall back to UDP semantics.
            TransportProtocol::Udp | TransportProtocol::Tcp => {
                let socket = UdpSocket::bind(bind_addr).await?;
                debug!("SIP transport (UDP) bound to {}", socket.local_addr()?);
                Ok(Self::Udp(socket))
            }
        }
    }

    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        match self {
            Self::Udp(s) => Ok(s.local_addr()?),
            Self::Tls(t) => Ok(t.local_addr),
        }
    }

    /// Send a message. For the `Tls` variant `to` is ignored — all traffic
    /// funnels through the one persistent connection to the server.
    pub async fn send(&self, data: &[u8], to: SocketAddr) -> anyhow::Result<()> {
        match self {
            Self::Udp(s) => { s.send_to(data, to).await?; Ok(()) }
            Self::Tls(t) => t.send(data).await,
        }
    }

    /// Receive one complete SIP message; for `Tls` the returned address is
    /// always the server's address (TLS has no per-datagram sender).
    pub async fn recv(&self) -> anyhow::Result<(Vec<u8>, SocketAddr)> {
        match self {
            Self::Udp(s) => {
                let mut buf = vec![0u8; 65_535];
                let (len, from) = s.recv_from(&mut buf).await?;
                buf.truncate(len);
                Ok((buf, from))
            }
            Self::Tls(t) => t.recv().await,
        }
    }
}

// ── TLS transport ────────────────────────────────────────────────────────────

pub struct TlsConn {
    server_addr: SocketAddr,
    local_addr:  SocketAddr,
    write: Mutex<WriteHalf<TlsStream<tokio::net::TcpStream>>>,
    read:  Mutex<(ReadHalf<TlsStream<tokio::net::TcpStream>>, MessageFramer)>,
}

impl TlsConn {
    async fn connect(
        bind_addr:            SocketAddr,
        server_addr:          SocketAddr,
        server_name:          &str,
        insecure_skip_verify: bool,
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
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        };

        let connector = TlsConnector::from(Arc::new(config));
        let name = rustls::pki_types::ServerName::try_from(server_name.to_string())
            .context("Invalid TLS server name")?;

        let socket = if bind_addr.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
        socket.set_reuseaddr(true)?;
        socket.bind(bind_addr).context("Binding TCP socket")?;
        let tcp = socket.connect(server_addr).await.context("Connecting TCP")?;
        let local_addr = tcp.local_addr()?;

        let tls_stream = connector.connect(name, tcp).await.context("TLS handshake")?;
        debug!("SIP transport (TLS) connected to {server_addr} (local {local_addr})");

        let (read_half, write_half) = split(tls_stream);
        Ok(Self {
            server_addr,
            local_addr,
            write: Mutex::new(write_half),
            read:  Mutex::new((read_half, MessageFramer::new())),
        })
    }

    async fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        let mut w = self.write.lock().await;
        w.write_all(data).await.context("TLS write")
    }

    async fn recv(&self) -> anyhow::Result<(Vec<u8>, SocketAddr)> {
        let mut guard = self.read.lock().await;
        let (read_half, framer) = &mut *guard;
        loop {
            if let Some(msg) = framer.try_take_message() {
                return Ok((msg, self.server_addr));
            }
            let mut chunk = [0u8; 4096];
            let n = read_half.read(&mut chunk).await.context("TLS read")?;
            if n == 0 {
                anyhow::bail!("TLS connection closed by peer");
            }
            framer.push(&chunk[..n]);
        }
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
        Self {
            supported_algs: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.supported_algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.supported_algs)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}
