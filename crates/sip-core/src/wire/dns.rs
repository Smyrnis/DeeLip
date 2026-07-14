//! Minimal hand-rolled DNS client for the optional custom-nameserver
//! override and SRV-record (RFC 3263) service discovery -- see
//! `NetworkConfig::{custom_nameserver, dns_srv_enabled}` and docs/crates/sip-core.md's
//! "Wire layer" section.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::Context;
use deelip_config::TransportProtocol;
use rand::Rng;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::debug;

const DNS_TIMEOUT: Duration = Duration::from_secs(3);
const QTYPE_A: u16 = 1;
const QTYPE_AAAA: u16 = 28;
const QTYPE_SRV: u16 = 33;

#[derive(Debug, Clone)]
struct SrvTarget {
    priority: u16,
    port: u16,
    target: String,
}

enum Answer {
    Addr(IpAddr),
    Srv(SrvTarget),
}

/// Resolve `connect_host:connect_port` to a socket address, optionally
/// trying SRV-record discovery first -- the single entry point
/// `connect_transport_concrete` calls instead of `tokio::net::lookup_host`
/// directly. Falls back to `tokio::net::lookup_host` (today's exact
/// pre-existing behavior) whenever neither a custom nameserver nor a usable
/// `/etc/resolv.conf` entry is available, so an unconfigured system behaves
/// identically to before this module existed.
pub async fn resolve_target(
    connect_host: &str, connect_port: u16, transport: TransportProtocol, custom_nameserver: Option<&str>,
    srv_enabled: bool,
) -> anyhow::Result<SocketAddr> {
    if let Ok(ip) = connect_host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, connect_port));
    }

    let dns_server = custom_nameserver.and_then(parse_nameserver).or_else(system_resolver);

    if srv_enabled && let Some(server) = dns_server {
        let service = srv_service_name(connect_host, transport);
        match query(server, &service, QTYPE_SRV).await {
            Ok(mut answers) => {
                answers.sort_by_key(|a| match a {
                    Answer::Srv(s) => s.priority,
                    Answer::Addr(_) => u16::MAX,
                });
                for answer in answers {
                    if let Answer::Srv(srv) = answer
                        && let Ok(addr) = resolve_host(&srv.target, srv.port, custom_nameserver).await
                    {
                        debug!("SRV {service} -> {}:{}", srv.target, srv.port);
                        return Ok(addr);
                    }
                }
                debug!("SRV lookup for {service} returned nothing usable, falling back to A/AAAA");
            }
            Err(e) => debug!("SRV lookup for {service} failed ({e:#}), falling back to A/AAAA"),
        }
    }

    resolve_host(connect_host, connect_port, custom_nameserver).await
}

/// SIP SRV service name for a domain, per RFC 3263 -- which one depends on
/// the transport this connection will actually use. `Auto` starts from
/// UDP's service name since that's the first candidate `connect_transport_auto` tries.
fn srv_service_name(domain: &str, transport: TransportProtocol) -> String {
    let service = match transport {
        TransportProtocol::Tls => "_sips._tcp",
        TransportProtocol::Tcp => "_sip._tcp",
        TransportProtocol::Udp | TransportProtocol::Auto => "_sip._udp",
    };
    format!("{service}.{domain}")
}

async fn resolve_host(host: &str, port: u16, custom_nameserver: Option<&str>) -> anyhow::Result<SocketAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let Some(server) = custom_nameserver.and_then(parse_nameserver).or_else(system_resolver) else {
        // No custom nameserver configured and no usable /etc/resolv.conf --
        // fall back to the OS resolver exactly like before this module existed.
        // Bounded the same as the hand-rolled query() path below: an
        // unreachable/misbehaving OS resolver (e.g. no response, common on
        // Windows without a /etc/resolv.conf-style escape hatch) must not be
        // able to hang the caller forever -- this sits on main()'s startup
        // path before the app window exists.
        return timeout(DNS_TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .context("DNS lookup timed out")??
            .next()
            .ok_or_else(|| anyhow::anyhow!("DNS lookup failed for {host}"));
    };
    for qtype in [QTYPE_A, QTYPE_AAAA] {
        if let Ok(answers) = query(server, host, qtype).await {
            for answer in answers {
                if let Answer::Addr(ip) = answer {
                    return Ok(SocketAddr::new(ip, port));
                }
            }
        }
    }
    anyhow::bail!("DNS lookup failed for {host} (server {server})")
}

