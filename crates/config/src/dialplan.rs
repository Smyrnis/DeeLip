//! Rule-based number transformation applied before dialing a bare
//! (non-URI) number -- a small ordered list of regex match/replace rules,
//! evaluated first-match-wins, falling back to `SipAccount::dialing_prefix`'s
//! simple auto-prepend if nothing matches. Uses the `regex` crate directly
//! (already present in this workspace's dependency tree transitively)
//! rather than a hand-rolled pattern language, since a real match/replace
//! engine is exactly what `regex` already is -- no reason to reinvent it.

use regex::Regex;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DialPlanRule {
    /// Regex matched against the raw dialed number, before any `@domain`
    /// is appended -- e.g. `^0(\d+)$` to strip a leading trunk-access "0".
    pub pattern: String,
    /// Regex replacement template (`$1`, `${1}`, etc. -- see `regex::Regex::replace`)
    /// producing the number actually dialed.
    pub replacement: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Apply the first enabled rule whose `pattern` matches `raw`, in list
/// order, returning the transformed number -- `None` if no rule matches
/// (or the list is empty, or every matching rule turned out to have an
/// invalid regex), so callers fall back to their own default (e.g.
/// `SipAccount::dialing_prefix`'s auto-prepend).
pub fn apply_dial_plan(raw: &str, rules: &[DialPlanRule]) -> Option<String> {
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        let Ok(re) = Regex::new(&rule.pattern) else {
            continue;
        };
        if re.is_match(raw) {
            return Some(re.replace(raw, rule.replacement.as_str()).into_owned());
        }
    }
    None
}

#[cfg(test)]
#[path = "../tests/unit/dialplan.rs"]
mod tests;
