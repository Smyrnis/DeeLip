//! SIP presence (RFC 3856/3265, `Event: presence`) subscription state and
//! PIDF body parsing -- hand-rolled, matching this crate's existing style of
//! not pulling in a parsing dependency for simple/fixed body shapes (see
//! `sdp.rs`, `message.rs`, `auth.rs`).

use tokio::time::Instant;

/// Coarse availability derived from a PIDF `<basic>` element (RFC 3863),
/// which only standardizes `open`/`closed` -- finer distinctions like
/// away/busy would need RPID extensions, not implemented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceState {
    /// No NOTIFY received yet, or the subscription failed.
    Unknown,
    Available,
    Offline,
}

/// One outstanding SUBSCRIBE dialog, tracked separately from `Dialog` (call
/// dialogs) since the shape and lifecycle are different: no SDP/media, and a
/// periodic re-SUBSCRIBE before expiry instead of BYE-terminated.
pub struct PresenceSubscription {
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub target_uri: String,
    pub local_cseq: u32,
    pub auth_retried: bool,
    pub refresh_after: Instant,
    pub state: PresenceState,
}

impl PresenceSubscription {
    pub fn new(call_id: String, local_tag: String, target_uri: String) -> Self {
        Self {
            call_id,
            local_tag,
            target_uri,
            remote_tag: None,
            local_cseq: 1,
            auth_retried: false,
            // Overwritten with a real deadline once the 200 OK's Expires is known;
            // starting in the past just means the periodic scan won't try to
            // "refresh" a subscription that hasn't been accepted yet (the scan
            // only re-SUBSCRIBEs entries already past due, and a fresh
            // subscription's response handling sets this before that can matter).
            refresh_after: Instant::now(),
            state: PresenceState::Unknown,
        }
    }

    pub fn next_local_cseq(&mut self) -> u32 {
        self.local_cseq += 1;
        self.local_cseq
    }
}

/// Extract the `<basic>open|closed</basic>` status from a PIDF body.
pub fn parse_pidf_basic(body: &str) -> Option<PresenceState> {
    let start = body.find("<basic>")? + "<basic>".len();
    let rest = &body[start..];
    let end = rest.find("</basic>")?;
    match rest[..end].trim() {
        "open" => Some(PresenceState::Available),
        "closed" => Some(PresenceState::Offline),
        _ => None,
    }
}

/// Parse a `Subscription-State:` header value into its state token
/// (`active`/`pending`/`terminated`) and an optional `expires=`/`retry-after=` param.
pub fn parse_subscription_state(header: &str) -> (&str, Option<u32>) {
    let mut parts = header.split(';');
    let state = parts.next().unwrap_or("").trim();
    let mut param = None;
    for part in parts {
        let part = part.trim();
        let value = part.strip_prefix("expires=").or_else(|| part.strip_prefix("retry-after="));
        if let Some(v) = value
            && let Ok(n) = v.trim_matches('"').parse::<u32>()
        {
            param = Some(n);
        }
    }
    (state, param)
}

#[cfg(test)]
#[path = "../../tests/unit/presence.rs"]
mod tests;
