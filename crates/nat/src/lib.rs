pub mod ice;
pub mod stun;
pub mod turn_relay;

pub use ice::{IceConnection, IceGathered};
pub use stun::discover_external_addr;
pub use turn_relay::{TurnRelay, allocate_relay};

/// Allocate a local RTP port (even port, per SIP convention). `range`, if
/// given, restricts the search to `min..=max` (e.g. for a fixed firewall
/// port-forward). Known TOCTOU tradeoff -- full picture: `docs/crates/nat.md`.
pub fn alloc_rtp_port(range: Option<(u16, u16)>) -> std::io::Result<u16> {
    match range {
        Some((min, max)) => alloc_rtp_port_in_range(min, max),
        None => alloc_rtp_port_ephemeral(),
    }
}

fn alloc_rtp_port_ephemeral() -> std::io::Result<u16> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let port = sock.local_addr()?.port();
    Ok(port & !1) // round down to even
}

fn alloc_rtp_port_in_range(min: u16, max: u16) -> std::io::Result<u16> {
    let mut port = if min.is_multiple_of(2) { min } else { min.saturating_add(1) };
    while port <= max {
        if std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok() {
            return Ok(port);
        }
        match port.checked_add(2) {
            Some(next) => port = next,
            None => break,
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        format!("No free RTP port available in configured range {min}-{max}"),
    ))
}
