use std::collections::HashMap;
use md5::{Digest, Md5};

// ── Digest computation ────────────────────────────────────────────────────────

fn md5_hex(data: &str) -> String {
    let mut h = Md5::new();
    h.update(data.as_bytes());
    format!("{:x}", h.finalize())
}

/// Compute the RFC 2617 digest response value.
pub fn compute_digest_response(
    username: &str,
    realm:    &str,
    password: &str,
    method:   &str,
    uri:      &str,
    nonce:    &str,
) -> String {
    let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
}

/// Build an `Authorization:` header value.
pub fn build_auth_header(
    username: &str,
    realm:    &str,
    nonce:    &str,
    uri:      &str,
    response: &str,
) -> String {
    format!(
        "Authorization: Digest username=\"{username}\", realm=\"{realm}\", \
         nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5"
    )
}

/// Parse a `WWW-Authenticate`/`Proxy-Authenticate` challenge and build the
/// matching `Authorization:` header value for a retried request -- the
/// parse-compute-build sequence shared by REGISTER, INVITE, and SUBSCRIBE's
/// 401/407 retry handling (previously duplicated inline at each call site).
pub fn build_challenge_response(
    username: &str,
    password: &str,
    method:   &str,
    uri:      &str,
    challenge_header: &str,
) -> Option<String> {
    let challenge = DigestChallenge::parse(challenge_header)?;
    let digest = compute_digest_response(username, &challenge.realm, password, method, uri, &challenge.nonce);
    Some(build_auth_header(username, &challenge.realm, &challenge.nonce, uri, &digest))
}

// ── Challenge parsing ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DigestChallenge {
    pub realm:     String,
    pub nonce:     String,
    pub algorithm: String,
    pub opaque:    Option<String>,
}

impl DigestChallenge {
    /// Parse a `WWW-Authenticate: Digest ...` header value.
    pub fn parse(header: &str) -> Option<Self> {
        let body = header
            .trim_start_matches("Digest")
            .trim_start_matches("digest")
            .trim();

        let params = parse_kv_pairs(body);

        Some(DigestChallenge {
            realm:     params.get("realm")?.clone(),
            nonce:     params.get("nonce")?.clone(),
            algorithm: params.get("algorithm").cloned().unwrap_or_else(|| "MD5".into()),
            opaque:    params.get("opaque").cloned(),
        })
    }
}

/// Parse comma-separated key="value" or key=value pairs.
fn parse_kv_pairs(s: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    // Split on commas that are not inside quotes
    let mut depth = 0u8;
    let mut current = String::new();
    for c in s.chars() {
        match c {
            '"' => { depth ^= 1; current.push(c); }
            ',' if depth == 0 => {
                insert_kv(&mut map, &current);
                current.clear();
            }
            _ => current.push(c),
        }
    }
    insert_kv(&mut map, &current);
    map
}

fn insert_kv(map: &mut HashMap<String, String>, s: &str) {
    let s = s.trim();
    if let Some(pos) = s.find('=') {
        let key   = s[..pos].trim().to_ascii_lowercase();
        let value = s[pos + 1..].trim().trim_matches('"').to_string();
        map.insert(key, value);
    }
}