/// First `nameserver` line in `/etc/resolv.conf` -- the system resolver's
/// own DNS server, used when SRV lookup is enabled but no custom nameserver
/// is configured (SRV isn't exposed by `tokio::net::lookup_host`/libc's
/// resolver, so it needs a real query regardless of whether a custom
/// nameserver was set).
fn system_resolver() -> Option<SocketAddr> {
    let text = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("nameserver")
            && let Ok(ip) = rest.trim().parse::<IpAddr>()
        {
            return Some(SocketAddr::new(ip, 53));
        }
    }
    None
}

fn parse_nameserver(s: &str) -> Option<SocketAddr> {
    s.parse::<SocketAddr>().ok().or_else(|| s.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53)))
}

async fn query(server: SocketAddr, name: &str, qtype: u16) -> anyhow::Result<Vec<Answer>> {
    let id: u16 = rand::thread_rng().r#gen();
    let packet = build_query(id, name, qtype);
    let bind_addr = if server.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
    let sock = UdpSocket::bind(bind_addr).await?;
    sock.connect(server).await?;
    sock.send(&packet).await?;
    let mut buf = [0u8; 2048];
    let n = timeout(DNS_TIMEOUT, sock.recv(&mut buf)).await??;
    parse_response(&buf[..n], id, qtype)
}

fn encode_name(name: &str, buf: &mut Vec<u8>) {
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
}

fn build_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(name.len() + 16);
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD=1
    buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    encode_name(name, &mut buf);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // QCLASS=IN
    buf
}

/// Skip a (possibly-compressed) name at `pos` without decoding it --
/// used only for the question section, which this module always emits
/// itself and never contains internal compression pointers.
fn skip_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *buf.get(pos)? as usize;
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        pos += 1 + len;
    }
}

/// Decode a (possibly-compressed) name at `pos`, returning it plus the
/// offset just past its on-the-wire encoding (before following any
/// compression pointer).
fn decode_name(buf: &[u8], pos: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut cur = pos;
    let mut end_pos = None;
    let mut hops = 0;
    loop {
        hops += 1;
        if hops > 128 {
            return None; // guard against a malicious/corrupt pointer loop
        }
        let len = *buf.get(cur)? as usize;
        if len == 0 {
            if end_pos.is_none() {
                end_pos = Some(cur + 1);
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            let lo = *buf.get(cur + 1)? as usize;
            if end_pos.is_none() {
                end_pos = Some(cur + 2);
            }
            cur = ((len & 0x3F) << 8) | lo;
            continue;
        }
        let label = buf.get(cur + 1..cur + 1 + len)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        cur += 1 + len;
    }
    Some((labels.join("."), end_pos.unwrap_or(cur)))
}

fn parse_response(buf: &[u8], expected_id: u16, qtype: u16) -> anyhow::Result<Vec<Answer>> {
    if buf.len() < 12 {
        anyhow::bail!("DNS response too short");
    }
    if u16::from_be_bytes([buf[0], buf[1]]) != expected_id {
        anyhow::bail!("DNS response ID mismatch");
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let ancount = u16::from_be_bytes([buf[6], buf[7]]) as usize;

    let mut pos = 12;
    for _ in 0..qdcount {
        pos = skip_name(buf, pos).ok_or_else(|| anyhow::anyhow!("bad question name"))? + 4;
    }

    let mut answers = Vec::new();
    for _ in 0..ancount {
        let (_, next) = decode_name(buf, pos).ok_or_else(|| anyhow::anyhow!("bad answer name"))?;
        pos = next;
        if pos + 10 > buf.len() {
            break;
        }
        let rtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let rdlen = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        let rdata = pos + 10;
        if rdata + rdlen > buf.len() {
            break;
        }
        if rtype == qtype {
            match qtype {
                QTYPE_A if rdlen == 4 => {
                    answers.push(Answer::Addr(IpAddr::from([
                        buf[rdata],
                        buf[rdata + 1],
                        buf[rdata + 2],
                        buf[rdata + 3],
                    ])));
                }
                QTYPE_AAAA if rdlen == 16 => {
                    let mut octets = [0u8; 16];
                    octets.copy_from_slice(&buf[rdata..rdata + 16]);
                    answers.push(Answer::Addr(IpAddr::from(octets)));
                }
                QTYPE_SRV if rdlen >= 6 => {
                    let priority = u16::from_be_bytes([buf[rdata], buf[rdata + 1]]);
                    let port = u16::from_be_bytes([buf[rdata + 4], buf[rdata + 5]]);
                    if let Some((target, _)) = decode_name(buf, rdata + 6) {
                        answers.push(Answer::Srv(SrvTarget { priority, port, target }));
                    }
                }
                _ => {}
            }
        }
        pos = rdata + rdlen;
    }
    Ok(answers)
}

#[cfg(test)]
#[path = "../../tests/unit/dns.rs"]
mod tests;
