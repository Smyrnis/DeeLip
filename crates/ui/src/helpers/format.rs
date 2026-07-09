use deelip_config::{CallStatus, SipAccount};
use deelip_sip::AudioCodec;

/// Display label for an account picker — `display_name` if set, else `user@server`.
pub(crate) fn account_label(account: &SipAccount) -> String {
    match account
        .account_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| account.display_name.as_deref().filter(|s| !s.is_empty()))
    {
        Some(name) => name.to_string(),
        None => format!("{}@{}", account.username, account.server),
    }
}

pub(crate) fn status_filter_label(filter: &Option<CallStatus>) -> &'static str {
    match filter {
        None => "All",
        Some(CallStatus::Answered) => "Answered",
        Some(CallStatus::Missed) => "Missed",
        Some(CallStatus::Rejected) => "Rejected",
        Some(CallStatus::Failed) => "Failed",
    }
}

/// Quote a CSV field if it contains a comma, quote, or newline.
pub(crate) fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Shorten a SIP URI for display: `sip:alice@example.com` → `alice@example.com`.
pub(crate) fn short_uri(uri: &str) -> String {
    uri.strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))
        .unwrap_or(uri)
        .to_string()
}

/// Display label for a `SipAccount::codec_order` entry in Settings.
pub(crate) fn codec_label(s: &str) -> &'static str {
    match s {
        "opus" => "Opus",
        "g722" => "G.722",
        "pcmu" => "PCMU (G.711 μ-law)",
        "pcma" => "PCMA (G.711 A-law)",
        "gsm" => "GSM 06.10",
        "ilbc" => "iLBC",
        "g729" => "G.729",
        _ => "Unknown",
    }
}

/// Same table as `codec_label`, keyed by `AudioCodec` directly -- for
/// displaying an already-negotiated codec (e.g. call statistics) rather
/// than a `SipAccount::codec_order` entry.
pub(crate) fn audio_codec_label(codec: AudioCodec) -> &'static str {
    codec_label(match codec {
        AudioCodec::Opus => "opus",
        AudioCodec::G722 => "g722",
        AudioCodec::Pcmu => "pcmu",
        AudioCodec::Pcma => "pcma",
        AudioCodec::Gsm => "gsm",
        AudioCodec::Ilbc => "ilbc",
        AudioCodec::G729 => "g729",
    })
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn format_duration(secs: u32) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// `MM:SS` (or `H:MM:SS` past an hour) -- the focused-call screen's live
/// timer, always zero-padded and always showing minutes even under a
/// minute (unlike `format_duration`'s history-log-friendly "40s"), since a
/// ticking instrument-panel clock reads oddly if its own field count keeps
/// changing.
pub(crate) fn format_call_timer(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Local calendar date/time for a Unix timestamp, e.g. `"2026-07-09 14:32"`
/// -- History's absolute-timestamp display, replacing the old relative
/// "4d ago" (`format_age`). Uses `chrono` rather than hand-rolling
/// calendar/timezone math (leap years, month lengths, DST offsets).
pub(crate) fn format_timestamp(ts: u64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts as i64, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        _ => "—".to_string(),
    }
}
