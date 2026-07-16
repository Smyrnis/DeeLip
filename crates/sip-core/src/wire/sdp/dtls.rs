//! RFC 5763/5764 DTLS-SRTP negotiation: `a=setup` (RFC 4145 §4) and
//! `a=fingerprint` (RFC 8122) SDP attributes. The actual DTLS handshake and
//! SRTP key export happen in `media-engine`; this module only covers the
//! SDP wire format used to bootstrap it.

/// Which side initiates the DTLS handshake (sends the first ClientHello).
/// `ActPass` only ever appears in an offer -- an answer always resolves to
/// `Active` or `Passive` (RFC 4145 §4, RFC 5763 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    ActPass,
    Active,
    Passive,
}

impl Setup {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Setup::ActPass => "actpass",
            Setup::Active => "active",
            Setup::Passive => "passive",
        }
    }

    pub(super) fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "actpass" => Some(Setup::ActPass),
            "active" => Some(Setup::Active),
            "passive" => Some(Setup::Passive),
            _ => None,
        }
    }
}

/// A DTLS certificate's fingerprint (RFC 8122), as carried in `a=fingerprint`.
/// `hash_func` is always `"sha-256"` here (the only algorithm this codebase
/// generates or accepts) and `hex` is colon-separated upper-hex, e.g.
/// `"AB:CD:EF:..."`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtlsFingerprint {
    pub hash_func: String,
    pub hex: String,
}

impl DtlsFingerprint {
    /// Computes the SHA-256 fingerprint of a DER-encoded X.509 certificate,
    /// formatted per RFC 8122 §5 (colon-separated upper-hex octets).
    pub fn from_cert_der(der: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(der);
        let hex = digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
        Self { hash_func: "sha-256".to_string(), hex }
    }

    pub(super) fn to_line(&self) -> String {
        format!("a=fingerprint:{} {}\r\n", self.hash_func, self.hex)
    }

    pub(super) fn parse_line(line: &str) -> Option<Self> {
        // "a=fingerprint:sha-256 AB:CD:..."
        let rest = line.trim().strip_prefix("a=fingerprint:")?;
        let mut parts = rest.splitn(2, ' ');
        let hash_func = parts.next()?.trim().to_string();
        let hex = parts.next()?.trim().to_string();
        if hash_func.is_empty() || hex.is_empty() {
            return None;
        }
        Some(Self { hash_func, hex })
    }
}

/// Generates a fresh self-signed X.509 certificate + private key for one
/// call's DTLS-SRTP session (see `MediaEncryption::DtlsSrtp` -- one
/// cert/fingerprint per call, shared across audio and video). Returns
/// `(cert_der, private_key_der, fingerprint)`. `media-engine` reconstructs a
/// `webrtc_dtls::crypto::Certificate` from the DER bytes at
/// `MediaEngine::start` (`rcgen::KeyPair: TryFrom<&[u8]>` for the private
/// key, `rustls::pki_types::CertificateDer::from` for the cert), keeping the
/// `webrtc-dtls` dependency isolated to that one crate.
pub fn generate_dtls_cert() -> anyhow::Result<(Vec<u8>, Vec<u8>, DtlsFingerprint)> {
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    let CertifiedKey { cert, key_pair } = generate_simple_self_signed(vec!["deelip".to_string()])
        .map_err(|e| anyhow::anyhow!("Generating DTLS-SRTP self-signed certificate: {e}"))?;
    let cert_der = cert.der().to_vec();
    let private_key_der = key_pair.serialize_der();
    let fingerprint = DtlsFingerprint::from_cert_der(&cert_der);
    Ok((cert_der, private_key_der, fingerprint))
}
