//! Real two-socket integration coverage for DTLS-SRTP -- unlike
//! `zrtp_session.rs`'s tests (pure state-machine, no real sockets), this
//! genuinely runs two `DTLSConn`s against each other over real localhost
//! UDP sockets, since `webrtc_dtls::conn::DTLSConn` owns its own I/O rather
//! than exposing a byte-in/event-out interface the way `ZrtpEngine` does
//! (see `dtls_demux::DemuxConn`'s doc comment) -- there's no smaller unit to
//! test in isolation.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use deelip_sip::DtlsFingerprint;

use super::*;
use crate::dtls_demux::DemuxConn;
use crate::engine::RtpSocket;

async fn bind_pair() -> (Arc<RtpSocket>, SocketAddr, Arc<RtpSocket>, SocketAddr) {
    let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let a_addr = a.local_addr().unwrap();
    let b_addr = b.local_addr().unwrap();
    (Arc::new(RtpSocket::Direct(a)), b_addr, Arc::new(RtpSocket::Direct(b)), a_addr)
}

/// Feeds every packet received on `sock` into `inbound_tx` -- a minimal
/// stand-in for `engine.rs::recv_loop`'s real RTP/ZRTP-vs-DTLS demux
/// dispatch, since this test's whole point is exercising `DemuxConn` +
/// `DTLSConn` against a real socket pair, not `recv_loop`'s classification
/// logic (every packet on these sockets is a DTLS record anyway, nothing
/// else is ever sent to them in this test).
fn spawn_pump(sock: Arc<RtpSocket>, inbound_tx: mpsc::UnboundedSender<Vec<u8>>) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((n, _)) = sock.recv_from(&mut buf).await {
            if inbound_tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
}

fn gen_cert() -> (Vec<u8>, Vec<u8>, DtlsFingerprint) {
    deelip_sip::generate_dtls_cert().expect("generating a test DTLS-SRTP certificate")
}

async fn recv_outcome(rx: &mut mpsc::UnboundedReceiver<DtlsSrtpOutcome>) -> DtlsSrtpOutcome {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("DTLS-SRTP handshake timed out")
        .expect("outcome channel closed with no outcome")
}

#[tokio::test]
async fn full_handshake_derives_matching_srtp_keys() {
    let (sock_a, addr_b, sock_b, addr_a) = bind_pair().await;

    let (cert_a, key_a, fp_a) = gen_cert();
    let (cert_b, key_b, fp_b) = gen_cert();

    let (demux_a, inbound_a) = DemuxConn::new(sock_a.clone(), addr_b);
    let (demux_b, inbound_b) = DemuxConn::new(sock_b.clone(), addr_a);
    spawn_pump(sock_a, inbound_a);
    spawn_pump(sock_b, inbound_b);

    let (outcome_tx_a, mut outcome_rx_a) = mpsc::unbounded_channel();
    let (outcome_tx_b, mut outcome_rx_b) = mpsc::unbounded_channel();

    let params_a =
        DtlsSrtpParams { cert_der: cert_a, private_key_der: key_a, is_client: true, expected_remote_fingerprint: fp_b };
    let params_b = DtlsSrtpParams {
        cert_der: cert_b,
        private_key_der: key_b,
        is_client: false,
        expected_remote_fingerprint: fp_a,
    };

    tokio::spawn(run_dtls_handshake(params_a, demux_a, outcome_tx_a));
    tokio::spawn(run_dtls_handshake(params_b, demux_b, outcome_tx_b));

    let outcome_a = recv_outcome(&mut outcome_rx_a).await;
    let outcome_b = recv_outcome(&mut outcome_rx_b).await;

    let (a_local_key, a_local_salt, a_remote_key, a_remote_salt) = match outcome_a {
        DtlsSrtpOutcome::Secure { local_key, local_salt, remote_key, remote_salt } => {
            (local_key, local_salt, remote_key, remote_salt)
        }
        DtlsSrtpOutcome::FingerprintMismatch => panic!("A: unexpected fingerprint mismatch"),
        DtlsSrtpOutcome::Failed(reason) => panic!("A: handshake failed: {reason}"),
    };
    let (b_local_key, b_local_salt, b_remote_key, b_remote_salt) = match outcome_b {
        DtlsSrtpOutcome::Secure { local_key, local_salt, remote_key, remote_salt } => {
            (local_key, local_salt, remote_key, remote_salt)
        }
        DtlsSrtpOutcome::FingerprintMismatch => panic!("B: unexpected fingerprint mismatch"),
        DtlsSrtpOutcome::Failed(reason) => panic!("B: handshake failed: {reason}"),
    };

    // A encrypts with its own "local" key; B must decrypt with the exact
    // same bytes as its own "remote" key, and vice versa -- the real proof
    // that both sides derived usable, matching SRTP keying material.
    assert_eq!(a_local_key, b_remote_key);
    assert_eq!(a_local_salt, b_remote_salt);
    assert_eq!(a_remote_key, b_local_key);
    assert_eq!(a_remote_salt, b_local_salt);
}

#[tokio::test]
async fn fingerprint_mismatch_is_detected_not_silently_accepted() {
    let (sock_a, addr_b, sock_b, addr_a) = bind_pair().await;

    let (cert_a, key_a, fp_a) = gen_cert();
    let (cert_b, key_b, _fp_b) = gen_cert();
    let (_, _, wrong_fp) = gen_cert(); // a third, unrelated cert's fingerprint

    let (demux_a, inbound_a) = DemuxConn::new(sock_a.clone(), addr_b);
    let (demux_b, inbound_b) = DemuxConn::new(sock_b.clone(), addr_a);
    spawn_pump(sock_a, inbound_a);
    spawn_pump(sock_b, inbound_b);

    let (outcome_tx_a, mut outcome_rx_a) = mpsc::unbounded_channel();
    let (outcome_tx_b, mut outcome_rx_b) = mpsc::unbounded_channel();

    // A deliberately expects the wrong fingerprint for B -- simulating an
    // SDP whose advertised fingerprint doesn't match the peer's actual
    // certificate (the exact scenario `handle_dtls_outcome` must never
    // silently fall back to plaintext for).
    let params_a = DtlsSrtpParams {
        cert_der: cert_a,
        private_key_der: key_a,
        is_client: true,
        expected_remote_fingerprint: wrong_fp,
    };
    let params_b = DtlsSrtpParams {
        cert_der: cert_b,
        private_key_der: key_b,
        is_client: false,
        expected_remote_fingerprint: fp_a,
    };

    tokio::spawn(run_dtls_handshake(params_a, demux_a, outcome_tx_a));
    tokio::spawn(run_dtls_handshake(params_b, demux_b, outcome_tx_b));

    let outcome_a = recv_outcome(&mut outcome_rx_a).await;
    let _outcome_b = recv_outcome(&mut outcome_rx_b).await; // B's fingerprint check passes; just drain it

    assert!(
        matches!(outcome_a, DtlsSrtpOutcome::FingerprintMismatch),
        "expected FingerprintMismatch, the handshake must not silently succeed with a wrong expected fingerprint"
    );
}
