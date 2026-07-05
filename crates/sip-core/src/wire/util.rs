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

/// Read `Expires` from a REGISTER/SUBSCRIBE response, falling back to the
/// `expires=` param on the `Contact` header (RFC 3261 §10.2.4 allows either).
pub fn extract_expires(msg: &crate::wire::message::SipMessage) -> Option<u32> {
    if let Some(v) = msg.header("Expires") {
        if let Ok(n) = v.trim().parse::<u32>() {
            return Some(n);
        }
    }
    if let Some(contact) = msg.header("Contact") {
        for param in contact.split(';') {
            if let Some(v) = param.trim().strip_prefix("expires=") {
                if let Ok(n) = v.trim_matches('"').parse::<u32>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Extract the `tag=` param from a `To`/`From` header value.
pub fn parse_tag(header: &str) -> Option<String> {
    for part in header.split(';') {
        if let Some(v) = part.trim().strip_prefix("tag=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Extract the URI from a `To`/`From`/`Contact`-style header value, preferring
/// the `<...>` angle-bracket form but falling back to a bare URI.
pub fn parse_uri(header: &str) -> Option<String> {
    if let Some(start) = header.find('<') {
        if let Some(end) = header.find('>') {
            return Some(header[start + 1..end].to_string());
        }
    }
    Some(header.split(';').next()?.trim().to_string())
}

/// Percent-encode a `Replaces` value (RFC 3891) for embedding as a URI
/// parameter. Our own generated call-ids/tags are plain hex and never
/// actually contain these characters, but this is correct regardless.
/// `%` must be encoded first to avoid double-encoding the others' output.
pub fn encode_replaces_param(s: &str) -> String {
    s.replace('%', "%25")
        .replace(';', "%3B")
        .replace('=', "%3D")
        .replace(',', "%2C")
        .replace('@', "%40")
}
