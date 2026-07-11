use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64;
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

/// Extract the `received=`/`rport=` params from a response's own `Via:`
/// header -- what the server actually saw as our source IP/port, distinct
/// from the `local_ip:local_port` we sent it in that same header. Used by
/// `SipAccount::allow_ip_rewrite` to self-discover a public address from
/// the registrar's own feedback, without a separate STUN server.
pub fn parse_via_received(via: &str) -> (Option<String>, Option<u16>) {
    let mut received = None;
    let mut rport = None;
    for part in via.split(';').skip(1) {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("received=") {
            received = Some(v.trim().to_string());
        } else if let Some(v) = part.strip_prefix("rport=") {
            rport = v.trim().parse().ok();
        }
    }
    (received, rport)
}

/// Parse a `Session-Expires:` header (RFC 4028) into its interval (seconds)
/// and optional `refresher=` param (`"uac"`/`"uas"`, lowercased) --
/// used to negotiate `SipAccount::session_timers_enabled`'s periodic
/// re-INVITE keep-alives.
pub fn parse_session_expires(header: &str) -> Option<(u32, Option<String>)> {
    let mut parts = header.split(';');
    let interval = parts.next()?.trim().parse::<u32>().ok()?;
    let mut refresher = None;
    for part in parts {
        if let Some(v) = part.trim().strip_prefix("refresher=") {
            refresher = Some(v.trim().to_ascii_lowercase());
        }
    }
    Some((interval, refresher))
}

/// Extract the `answer-after=N` param from a `Call-Info` header -- a common
/// intercom/paging-hardware convention (Algo/CyberData/Grandstream paging
/// adapters, Asterisk's chan_pjsip auto-answer support) signaling that this
/// INVITE should be auto-answered after N seconds rather than rung normally.
/// Not an IETF-standardized header parameter; scans every `Call-Info` header
/// line (a message may carry several, and each may itself be a
/// comma-separated list per RFC 3261 header-field syntax) for the first one
/// carrying it.
pub fn parse_call_info_answer_after(msg: &crate::wire::message::SipMessage) -> Option<u32> {
    msg.headers_all("Call-Info").into_iter().find_map(|header| {
        header.split(',').find_map(|entry| {
            entry
                .split(';')
                .skip(1)
                .find_map(|param| param.trim().strip_prefix("answer-after=").and_then(|v| v.trim().parse::<u32>().ok()))
        })
    })
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
    let candidate = header.split(';').next()?.trim();
    // Some UAs send a malformed bare-form header with a display-name-like
    // token glued directly onto the URI with no quotes/brackets (e.g.
    // `600:sip:600@host`) -- RFC 3261's bare "addr-spec" form has no display
    // name at all, but skipping forward to the scheme rather than storing
    // the leading token protects against corrupting `remote_uri`/`peer_uri`
    // if one ever arrives that way. No-op when the scheme already starts
    // the string, which is the normal, well-formed case.
    for scheme in ["sip:", "sips:", "tel:"] {
        if let Some(idx) = candidate.find(scheme) {
            return Some(candidate[idx..].to_string());
        }
    }
    Some(candidate.to_string())
}

/// Extract `(host, port)` from a bare or `sip:`/`sips:`-prefixed URI's
/// authority part -- e.g. `sip:192.168.1.50:5060`, `192.168.1.50`,
/// `sip:bob@192.168.1.50`, or `sip:[::1]:5060`. Defaults to port 5060 when
/// absent. Used by `SipAccount::local_account` (serverless) calls to resolve
/// the initial INVITE's destination straight from the dialed target, since
/// there's no outbound proxy to route it via `self.server_addr` for.
pub fn uri_host_port(uri: &str) -> Option<(String, u16)> {
    let rest = uri.strip_prefix("sips:").or_else(|| uri.strip_prefix("sip:")).unwrap_or(uri);
    let authority = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    let authority = authority.split([';', '?']).next().unwrap_or(authority).trim();
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: "[::1]" or "[::1]:5060".
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(5060);
        return Some((host.to_string(), port));
    }
    match authority.split_once(':') {
        Some((host, port_str)) => Some((host.to_string(), port_str.parse().unwrap_or(5060))),
        None => Some((authority.to_string(), 5060)),
    }
}

/// Percent-encode a `Replaces` value (RFC 3891) for embedding as a URI
/// parameter. Our own generated call-ids/tags are plain hex and never
/// actually contain these characters, but this is correct regardless.
/// `%` must be encoded first to avoid double-encoding the others' output.
pub fn encode_replaces_param(s: &str) -> String {
    s.replace('%', "%25").replace(';', "%3B").replace('=', "%3D").replace(',', "%2C").replace('@', "%40")
}

#[cfg(test)]
#[path = "../../tests/unit/util.rs"]
mod tests;
