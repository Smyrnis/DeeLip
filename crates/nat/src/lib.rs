pub mod ice;
pub mod stun;
pub mod turn_relay;

pub use ice::{IceConnection, IceGathered};
pub use stun::discover_external_addr;
pub use turn_relay::{allocate_relay, TurnRelay};

/// Allocate an ephemeral local RTP port (even port, per SIP convention).
pub fn alloc_rtp_port() -> std::io::Result<u16> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let port = sock.local_addr()?.port();
    Ok(port & !1) // round down to even
}
