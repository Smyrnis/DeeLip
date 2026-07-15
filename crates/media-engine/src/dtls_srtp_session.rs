//! RFC 5763/5764 DTLS-SRTP: runs the DTLS 1.2 handshake over the shared RTP
//! socket (via `dtls_demux::DemuxConn`) using the `webrtc-dtls` crate, then
//! exports SRTP keying material once the handshake completes. Structurally
//! the media-engine-side counterpart of `zrtp_session.rs`, but shaped very
//! differently: `webrtc_dtls::conn::DTLSConn` owns its `Conn` and runs its
//! own internal read loop, rather than exposing a `handle_incoming`-style
//! byte-in/event-out interface the way `ZrtpEngine` does -- see
//! `dtls_demux::DemuxConn`'s doc comment.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use webrtc_dtls::config::Config;
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::{Certificate, CryptoPrivateKey};
use webrtc_dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use webrtc_util_dtls::KeyingMaterialExporter;

use deelip_sip::DtlsFingerprint;

use crate::dtls_demux::DemuxConn;

const SRTP_MASTER_KEY_LEN: usize = 16;
const SRTP_MASTER_SALT_LEN: usize = 14;
/// RFC 5764 §4.2's fixed exporter label.
const EXPORTER_LABEL: &str = "EXTRACTOR-dtls_srtp";

/// Everything one call's DTLS-SRTP handshake needs -- see
/// `deelip_sip::call::media_setup::DtlsCallParams`, whose fields this
/// mirrors (that type stays sip-core-side/`webrtc-dtls`-free; this is
/// where its bytes actually get turned into a live handshake).
pub struct DtlsSrtpParams {
    pub cert_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
    /// `true`: we send the DTLS ClientHello (RFC 4145's `a=setup:active`).
    /// `false`: we wait for the peer's (`a=setup:passive`).
    pub is_client: bool,
    /// Compared against the peer's actual certificate once the handshake
    /// completes (`State::peer_certificates`) -- the real MITM-prevention
    /// step, since SDP's `a=fingerprint` is otherwise just an unauthenticated
    /// claim. See `DtlsSrtpOutcome::FingerprintMismatch`.
    pub expected_remote_fingerprint: DtlsFingerprint,
}

/// Result of one call's DTLS-SRTP handshake, sent back to `engine.rs`'s
/// `recv_loop` over a channel once `run_dtls_handshake`'s background task
/// finishes (there is exactly one outcome per call, unlike ZRTP's ongoing
/// `Vec<ZrtpOutcome>` stream -- the DTLS session isn't kept alive/pumped
/// after key export, matching this feature's disclosed scope).
pub enum DtlsSrtpOutcome {
    Secure {
        local_key: [u8; SRTP_MASTER_KEY_LEN],
        local_salt: [u8; SRTP_MASTER_SALT_LEN],
        remote_key: [u8; SRTP_MASTER_KEY_LEN],
        remote_salt: [u8; SRTP_MASTER_SALT_LEN],
    },
    /// The peer's actual certificate didn't match what SDP advertised --
    /// an active-attack indicator, not a negotiation failure. Callers must
    /// NOT fall back to plaintext RTP on this outcome (unlike `Failed`).
    FingerprintMismatch,
    Failed(String),
}

