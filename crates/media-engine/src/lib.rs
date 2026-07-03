pub mod aec;
pub mod audio;
pub mod codec;
pub mod dtmf;
pub mod engine;
pub mod rtp;

pub use engine::{ConferenceLeg, MediaEngine};

/// Allocate an ephemeral local RTP port (even port, per SIP convention).
pub fn alloc_rtp_port() -> u16 {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").expect("bind");
    let port = sock.local_addr().expect("local addr").port();
    port & !1 // round down to even
}
