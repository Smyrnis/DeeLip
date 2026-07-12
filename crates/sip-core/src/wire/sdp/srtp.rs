//! SDES-SRTP (RFC 4568) key material carried in `a=crypto:` SDP lines.

pub const SRTP_MASTER_KEY_LEN: usize = 16;
pub const SRTP_MASTER_SALT_LEN: usize = 14;
const SRTP_SUITE: &str = "AES_CM_128_HMAC_SHA1_80";

/// SDES-SRTP master key + salt (RFC 4568), carried in `a=crypto:` SDP lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtpParams {
    pub key: [u8; SRTP_MASTER_KEY_LEN],
    pub salt: [u8; SRTP_MASTER_SALT_LEN],
}

impl SrtpParams {
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = [0u8; SRTP_MASTER_KEY_LEN];
        let mut salt = [0u8; SRTP_MASTER_SALT_LEN];
        let mut rng = rand::rngs::OsRng;
        rng.fill_bytes(&mut key);
        rng.fill_bytes(&mut salt);
        Self { key, salt }
    }

    pub(super) fn to_crypto_line(&self, tag: u32) -> String {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut combined = Vec::with_capacity(SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN);
        combined.extend_from_slice(&self.key);
        combined.extend_from_slice(&self.salt);
        let inline = STANDARD.encode(combined);
        format!("a=crypto:{tag} {SRTP_SUITE} inline:{inline}\r\n")
    }

    pub(super) fn parse_crypto_line(line: &str) -> Option<Self> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        // "a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:<base64>[|2^20|1:4]"
        let rest = line.trim().strip_prefix("a=crypto:")?;
        let mut parts = rest.split_whitespace();
        parts.next()?; // tag
        let suite = parts.next()?;
        if suite != SRTP_SUITE {
            return None;
        }
        let key_param = parts.next()?;
        let b64 = key_param.strip_prefix("inline:")?.split('|').next()?;
        let raw = STANDARD.decode(b64).ok()?;
        if raw.len() != SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN {
            return None;
        }
        let mut key = [0u8; SRTP_MASTER_KEY_LEN];
        let mut salt = [0u8; SRTP_MASTER_SALT_LEN];
        key.copy_from_slice(&raw[..SRTP_MASTER_KEY_LEN]);
        salt.copy_from_slice(&raw[SRTP_MASTER_KEY_LEN..]);
        Some(Self { key, salt })
    }
}

/// Both sides' SRTP keys for one call. Per RFC 4568, each side's a=crypto line
/// declares the key IT uses to encrypt what it sends: encrypt outgoing traffic
/// with `local`'s own key, decrypt incoming traffic with `remote`'s key.
#[derive(Debug, Clone)]
pub struct SrtpSession {
    pub local: SrtpParams,
    pub remote: SrtpParams,
}
