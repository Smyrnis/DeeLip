//! Voicemail message-waiting indication (RFC 3842, `Event: message-summary`)
//! subscription state and `application/simple-message-summary` body parsing
//! -- hand-rolled, matching this crate's existing style of not pulling in a
//! parsing dependency for simple/fixed body shapes (see `sdp.rs`,
//! `message.rs`, `auth.rs`, `presence.rs`).
//!
//! Kept as a separate module/map from `presence.rs` rather than generalizing
//! that one -- the SUBSCRIBE/refresh/auth-retry mechanics are shared (see
//! `SipStack::build_subscribe`'s `event_package`/`accept` params), but the
//! NOTIFY body shape and the state each carries are different enough that a
//! shared struct would just be a generic blob fighting two different call
//! sites, mirroring the deliberate `dialogs`/`subscriptions` split already
//! documented on `SipStack`.

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
mod tests {
    use super::*;

    #[test]
    fn parses_waiting_with_counts() {
        let body = "Messages-Waiting: yes\r\nMessage-Account: sip:1000@example.com\r\nVoice-Message: 3/2 (0/0)\r\n";
        let state = parse_mwi_summary(body).unwrap();
        assert_eq!(state, MwiState { waiting: true, new_messages: 3, old_messages: 2 });
    }

    #[test]
    fn parses_not_waiting() {
        let body = "Messages-Waiting: no\r\n";
        let state = parse_mwi_summary(body).unwrap();
        assert_eq!(state, MwiState { waiting: false, new_messages: 0, old_messages: 0 });
    }

    #[test]
    fn missing_messages_waiting_line_returns_none() {
        assert_eq!(parse_mwi_summary("Voice-Message: 1/0\r\n"), None);
    }

    #[test]
    fn waiting_without_voice_message_line_defaults_counts_to_zero() {
        let state = parse_mwi_summary("Messages-Waiting: yes\r\n").unwrap();
        assert_eq!(state, MwiState { waiting: true, new_messages: 0, old_messages: 0 });
    }
}
