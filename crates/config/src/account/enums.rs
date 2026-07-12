//! Small serde-backed enums shared by `SipAccount`/`AppConfig` -- transport,
//! media encryption, DTMF mode, update-check cadence, list double-click
//! action, recording format, and UI language -- plus each one's hand-rolled
//! SQL string mapping (`db.rs` stores them as plain `TEXT` columns, not via
//! serde, so these can't just derive `Display`/`FromStr`).

use serde::{Deserialize, Serialize};

// ── Transport protocol ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransportProtocol {
    #[default]
    Udp,
    Tcp,
    Tls,
    /// Try UDP, then TCP, then TLS at connect time, keeping whichever one
    /// actually gets a response to a probe REGISTER -- see
    /// `deelip_sip::client`'s `connect_transport_auto`. Once resolved, the
    /// rest of the stack treats the connection exactly as if that concrete
    /// transport had been configured directly.
    Auto,
}

pub(super) fn transport_to_str(t: &TransportProtocol) -> &'static str {
    match t {
        TransportProtocol::Udp => "udp",
        TransportProtocol::Tcp => "tcp",
        TransportProtocol::Tls => "tls",
        TransportProtocol::Auto => "auto",
    }
}
pub(super) fn transport_from_str(s: &str) -> TransportProtocol {
    match s {
        "tcp" => TransportProtocol::Tcp,
        "tls" => TransportProtocol::Tls,
        "auto" => TransportProtocol::Auto,
        _ => TransportProtocol::Udp,
    }
}

// ── Media encryption ──────────────────────────────────────────────────────────

/// Whether to offer/require SRTP for this account's media -- independent of
/// the signaling transport, unlike DeeLip's previous behavior (SRTP exactly
/// when `transport == Tls`, and nothing else).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaEncryption {
    /// Offer SRTP exactly when the (resolved) signaling transport is TLS --
    /// preserves every existing config's actual behavior as the default.
    #[default]
    MatchTransport,
    /// Never offer SRTP, regardless of signaling transport.
    Disabled,
    /// Always offer SRTP (SDES, RFC 4568), regardless of signaling transport
    /// -- e.g. encrypted media over a plain UDP/TCP signaling path.
    Enabled,
    /// RFC 6189 ZRTP key agreement instead of SDES -- negotiated entirely
    /// in-band over the RTP socket, so no `a=crypto`/SDP involvement at all
    /// (see `SipAccount::wants_srtp`, which returns `false` for this variant
    /// on purpose). Full picture, including verification scope/caveats:
    /// `docs/crates/sip-core.md`.
    Zrtp,
}

pub(super) fn media_encryption_to_str(m: MediaEncryption) -> &'static str {
    match m {
        MediaEncryption::MatchTransport => "match_transport",
        MediaEncryption::Disabled => "disabled",
        MediaEncryption::Enabled => "enabled",
        MediaEncryption::Zrtp => "zrtp",
    }
}
pub(super) fn media_encryption_from_str(s: &str) -> MediaEncryption {
    match s {
        "disabled" => MediaEncryption::Disabled,
        "enabled" => MediaEncryption::Enabled,
        "zrtp" => MediaEncryption::Zrtp,
        _ => MediaEncryption::MatchTransport,
    }
}

// ── DTMF mode ─────────────────────────────────────────────────────────────────

/// How this account sends DTMF digits during a call — some PBXes/gateways
/// only reliably support one of these, so it's configurable per account
/// rather than a single hardcoded scheme.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DtmfMode {
    /// RFC 2833/4733 out-of-band RTP telephone-event packets (what DeeLip
    /// has always done) — the most broadly interoperable default.
    #[default]
    Rfc2833,
    /// SIP INFO requests with an `application/dtmf-relay` body — an older,
    /// still-common scheme some PBXes/gateways prefer over RTP events.
    SipInfo,
    /// A real dual-tone audio signal mixed into the outgoing RTP audio
    /// itself, exactly as if the digit were dialed on a physical phone —
    /// for gateways/PBXes that don't reliably support either out-of-band
    /// scheme above.
    Inband,
    /// Detect per-call rather than force one scheme: use RFC 2833 if the
    /// negotiated SDP carries a `telephone-event` payload type, otherwise
    /// fall back to SIP INFO. Decided once per call from the already-
    /// negotiated media (`CallMediaReady::dtmf_type`), not re-checked digit
    /// by digit.
    Auto,
}