fn sha256_fingerprint_hex(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Runs the handshake to completion and sends exactly one outcome over
/// `outcome_tx`. Spawned on its own task by `engine.rs::MediaEngine::start`,
/// concurrently with (not before) `recv_loop` -- unlike ZRTP's initial Hello,
/// this can't run before `recv_loop` starts, since `recv_loop` is what feeds
/// `demux` its inbound bytes in the first place.
pub(crate) async fn run_dtls_handshake(
    params: DtlsSrtpParams, demux: DemuxConn, outcome_tx: tokio::sync::mpsc::UnboundedSender<DtlsSrtpOutcome>,
) {
    let outcome = run(params, demux).await;
    let _ = outcome_tx.send(outcome);
}

async fn run(params: DtlsSrtpParams, demux: DemuxConn) -> DtlsSrtpOutcome {
    let key_pair = match rcgen::KeyPair::try_from(params.private_key_der.as_slice()) {
        Ok(kp) => kp,
        Err(e) => return DtlsSrtpOutcome::Failed(format!("Parsing DTLS-SRTP private key: {e}")),
    };
    let private_key = match CryptoPrivateKey::try_from(&key_pair) {
        Ok(k) => k,
        Err(e) => return DtlsSrtpOutcome::Failed(format!("Converting DTLS-SRTP private key: {e}")),
    };
    let certificate =
        Certificate { certificate: vec![rustls::pki_types::CertificateDer::from(params.cert_der)], private_key };

    let config = Config {
        certificates: vec![certificate],
        srtp_protection_profiles: vec![SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80],
        // Self-signed certs authenticated out-of-band via the SDP
        // `a=fingerprint`, not a CA chain -- this is the standard
        // WebRTC-style DTLS-SRTP trust model. `insecure_skip_verify` lets
        // the handshake itself complete regardless of cert validity; the
        // real MITM-prevention check happens below, comparing the peer's
        // actual certificate against what SDP advertised.
        insecure_skip_verify: true,
        // Both peers present a self-signed cert and both need the other's
        // for fingerprint verification -- this is mutual auth, not the
        // ordinary one-way "client verifies server" TLS relationship.
        // Without this (only meaningful for the server/`is_client: false`
        // side), the server never requests/receives the client's
        // certificate at all, and `state.peer_certificates` comes back
        // empty -- confirmed by this module's own two-socket test failing
        // with exactly that symptom before this was added.
        client_auth: webrtc_dtls::config::ClientAuthType::RequireAnyClientCert,
        ..Default::default()
    };

    let conn: Arc<dyn webrtc_util_dtls::Conn + Send + Sync> = Arc::new(demux);
    let dtls_conn = match DTLSConn::new(conn, config, params.is_client, None).await {
        Ok(c) => c,
        Err(e) => return DtlsSrtpOutcome::Failed(format!("DTLS-SRTP handshake failed: {e}")),
    };

    let state = dtls_conn.connection_state().await;
    let Some(peer_cert_der) = state.peer_certificates.first() else {
        return DtlsSrtpOutcome::Failed("DTLS-SRTP handshake completed with no peer certificate".to_string());
    };
    let actual_fingerprint = sha256_fingerprint_hex(peer_cert_der);
    if actual_fingerprint != params.expected_remote_fingerprint.hex {
        return DtlsSrtpOutcome::FingerprintMismatch;
    }

    let export_len = 2 * (SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN);
    let keying_material = match state.export_keying_material(EXPORTER_LABEL, &[], export_len).await {
        Ok(km) => km,
        Err(e) => return DtlsSrtpOutcome::Failed(format!("Exporting SRTP keying material: {e}")),
    };
    // RFC 5764 §4.2: client_write_key | server_write_key | client_write_salt
    // | server_write_salt -- NOT interleaved key+salt pairs.
    let client_key: [u8; SRTP_MASTER_KEY_LEN] = keying_material[0..16].try_into().unwrap();
    let server_key: [u8; SRTP_MASTER_KEY_LEN] = keying_material[16..32].try_into().unwrap();
    let client_salt: [u8; SRTP_MASTER_SALT_LEN] = keying_material[32..46].try_into().unwrap();
    let server_salt: [u8; SRTP_MASTER_SALT_LEN] = keying_material[46..60].try_into().unwrap();

    let (local_key, local_salt, remote_key, remote_salt) = if params.is_client {
        (client_key, client_salt, server_key, server_salt)
    } else {
        (server_key, server_salt, client_key, client_salt)
    };

    DtlsSrtpOutcome::Secure { local_key, local_salt, remote_key, remote_salt }
}

#[cfg(test)]
#[path = "../tests/unit/dtls_srtp_session.rs"]
mod tests;
