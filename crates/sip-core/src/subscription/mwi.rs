//! Voicemail message-waiting indication (RFC 3842, `Event: message-summary`)
//! subscription state and `application/simple-message-summary` body parsing.
//! Why this stays a separate module/map from `presence.rs`: docs/crates/sip-core.md's
//! "Why MWI is a separate module" section.

use tokio::time::Instant;

/// Parsed `application/simple-message-summary` body (RFC 3842 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MwiState {
    pub waiting: bool,
    /// New/old counts from the `Voice-Message:` line's first `N/M` pair, if
    /// present -- 0/0 if the line is missing (only `Messages-Waiting` is
    /// mandatory per the RFC).
    pub new_messages: u32,
    pub old_messages: u32,
}

/// One outstanding MWI SUBSCRIBE dialog -- same shape as `PresenceSubscription`
/// (call_id/tags/cseq/refresh_after bookkeeping is identical regardless of
/// event package) but kept as its own type since the two are never
/// interchangeable and duplicating ~10 plain fields is cheaper than
/// generalizing over a state type that's genuinely different per package.
pub struct MwiSubscription {
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub target_uri: String,
    pub local_cseq: u32,
    pub auth_retried: bool,
    pub refresh_after: Instant,
    pub state: MwiState,
}

impl MwiSubscription {
    pub fn new(call_id: String, local_tag: String, target_uri: String) -> Self {
        Self {
            call_id,
            local_tag,
            target_uri,
            remote_tag: None,
            local_cseq: 1,
            auth_retried: false,
            refresh_after: Instant::now(),
            state: MwiState::default(),
        }
    }

    pub fn next_local_cseq(&mut self) -> u32 {
        self.local_cseq += 1;
        self.local_cseq
    }
}

/// Parse an `application/simple-message-summary` body. `None` if it doesn't
/// contain a `Messages-Waiting:` line at all (the one mandatory field).
pub fn parse_mwi_summary(body: &str) -> Option<MwiState> {
    let mut found_waiting = false;
    let mut state = MwiState::default();
    for line in body.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Messages-Waiting:") {
            found_waiting = true;
            state.waiting = v.trim().eq_ignore_ascii_case("yes");
        } else if let Some(v) = line.strip_prefix("Voice-Message:") {
            // "N/M (N2/M2)" -- only the first (non-urgent) N/M pair is used.
            let counts = v.split_whitespace().next().unwrap_or("");
            let mut parts = counts.splitn(2, '/');
            if let (Some(n), Some(m)) = (parts.next(), parts.next()) {
                state.new_messages = n.parse().unwrap_or(0);
                state.old_messages = m.parse().unwrap_or(0);
            }
        }
    }
    found_waiting.then_some(state)
}

#[cfg(test)]
#[path = "../../tests/unit/mwi.rs"]
mod tests;
