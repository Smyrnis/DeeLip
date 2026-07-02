use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    // XOR upper bits with counter to keep values distinct even if called rapidly
    ts ^ (seq.wrapping_shl(20))
}

pub fn new_call_id(host: &str) -> String {
    format!("{:016x}@{host}", unique_id())
}

pub fn new_branch() -> String {
    // RFC 3261 magic cookie prefix is mandatory
    format!("z9hG4bK{:016x}", unique_id())
}

pub fn new_tag() -> String {
    format!("{:016x}", unique_id())
}

/// Determine the local IP that would be used to reach `server:port`.
/// Uses a connected UDP socket — no packets are actually sent.
pub fn local_ip_for(server: &str, port: u16) -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.connect(format!("{server}:{port}"))?;
    Ok(sock.local_addr()?.ip().to_string())
}
