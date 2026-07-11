use deelip_config::{apply_dial_plan, DialPlanRule};

/// Normalize a dial-box entry into a full SIP URI. Bare numbers/usernames
/// (no scheme, no "@") are dialed against the account's own domain, matching
/// how MicroSIP and other softphones resolve local extensions.
pub(crate) fn normalize_target(raw: &str, domain: &str) -> String {
    normalize_target_with_prefix(raw, domain, "", &[])
}

/// Same as `normalize_target`, but transforms a bare number before
/// appending the domain -- only the bare-number case gets it, since a full
/// SIP URI or an explicit `user@host` entry is already a specific
/// destination, not a local extension to dial out from. `dial_plan`'s
/// first matching rule (see `apply_dial_plan`) wins; `prefix` (e.g. "9" for
/// an outside line) is just the simple auto-prepend fallback used when no
/// rule matches (or the list is empty).
pub(crate) fn normalize_target_with_prefix(
    raw: &str, domain: &str, prefix: &str, dial_plan: &[DialPlanRule],
) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("sip:") || lower.starts_with("sips:") {
        raw.to_string()
    } else if raw.contains('@') {
        format!("sip:{raw}")
    } else {
        let dialed = apply_dial_plan(raw, dial_plan).unwrap_or_else(|| format!("{prefix}{raw}"));
        // A `SipAccount::local_account` has no domain to append -- dialing a
        // bare IP/host (e.g. "192.168.1.50") from one should stay just that,
        // not become the malformed "sip:192.168.1.50@" this would otherwise
        // produce.
        if domain.trim().is_empty() {
            format!("sip:{dialed}")
        } else {
            format!("sip:{dialed}@{domain}")
        }
    }
}

/// Extract the user/number portion of a SIP URI for blocklist comparison,
/// e.g. `sip:5551234@host;user=phone` -> `"5551234"`. Bare entries (no
/// scheme/`@`) pass through unchanged (lowercased), so a blocklist entry can
/// be typed as either a plain number or a full SIP URI.
pub(crate) fn extract_user_part(uri: &str) -> String {
    let lower = uri.trim().to_ascii_lowercase();
    let stripped = lower.strip_prefix("sip:").or_else(|| lower.strip_prefix("sips:")).unwrap_or(&lower);
    let before_at = stripped.split('@').next().unwrap_or(stripped);
    before_at.split(';').next().unwrap_or(before_at).to_string()
}

#[cfg(test)]
#[path = "../../tests/unit/helpers.rs"]
mod tests;