pub(super) fn dtmf_mode_to_str(m: DtmfMode) -> &'static str {
    match m {
        DtmfMode::Rfc2833 => "rfc2833",
        DtmfMode::SipInfo => "sipinfo",
        DtmfMode::Inband => "inband",
        DtmfMode::Auto => "auto",
    }
}
pub(super) fn dtmf_mode_from_str(s: &str) -> DtmfMode {
    match s {
        "sipinfo" => DtmfMode::SipInfo,
        "inband" => DtmfMode::Inband,
        "auto" => DtmfMode::Auto,
        _ => DtmfMode::Rfc2833,
    }
}

// ── Update check frequency ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateCheckFrequency {
    #[default]
    Always,
    Daily,
    Weekly,
    Never,
}

impl UpdateCheckFrequency {
    /// Minimum seconds that must elapse since `last_update_check` before a
    /// new automatic check is due -- `None` for `Always` (no minimum) and
    /// `Never` (never due, checked separately by the caller).
    fn min_interval_secs(self) -> Option<u64> {
        match self {
            UpdateCheckFrequency::Always => Some(0),
            UpdateCheckFrequency::Daily => Some(24 * 3600),
            UpdateCheckFrequency::Weekly => Some(7 * 24 * 3600),
            UpdateCheckFrequency::Never => None,
        }
    }

    /// Whether an automatic update check is due right now, given
    /// `last_update_check` and the current unix time.
    pub fn is_due(self, last_update_check: Option<u64>, now: u64) -> bool {
        let Some(min_interval) = self.min_interval_secs() else {
            return false;
        };
        match last_update_check {
            Some(last) => now.saturating_sub(last) >= min_interval,
            None => true,
        }
    }
}

pub(super) fn update_check_frequency_to_str(f: UpdateCheckFrequency) -> &'static str {
    match f {
        UpdateCheckFrequency::Always => "always",
        UpdateCheckFrequency::Daily => "daily",
        UpdateCheckFrequency::Weekly => "weekly",
        UpdateCheckFrequency::Never => "never",
    }
}
pub(super) fn update_check_frequency_from_str(s: &str) -> UpdateCheckFrequency {
    match s {
        "daily" => UpdateCheckFrequency::Daily,
        "weekly" => UpdateCheckFrequency::Weekly,
        "never" => UpdateCheckFrequency::Never,
        _ => UpdateCheckFrequency::Always,
    }
}

// ── Default list action ───────────────────────────────────────────────────────

/// What double-clicking a row's name/number in History or Contacts does --
/// see `deelip_ui::helpers::list_row`'s double-click handling. `Edit` only
/// makes sense in Contacts (nothing to edit in a History entry); History
/// falls back to `Call` if this is set to `Edit`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DefaultListAction {
    #[default]
    Call,
    Message,
    Edit,
}

pub(super) fn default_list_action_to_str(a: DefaultListAction) -> &'static str {
    match a {
        DefaultListAction::Call => "call",
        DefaultListAction::Message => "message",
        DefaultListAction::Edit => "edit",
    }
}
pub(super) fn default_list_action_from_str(s: &str) -> DefaultListAction {
    match s {
        "message" => DefaultListAction::Message,
        "edit" => DefaultListAction::Edit,
        _ => DefaultListAction::Call,
    }
}

// ── Recording format ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecordingFormat {
    #[default]
    Wav,
    Mp3,
}

pub(super) fn recording_format_to_str(f: RecordingFormat) -> &'static str {
    match f {
        RecordingFormat::Wav => "wav",
        RecordingFormat::Mp3 => "mp3",
    }
}
pub(super) fn recording_format_from_str(s: &str) -> RecordingFormat {
    match s {
        "mp3" => RecordingFormat::Mp3,
        _ => RecordingFormat::Wav,
    }
}

// ── Language ───────────────────────────────────────────────────────────────────

/// UI display language -- infrastructure for `deelip_ui::strings`' locale
/// loading. English-only for now, see `docs/crates/i18n.md`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
}

pub(super) fn language_to_str(l: Language) -> &'static str {
    match l {
        Language::En => "en",
    }
}
pub(super) fn language_from_str(_s: &str) -> Language {
    // Only one variant exists today -- see `Language`'s own doc comment.
    // Kept as a real function (not a bare constant) matching every sibling
    // `..._from_str` in this file, so adding a second locale later is a
    // one-line match-arm change here, not a signature change at both of
    // this function's call sites.
    Language::En
}
