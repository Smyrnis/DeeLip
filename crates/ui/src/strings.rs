//! Minimal i18n infrastructure: flat/interpolated string lookup keyed by a
//! flat dotted key (e.g. `"directory.search_hint"`), backed by an embedded
//! per-locale JSON file (`assets/locales/<code>.json`). English-only for
//! now -- see `ARCHITECTURE_GAPS.md` item 6 for the planned view-by-view
//! migration to this API and why a second translated locale isn't wired up
//! yet (RTL layout and pluralization are likewise out of scope for now).

use std::collections::HashMap;
use std::sync::OnceLock;

use deelip_config::Language;

static STRINGS: OnceLock<HashMap<String, String>> = OnceLock::new();

const EN: &str = include_str!("../../../assets/locales/en.json");

/// Parse and cache `language`'s locale file -- call once at startup (see
/// `DeelipApp::new`), before any `t`/`tf` call. A call before `init` (or a
/// key missing from the parsed file) falls back to the raw key itself (see
/// `t`'s own doc comment) rather than panicking, so a startup-ordering bug
/// or an unmigrated call site fails visibly instead of crashing.
pub fn init(language: Language) {
    let json = match language {
        Language::En => EN,
    };
    let map: HashMap<String, String> = serde_json::from_str(json).unwrap_or_else(|e| {
        tracing::error!("Failed to parse locale JSON: {e}");
        HashMap::new()
    });
    let _ = STRINGS.set(map);
}

/// Look up `key` in the current locale -- falls back to `key` itself
/// (visibly wrong, not a panic or blank string) if `init` hasn't run yet or
/// the key is missing from the locale file, so an unmigrated call site or a
/// genuine typo is obvious at a glance during Xvfb verification instead of
/// silently rendering blank.
pub fn t(key: &str) -> String {
    STRINGS.get().and_then(|m| m.get(key)).cloned().unwrap_or_else(|| key.to_string())
}

/// Same as `t`, but substitutes `{name}`-style placeholders from `args`
/// after lookup -- e.g. `tf("dialer.calling", &[("name", &caller_name)])`
/// for a locale string like `"Calling {name}…"`. Icon-glyph-plus-label
/// composites (e.g. `"{icon}  Complete Transfer"`) should keep the icon
/// concatenation in Rust and pass only the label half through `t`/`tf` --
/// glyphs aren't translatable text.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}
