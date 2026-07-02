//! STUN Binding Request/Response (RFC 5389) — IPv4 only.
//! Used to discover the external IP:port through a NAT device.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context};
use tokio::net::UdpSocket;
use tokio::time::{timeout, Duration};

const MAGIC_COOKIE:       u32 = 0x2112_A442;
const BINDING_REQUEST:    u16 = 0x0001;
const BINDING_SUCCESS:    u16 = 0x0101;
const ATTR_MAPPED_ADDR:   u16 = 0x0001;
const ATTR_XOR_MAPPED:    u16 = 0x0020;

/// Send a STUN Binding Request to `stun_server` (e.g. `"stun.l.google.com:19302"`)
/// and return the external `SocketAddr` as seen by the STUN server.
pub async fn discover_external_addr(stun_server: &str) -> anyhow::Result<SocketAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("Bind STUN socket")?;

    let txn_id = random_txn_id();

    // 20-byte Binding Request with no attributes
    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    req[2..4].copy_from_slice(&0u16.to_be_bytes()); // message length = 0
    req[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    req[8..20].copy_from_slice(&txn_id);

    socket
        .send_to(&req, stun_server)
        .await
        .context("Sending STUN Binding Request")?;

    let mut buf = [0u8; 512];
    let (n, _from) = timeout(Duration::from_secs(5), socket.recv_from(&mut buf))
        .await
        .context("STUN response timeout")?
        .context("STUN recv error")?;

    parse_binding_response(&buf[..n])
}

fn parse_binding_response(data: &[u8]) -> anyhow::Result<SocketAddr> {
    if data.len() < 20 {
        bail!("STUN response too short ({} bytes)", data.len());
    }

    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != BINDING_SUCCESS {
        bail!("Expected Binding Success (0x{BINDING_SUCCESS:04x}), got 0x{msg_type:04x}");
    }

    let mut xor_mapped: Option<SocketAddr> = None;
    let mut mapped:     Option<SocketAddr> = None;
    let mut offset = 20usize;

    while offset + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[offset],     data[offset + 1]]);
        let attr_len  = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > data.len() { break; }

        match attr_type {
            ATTR_XOR_MAPPED if attr_len >= 8 && data[offset + 1] == 0x01 => {
                // XOR port with upper 16 bits of magic cookie
                let port = u16::from_be_bytes([data[offset + 2], data[offset + 3]])
                    ^ ((MAGIC_COOKIE >> 16) as u16);
                let ip = Ipv4Addr::new(
                    data[offset + 4] ^ ((MAGIC_COOKIE >> 24) as u8),
                    data[offset + 5] ^ ((MAGIC_COOKIE >> 16) as u8),
                    data[offset + 6] ^ ((MAGIC_COOKIE >>  8) as u8),
                    data[offset + 7] ^   MAGIC_COOKIE         as u8,
                );
                xor_mapped = Some(SocketAddr::new(IpAddr::V4(ip), port));
            }
            ATTR_MAPPED_ADDR if attr_len >= 8 && data[offset + 1] == 0x01 => {
                let port = u16::from_be_bytes([data[offset + 2], data[offset + 3]]);
                let ip   = Ipv4Addr::new(
                    data[offset + 4], data[offset + 5],
                    data[offset + 6], data[offset + 7],
                );
                mapped = Some(SocketAddr::new(IpAddr::V4(ip), port));
            }
            _ => {}
        }

        // Attributes are padded to 4-byte boundaries
        offset += (attr_len + 3) & !3;
    }

    xor_mapped
        .or(mapped)
        .ok_or_else(|| anyhow::anyhow!("No MAPPED-ADDRESS found in STUN response"))
}

fn random_txn_id() -> [u8; 12] {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    let mix = t ^ c.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut id = [0u8; 12];
    id[0..8].copy_from_slice(&mix.to_be_bytes());
    id[8..12].copy_from_slice(&(mix as u32 ^ (mix >> 32) as u32).to_be_bytes());
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_xor_mapped_address() {
        // Craft a minimal Binding Response with XOR-MAPPED-ADDRESS for 203.0.113.1:12345
        let port_raw = 12345u16 ^ ((MAGIC_COOKIE >> 16) as u16);
        let ip0 = 203u8 ^ ((MAGIC_COOKIE >> 24) as u8);
        let ip1 = 0u8  ^ ((MAGIC_COOKIE >> 16) as u8);
        let ip2 = 113u8 ^ ((MAGIC_COOKIE >>  8) as u8);
        let ip3 = 1u8  ^   MAGIC_COOKIE as u8;

        let mut buf = vec![0u8; 32];
        buf[0..2].copy_from_slice(&BINDING_SUCCESS.to_be_bytes());
        buf[2..4].copy_from_slice(&12u16.to_be_bytes()); // attr length total
        buf[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        // attribute: XOR-MAPPED-ADDRESS
        buf[20..22].copy_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
        buf[22..24].copy_from_slice(&8u16.to_be_bytes());
        buf[24] = 0x00;
        buf[25] = 0x01; // family = IPv4
        buf[26..28].copy_from_slice(&port_raw.to_be_bytes());
        buf[28] = ip0; buf[29] = ip1; buf[30] = ip2; buf[31] = ip3;

        let addr = parse_binding_response(&buf).unwrap();
        assert_eq!(addr.port(), 12345);
        assert_eq!(addr.ip().to_string(), "203.0.113.1");
    }
}
