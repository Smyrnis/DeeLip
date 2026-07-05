use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::db::{bool_to_sql, sql_int_to_bool, sql_to_bool};
use crate::Db;

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

fn transport_to_str(t: &TransportProtocol) -> &'static str {
    match t {
        TransportProtocol::Udp => "udp",
        TransportProtocol::Tcp => "tcp",
        TransportProtocol::Tls => "tls",
        TransportProtocol::Auto => "auto",
    }
}
fn transport_from_str(s: &str) -> TransportProtocol {
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
}

fn media_encryption_to_str(m: MediaEncryption) -> &'static str {
    match m {
        MediaEncryption::MatchTransport => "match_transport",
        MediaEncryption::Disabled => "disabled",
        MediaEncryption::Enabled => "enabled",
    }
}
fn media_encryption_from_str(s: &str) -> MediaEncryption {
    match s {
        "disabled" => MediaEncryption::Disabled,
        "enabled" => MediaEncryption::Enabled,
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
}

fn dtmf_mode_to_str(m: DtmfMode) -> &'static str {
    match m {
        DtmfMode::Rfc2833 => "rfc2833",
        DtmfMode::SipInfo => "sipinfo",
        DtmfMode::Inband => "inband",
    }
}
fn dtmf_mode_from_str(s: &str) -> DtmfMode {
    match s {
        "sipinfo" => DtmfMode::SipInfo,
        "inband" => DtmfMode::Inband,
        _ => DtmfMode::Rfc2833,
    }
}

// ── SIP account ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipAccount {
    pub username: String,
    pub password: String,
    pub server: String,
    #[serde(default = "default_sip_port")]
    pub port: u16,
    pub display_name: Option<String>,
    #[serde(default)]
    pub transport: TransportProtocol,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Skip TLS certificate verification (self-signed/home-lab PBXes). Off by default.
    #[serde(default)]
    pub tls_insecure_skip_verify: bool,
    /// If set, an incoming call on this account that rings unanswered for
    /// `no_answer_timeout_secs` is redirected here (302 Moved Temporarily)
    /// instead of continuing to ring indefinitely. Empty/unset disables it.
    #[serde(default)]
    pub no_answer_forward: Option<String>,
    #[serde(default = "default_no_answer_timeout")]
    pub no_answer_timeout_secs: u32,
    /// If true, every incoming call on this account is immediately rejected
    /// with 486 Busy Here — no ringing, no forwarding (DND takes priority
    /// over forward_always/forward_on_busy if both are set).
    #[serde(default)]
    pub dnd: bool,
    /// If set, every incoming call on this account is immediately redirected
    /// here (302 Moved Temporarily) instead of ringing. Ignored while `dnd` is on.
    #[serde(default)]
    pub forward_always: Option<String>,
    /// If set, an incoming call that arrives while this account already has
    /// at least one active call is redirected here instead of ringing as a
    /// second (call-waiting) call. Unset: call-waiting behaves as it does today.
    #[serde(default)]
    pub forward_on_busy: Option<String>,
    /// Enabled codecs in preference order (canonical lowercase names:
    /// "opus", "g722", "pcmu", "pcma"). Controls both what we offer when
    /// calling out and what we're willing to answer with on an incoming
    /// call — a codec absent from this list is never used in either
    /// direction. Kept as plain strings rather than `deelip_sip::AudioCodec`
    /// since `deelip-sip` depends on `deelip-config`, not the reverse.
    #[serde(default = "default_codec_order")]
    pub codec_order: Vec<String>,
    /// If set (a codec name from `codec_order`'s vocabulary, e.g. "pcmu"),
    /// an incoming call's negotiated codec is forced to this one whenever
    /// the remote offer supports it at all -- overriding the offer's own PT
    /// preference order (which otherwise always wins among the codecs this
    /// account accepts, see `deelip_sip::wire::sdp::parse_sdp`). If the
    /// remote didn't offer it, negotiation falls back to normal. Unset:
    /// today's behavior, no override.
    #[serde(default)]
    pub force_incoming_codec: Option<String>,
    /// Negotiate and use RFC 3389 comfort noise: during silence (detected
    /// by a simple energy-threshold VAD in `deelip_media`), send an
    /// occasional Comfort Noise/SID packet instead of continuous encoded
    /// audio, and synthesize matching background noise for the far end's
    /// own silence rather than dead air. Only takes effect alongside a
    /// codec that shares CN's 8000 Hz RTP clock (i.e. not Opus). Off by
    /// default, like every other opt-in audio-processing toggle here.
    #[serde(default)]
    pub vad_enabled: bool,
    /// How this account sends DTMF digits (see `DtmfMode`).
    #[serde(default)]
    pub dtmf_mode: DtmfMode,
    /// If true, an incoming call on this account is automatically answered
    /// after `auto_answer_secs` of ringing (intercom-style) instead of
    /// waiting for the user. Off by default. Takes priority over DND/
    /// forwarding is NOT implied — those are checked first in the
    /// `IncomingCall` handler, same precedence as before this existed.
    #[serde(default)]
    pub auto_answer_enabled: bool,
    #[serde(default = "default_auto_answer_secs")]
    pub auto_answer_secs: u32,
    /// Mailbox to subscribe to for voicemail message-waiting indication
    /// (RFC 3842 `Event: message-summary`). Unset disables MWI entirely
    /// for this account — there's no separate on/off flag, presence of a
    /// mailbox value *is* the toggle (same `Option<String>` idiom as
    /// `no_answer_forward`/`forward_always` above).
    #[serde(default)]
    pub mailbox: Option<String>,
    /// Friendly nickname shown in the account picker/list, distinct from
    /// `display_name` (which goes out over the wire in From/Contact).
    /// Purely cosmetic -- falls back to `account_label`'s existing
    /// derivation (`display_name` or `username@server`) when unset.
    #[serde(default)]
    pub account_name: Option<String>,
    /// Outbound proxy ("host" or "host:port") to actually establish the SIP
    /// transport connection with, instead of `server`/`port` directly --
    /// `server` still supplies the SIP domain used in From/To/Contact URIs
    /// (via `domain()`) either way. Unset (the common case): connect
    /// straight to `server`/`port`, same as before this field existed.
    #[serde(default)]
    pub sip_proxy: Option<String>,
    /// SIP domain to use in From/To/Contact/Request-URIs when it differs
    /// from the registrar host in `server` (e.g. registering against a
    /// load-balanced front-end while presenting a stable public domain).
    /// Unset: `domain()` falls back to `server`, today's behavior.
    #[serde(default)]
    pub domain: Option<String>,
    /// Digest-auth login, when a provider requires an authentication
    /// identity distinct from the public SIP identity in `username`.
    /// Unset: `auth_username()` falls back to `username`, today's behavior.
    #[serde(default)]
    pub auth_username: Option<String>,
    /// Digits automatically prepended to a bare (non-URI) dialed number,
    /// e.g. "9" for an outside line through a PBX. Unset/empty: no prefix,
    /// today's behavior.
    #[serde(default)]
    pub dialing_prefix: Option<String>,
    /// Send `Privacy: id` on outgoing INVITEs, asking the far end/proxy to
    /// suppress caller-ID presentation (RFC 3323). Off by default.
    #[serde(default)]
    pub hide_caller_id: bool,
    /// Requested REGISTER `Expires` value in seconds -- the server may
    /// still return a shorter value in its response, which is always what
    /// re-registration timing actually uses (see `REG_MARGIN` in
    /// `deelip_sip::client`); this only controls what we *ask* for.
    #[serde(default = "default_register_expires")]
    pub register_expires: u32,
    /// If set, send a periodic empty UDP keepalive packet (a lone CRLF,
    /// per the common "double-CRLF ping" convention) to the registrar
    /// every N seconds to hold a NAT/firewall's outbound binding open
    /// between registrations. Unset/0 disables it -- today's behavior.
    #[serde(default)]
    pub keepalive_secs: Option<u32>,
    /// Independent SRTP policy for this account's media -- see
    /// `MediaEncryption`'s doc comment. Defaults to `MatchTransport`, which
    /// preserves the behavior every existing config already has.
    #[serde(default)]
    pub media_encryption: MediaEncryption,
    /// Manual override for the address advertised in Contact/SDP (`c=`)
    /// lines for this account, instead of the globally STUN-discovered
    /// external IP (or the locally-bound IP if STUN found nothing/isn't
    /// configured). Unset: today's behavior, sharing the one global value.
    #[serde(default)]
    pub public_address: Option<String>,
    /// Per-account override for whether to attempt ICE (RFC 8445) --
    /// `None` follows the global `AppConfig::ice_enabled` toggle (today's
    /// behavior); `Some(true)`/`Some(false)` force it on/off for just this
    /// account regardless of the global setting.
    #[serde(default)]
    pub ice_enabled: Option<bool>,
}

fn default_sip_port() -> u16 {
    5060
}
fn default_true() -> bool {
    true
}
fn default_no_answer_timeout() -> u32 {
    20
}
fn default_auto_answer_secs() -> u32 {
    3
}
fn default_register_expires() -> u32 {
    3600
}
fn default_codec_order() -> Vec<String> {
    ["opus", "g722", "pcmu", "pcma"].map(String::from).to_vec()
}

impl Default for SipAccount {
    fn default() -> Self {
        Self {
            username: "your_username".into(),
            password: "your_password".into(),
            server: "your.sip.server".into(),
            port: 5060,
            display_name: Some("Your Name".into()),
            transport: TransportProtocol::Udp,
            enabled: true,
            tls_insecure_skip_verify: false,
            no_answer_forward: None,
            no_answer_timeout_secs: default_no_answer_timeout(),
            dnd: false,
            forward_always: None,
            forward_on_busy: None,
            codec_order: default_codec_order(),
            force_incoming_codec: None,
            vad_enabled: false,
            dtmf_mode: DtmfMode::default(),
            auto_answer_enabled: false,
            auto_answer_secs: default_auto_answer_secs(),
            mailbox: None,
            account_name: None,
            sip_proxy: None,
            domain: None,
            auth_username: None,
            dialing_prefix: None,
            hide_caller_id: false,
            register_expires: default_register_expires(),
            keepalive_secs: None,
            media_encryption: MediaEncryption::default(),
            public_address: None,
            ice_enabled: None,
        }
    }
}

impl SipAccount {
    /// SIP domain used in From/To/Contact/Request-URIs -- `domain` if set,
    /// otherwise `server` (the common case: registrar and domain are the
    /// same host).
    pub fn domain(&self) -> &str {
        self.domain
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.server)
    }

    /// Digest-auth username -- `auth_username` if set, otherwise `username`.
    pub fn auth_username(&self) -> &str {
        self.auth_username
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.username)
    }

    /// (host, port) to actually establish the SIP transport connection
    /// with -- the configured outbound `sip_proxy` if set (splitting an
    /// optional trailing ":port", defaulting to this account's own `port`
    /// when absent), otherwise `server`/`port` directly.
    pub fn connect_target(&self) -> (String, u16) {
        match self.sip_proxy.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(proxy) => match proxy.rsplit_once(':') {
                Some((host, port_str)) if !host.is_empty() && port_str.parse::<u16>().is_ok() => {
                    (host.to_string(), port_str.parse().unwrap())
                }
                _ => (proxy.to_string(), self.port),
            },
            None => (self.server.clone(), self.port),
        }
    }

    /// Whether to offer/require SRTP, given the transport a connection
    /// actually resolved to (which may differ from `self.transport` when
    /// it's `TransportProtocol::Auto`) -- see `MediaEncryption`.
    pub fn wants_srtp(&self, resolved_transport: TransportProtocol) -> bool {
        match self.media_encryption {
            MediaEncryption::MatchTransport => resolved_transport == TransportProtocol::Tls,
            MediaEncryption::Disabled => false,
            MediaEncryption::Enabled => true,
        }
    }

    /// Whether to attempt ICE for this account -- `ice_enabled` override if
    /// set, otherwise the process-wide `global_default` (`AppConfig::ice_enabled`).
    pub fn wants_ice(&self, global_default: bool) -> bool {
        self.ice_enabled.unwrap_or(global_default)
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

fn update_check_frequency_to_str(f: UpdateCheckFrequency) -> &'static str {
    match f {
        UpdateCheckFrequency::Always => "always",
        UpdateCheckFrequency::Daily => "daily",
        UpdateCheckFrequency::Weekly => "weekly",
        UpdateCheckFrequency::Never => "never",
    }
}
fn update_check_frequency_from_str(s: &str) -> UpdateCheckFrequency {
    match s {
        "daily" => UpdateCheckFrequency::Daily,
        "weekly" => UpdateCheckFrequency::Weekly,
        "never" => UpdateCheckFrequency::Never,
        _ => UpdateCheckFrequency::Always,
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

fn recording_format_to_str(f: RecordingFormat) -> &'static str {
    match f {
        RecordingFormat::Wav => "wav",
        RecordingFormat::Mp3 => "mp3",
    }
}
fn recording_format_from_str(s: &str) -> RecordingFormat {
    match s {
        "mp3" => RecordingFormat::Mp3,
        _ => RecordingFormat::Wav,
    }
}

// ── Audio config ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// cpal device name to capture from; `None` uses the system default.
    /// Falls back to the default with a warning if the named device isn't found.
    pub input_device: Option<String>,
    /// cpal device name to play back to; `None` uses the system default.
    /// Falls back to the default with a warning if the named device isn't found.
    pub output_device: Option<String>,
    /// Not currently used — audio is always captured/played at 8 kHz.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// Not currently used — RTP frames are always 20ms.
    #[serde(default = "default_frame_ms")]
    pub frame_size_ms: u32,
    /// Acoustic echo cancellation. Off by default — only useful when using
    /// speakers/mic instead of a headset, where the mic picks up the call's
    /// own audio playing back out of the speaker.
    #[serde(default)]
    pub echo_cancellation: bool,
    /// cpal output device name to play the ringtone/ringback through;
    /// `None` uses the system default. Independent of `output_device` (the
    /// in-call audio device) so you can e.g. ring on PC speakers while
    /// talking through a headset -- same idiom, separate setting.
    #[serde(default)]
    pub ringtone_device: Option<String>,
    /// Path to a WAV file to play as the *incoming* ringtone instead of the
    /// synthesized two-tone cadence. Outgoing ringback is never customized
    /// this way. `None` (or a file that fails to load) falls back to the
    /// built-in tone.
    #[serde(default)]
    pub ringtone_file: Option<String>,
    /// Linear gain applied to ringtone/ringback playback via
    /// `rodio::Sink::set_volume` -- `1.0` is unchanged/full volume.
    #[serde(default = "default_ringtone_volume")]
    pub ringtone_volume: f32,
    /// Adaptive microphone gain control (see `deelip_media::agc`) -- boosts
    /// a quiet mic signal toward a target level and limits a loud one, with
    /// a hard clip-safety ceiling. Off by default, like echo cancellation.
    #[serde(default)]
    pub agc_enabled: bool,
}

fn default_sample_rate() -> u32 {
    48_000
}
fn default_frame_ms() -> u32 {
    20
}
fn default_ringtone_volume() -> f32 {
    1.0
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            sample_rate: default_sample_rate(),
            frame_size_ms: default_frame_ms(),
            echo_cancellation: false,
            ringtone_device: None,
            ringtone_file: None,
            ringtone_volume: default_ringtone_volume(),
            agc_enabled: false,
        }
    }
}

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// All configured accounts; every one with `enabled = true` is registered
    /// simultaneously on its own independent SIP stack.
    #[serde(default)]
    pub accounts: Vec<SipAccount>,
    #[serde(default)]
    pub audio: AudioConfig,
    /// Local port for the first enabled account's SIP stack; each
    /// subsequent enabled account binds `local_sip_port + N` (one UDP/TCP
    /// bind can't serve two independent stacks at once).
    #[serde(default = "default_sip_port")]
    pub local_sip_port: u16,
    /// Optional STUN server for NAT traversal, e.g. "stun.l.google.com:19302".
    pub stun_server: Option<String>,
    /// Optional TURN server: when set, ALL calls relay their RTP through it
    /// (no ICE candidate negotiation — an explicit, unconditional fallback for
    /// NATs that STUN alone can't traverse), e.g. "turn.example.com:3478".
    pub turn_server: Option<String>,
    pub turn_username: Option<String>,
    pub turn_password: Option<String>,

    /// UI appearance/behavior toggles below — unlike accounts/audio/network,
    /// these apply immediately (no restart needed) since they don't touch
    /// any running SipStack/MediaEngine state.
    #[serde(default = "default_true")]
    pub dark_mode: bool,
    #[serde(default = "default_true")]
    pub notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub ringtone_enabled: bool,

    /// Record every call under `recordings_dir()` (or `recordings_dir_override`
    /// if set). Off by default (opt-in, like echo cancellation) —
    /// restart-required, since it's baked into `MediaEngine::start` like the
    /// other audio settings.
    #[serde(default)]
    pub recording_enabled: bool,
    /// Recording output format -- see `RecordingFormat`. Restart-required,
    /// same as `recording_enabled`.
    #[serde(default)]
    pub recording_format: RecordingFormat,
    /// Directory to save call recordings in, instead of the default
    /// `~/.config/deelip/recordings` -- see `deelip_config::recordings_dir`.
    /// Restart-required, same as `recording_enabled`.
    #[serde(default)]
    pub recordings_dir_override: Option<String>,
    /// Start the main window hidden (only the tray icon visible) — restart-required.
    #[serde(default)]
    pub start_minimized: bool,
    /// Also write logs to `deelip_config::log_file_path()` (`~/.config/deelip/deelip.log`),
    /// in addition to the console -- read once at process startup (before
    /// this config value would otherwise be available), so it's
    /// restart-required like every other logging-adjacent setting.
    #[serde(default)]
    pub log_to_file: bool,

    /// Callers to auto-reject with 486 Busy Here before ringing, regardless of
    /// which account they call in on. Entries are matched against an incoming
    /// call's From URI by user-part (see `extract_user_part` in `deelip-ui`) —
    /// a bare number or a full `sip:`/`sips:` URI both work as entries.
    /// Applies immediately (no restart needed): read straight from config at
    /// decision time, not baked into any spawned SipStack/MediaEngine state.
    #[serde(default)]
    pub blocklist: Vec<String>,

    /// Attempt full ICE (RFC 8445) candidate gathering/connectivity checks
    /// for outgoing/incoming calls, falling back to the plain STUN-reflexive-
    /// or-TURN-unconditional path (see `stun_server`/`turn_server` above) if
    /// gathering fails or times out. Off by default (opt-in, like echo
    /// cancellation/recording) — read fresh per call, not restart-required,
    /// but not "instant" in the dark-mode sense either since it only takes
    /// effect on the next call placed/answered, not any call in progress.
    #[serde(default)]
    pub ice_enabled: bool,

    /// Enable system-wide Answer/Hangup/Mute hotkeys that work even when
    /// DeeLip isn't focused (Linux support is X11-only, same constraint as
    /// the main window itself being forced onto X11/XWayland). Off by
    /// default; registration happens once at startup, so changing this or
    /// any binding below requires a restart to take effect.
    #[serde(default)]
    pub global_hotkeys_enabled: bool,
    #[serde(default = "default_hotkey_answer")]
    pub hotkey_answer: String,
    #[serde(default = "default_hotkey_hangup")]
    pub hotkey_hangup: String,
    #[serde(default = "default_hotkey_mute")]
    pub hotkey_mute: String,

    /// If true, a new version found at startup (see `deelip-updater`) is
    /// downloaded and installed automatically -- only takes effect if the
    /// running binary is a self-updatable (tar.gz/`install.sh`) install;
    /// a system `.deb`/`.rpm` package always just shows the notification
    /// regardless of this toggle. Off by default (opt-in, like recording/
    /// ICE/global hotkeys above) -- applies immediately, no restart needed.
    #[serde(default)]
    pub auto_update_enabled: bool,
    /// Version the user explicitly dismissed ("Skip this version") in the
    /// update popup, so it doesn't keep nagging about the same release
    /// every launch. Cleared implicitly once a newer version supersedes it.
    #[serde(default)]
    pub update_skip_version: Option<String>,
    /// How often to automatically check for updates at startup -- the
    /// Settings tab's "Check for updates now" button always runs
    /// regardless of this. `Always` (every launch) is the default,
    /// preserving DeeLip's original behavior from before this setting
    /// existed.
    #[serde(default)]
    pub update_check_frequency: UpdateCheckFrequency,
    /// Unix timestamp of the last update check, automatic or manual --
    /// compared against `update_check_frequency` to decide whether a new
    /// automatic check is due at this startup (a manual check resets this
    /// too, so it counts toward postponing the next automatic one).
    #[serde(default)]
    pub last_update_check: Option<u64>,
}

fn default_hotkey_answer() -> String {
    "Ctrl+Alt+A".into()
}
fn default_hotkey_hangup() -> String {
    "Ctrl+Alt+H".into()
}
fn default_hotkey_mute() -> String {
    "Ctrl+Alt+M".into()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            accounts: vec![SipAccount::default()],
            audio: AudioConfig::default(),
            local_sip_port: 5060,
            stun_server: Some("stun.l.google.com:19302".into()),
            turn_server: None,
            turn_username: None,
            turn_password: None,
            dark_mode: true,
            notifications_enabled: true,
            ringtone_enabled: true,
            recording_enabled: false,
            recording_format: RecordingFormat::default(),
            recordings_dir_override: None,
            start_minimized: false,
            log_to_file: false,
            blocklist: Vec::new(),
            ice_enabled: false,
            global_hotkeys_enabled: false,
            hotkey_answer: default_hotkey_answer(),
            hotkey_hangup: default_hotkey_hangup(),
            hotkey_mute: default_hotkey_mute(),
            auto_update_enabled: false,
            update_skip_version: None,
            update_check_frequency: UpdateCheckFrequency::default(),
            last_update_check: None,
        }
    }
}

impl AppConfig {
    pub fn load(db: &Db) -> anyhow::Result<Self> {
        let get = |key: &str| db.get_setting(key);
        let get_bool =
            |key: &str, default: bool| get(key).map(|v| sql_to_bool(&v)).unwrap_or(default);
        let get_u32 =
            |key: &str, default: u32| get(key).and_then(|v| v.parse().ok()).unwrap_or(default);
        let get_f32 =
            |key: &str, default: f32| get(key).and_then(|v| v.parse().ok()).unwrap_or(default);

        let mut stmt = db.conn.prepare(
            "SELECT username, password, server, port, display_name, transport, enabled, \
                    tls_insecure_skip_verify, no_answer_forward, no_answer_timeout_secs, dnd, \
                    forward_always, forward_on_busy, codec_order, dtmf_mode, auto_answer_enabled, \
                    auto_answer_secs, mailbox, account_name, sip_proxy, domain, auth_username, \
                    dialing_prefix, hide_caller_id, register_expires, keepalive_secs, \
                    media_encryption, public_address, ice_enabled, force_incoming_codec, \
                    vad_enabled \
             FROM accounts ORDER BY sort_order",
        )?;
        let accounts = stmt
            .query_map([], |row| {
                let codec_order_json: String = row.get(13)?;
                let transport_str: String = row.get(5)?;
                let dtmf_mode_str: String = row.get(14)?;
                let media_encryption_str: String = row.get(26)?;
                let ice_enabled: Option<i64> = row.get(28)?;
                Ok(SipAccount {
                    username: row.get(0)?,
                    password: row.get(1)?,
                    server: row.get(2)?,
                    port: row.get(3)?,
                    display_name: row.get(4)?,
                    transport: transport_from_str(&transport_str),
                    enabled: sql_int_to_bool(row.get(6)?),
                    tls_insecure_skip_verify: sql_int_to_bool(row.get(7)?),
                    no_answer_forward: row.get(8)?,
                    no_answer_timeout_secs: row.get(9)?,
                    dnd: sql_int_to_bool(row.get(10)?),
                    forward_always: row.get(11)?,
                    forward_on_busy: row.get(12)?,
                    codec_order: serde_json::from_str(&codec_order_json)
                        .unwrap_or_else(|_| default_codec_order()),
                    dtmf_mode: dtmf_mode_from_str(&dtmf_mode_str),
                    auto_answer_enabled: sql_int_to_bool(row.get(15)?),
                    auto_answer_secs: row.get(16)?,
                    mailbox: row.get(17)?,
                    account_name: row.get(18)?,
                    sip_proxy: row.get(19)?,
                    domain: row.get(20)?,
                    auth_username: row.get(21)?,
                    dialing_prefix: row.get(22)?,
                    hide_caller_id: sql_int_to_bool(row.get(23)?),
                    register_expires: row.get(24)?,
                    keepalive_secs: row.get(25)?,
                    media_encryption: media_encryption_from_str(&media_encryption_str),
                    public_address: row.get(27)?,
                    ice_enabled: ice_enabled.map(sql_int_to_bool),
                    force_incoming_codec: row.get(29)?,
                    vad_enabled: sql_int_to_bool(row.get(30)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading accounts from database")?;

        Ok(AppConfig {
            accounts,
            audio: AudioConfig {
                input_device: get("audio.input_device"),
                output_device: get("audio.output_device"),
                sample_rate: get_u32("audio.sample_rate", default_sample_rate()),
                frame_size_ms: get_u32("audio.frame_size_ms", default_frame_ms()),
                echo_cancellation: get_bool("audio.echo_cancellation", false),
                ringtone_device: get("audio.ringtone_device"),
                ringtone_file: get("audio.ringtone_file"),
                ringtone_volume: get_f32("audio.ringtone_volume", default_ringtone_volume()),
                agc_enabled: get_bool("audio.agc_enabled", false),
            },
            local_sip_port: get_u32("local_sip_port", default_sip_port() as u32) as u16,
            stun_server: get("stun_server"),
            turn_server: get("turn_server"),
            turn_username: get("turn_username"),
            turn_password: get("turn_password"),
            dark_mode: get_bool("dark_mode", true),
            notifications_enabled: get_bool("notifications_enabled", true),
            ringtone_enabled: get_bool("ringtone_enabled", true),
            recording_enabled: get_bool("recording_enabled", false),
            recording_format: get("recording_format")
                .as_deref()
                .map(recording_format_from_str)
                .unwrap_or_default(),
            recordings_dir_override: get("recordings_dir_override"),
            start_minimized: get_bool("start_minimized", false),
            log_to_file: get_bool("log_to_file", false),
            blocklist: get("blocklist")
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
            ice_enabled: get_bool("ice_enabled", false),
            global_hotkeys_enabled: get_bool("global_hotkeys_enabled", false),
            hotkey_answer: get("hotkey_answer").unwrap_or_else(default_hotkey_answer),
            hotkey_hangup: get("hotkey_hangup").unwrap_or_else(default_hotkey_hangup),
            hotkey_mute: get("hotkey_mute").unwrap_or_else(default_hotkey_mute),
            auto_update_enabled: get_bool("auto_update_enabled", false),
            update_skip_version: get("update_skip_version"),
            update_check_frequency: get("update_check_frequency")
                .as_deref()
                .map(update_check_frequency_from_str)
                .unwrap_or_default(),
            last_update_check: get("last_update_check").and_then(|v| v.parse().ok()),
        })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.conn
            .execute("DELETE FROM accounts", [])
            .context("Clearing accounts table")?;
        for (i, acc) in self.accounts.iter().enumerate() {
            db.conn.execute(
                "INSERT INTO accounts (sort_order, username, password, server, port, display_name, \
                    transport, enabled, tls_insecure_skip_verify, no_answer_forward, \
                    no_answer_timeout_secs, dnd, forward_always, forward_on_busy, codec_order, \
                    dtmf_mode, auto_answer_enabled, auto_answer_secs, mailbox, account_name, \
                    sip_proxy, domain, auth_username, dialing_prefix, hide_caller_id, \
                    register_expires, keepalive_secs, media_encryption, public_address, \
                    ice_enabled, force_incoming_codec, vad_enabled) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
                    ?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31)",
                rusqlite::params![
                    i as i64,
                    acc.username,
                    acc.password,
                    acc.server,
                    acc.port,
                    acc.display_name,
                    transport_to_str(&acc.transport),
                    bool_to_sql(acc.enabled),
                    bool_to_sql(acc.tls_insecure_skip_verify),
                    acc.no_answer_forward,
                    acc.no_answer_timeout_secs,
                    bool_to_sql(acc.dnd),
                    acc.forward_always,
                    acc.forward_on_busy,
                    serde_json::to_string(&acc.codec_order)?,
                    dtmf_mode_to_str(acc.dtmf_mode),
                    bool_to_sql(acc.auto_answer_enabled),
                    acc.auto_answer_secs,
                    acc.mailbox,
                    acc.account_name,
                    acc.sip_proxy,
                    acc.domain,
                    acc.auth_username,
                    acc.dialing_prefix,
                    bool_to_sql(acc.hide_caller_id),
                    acc.register_expires,
                    acc.keepalive_secs,
                    media_encryption_to_str(acc.media_encryption),
                    acc.public_address,
                    acc.ice_enabled.map(bool_to_sql),
                    acc.force_incoming_codec,
                    bool_to_sql(acc.vad_enabled),
                ],
            ).with_context(|| format!("Inserting account {}", acc.username))?;
        }

        db.set_setting_opt("audio.input_device", &self.audio.input_device)?;
        db.set_setting_opt("audio.output_device", &self.audio.output_device)?;
        db.set_setting("audio.sample_rate", &self.audio.sample_rate.to_string())?;
        db.set_setting("audio.frame_size_ms", &self.audio.frame_size_ms.to_string())?;
        db.set_setting(
            "audio.echo_cancellation",
            bool_to_sql(self.audio.echo_cancellation),
        )?;
        db.set_setting_opt("audio.ringtone_device", &self.audio.ringtone_device)?;
        db.set_setting_opt("audio.ringtone_file", &self.audio.ringtone_file)?;
        db.set_setting("audio.ringtone_volume", &self.audio.ringtone_volume.to_string())?;
        db.set_setting("audio.agc_enabled", bool_to_sql(self.audio.agc_enabled))?;

        db.set_setting("local_sip_port", &self.local_sip_port.to_string())?;
        db.set_setting_opt("stun_server", &self.stun_server)?;
        db.set_setting_opt("turn_server", &self.turn_server)?;
        db.set_setting_opt("turn_username", &self.turn_username)?;
        db.set_setting_opt("turn_password", &self.turn_password)?;
        db.set_setting("dark_mode", bool_to_sql(self.dark_mode))?;
        db.set_setting(
            "notifications_enabled",
            bool_to_sql(self.notifications_enabled),
        )?;
        db.set_setting("ringtone_enabled", bool_to_sql(self.ringtone_enabled))?;
        db.set_setting("recording_enabled", bool_to_sql(self.recording_enabled))?;
        db.set_setting("recording_format", recording_format_to_str(self.recording_format))?;
        db.set_setting_opt("recordings_dir_override", &self.recordings_dir_override)?;
        db.set_setting("start_minimized", bool_to_sql(self.start_minimized))?;
        db.set_setting("log_to_file", bool_to_sql(self.log_to_file))?;
        db.set_setting("blocklist", &serde_json::to_string(&self.blocklist)?)?;
        db.set_setting("ice_enabled", bool_to_sql(self.ice_enabled))?;
        db.set_setting(
            "global_hotkeys_enabled",
            bool_to_sql(self.global_hotkeys_enabled),
        )?;
        db.set_setting("hotkey_answer", &self.hotkey_answer)?;
        db.set_setting("hotkey_hangup", &self.hotkey_hangup)?;
        db.set_setting("hotkey_mute", &self.hotkey_mute)?;
        db.set_setting("auto_update_enabled", bool_to_sql(self.auto_update_enabled))?;
        db.set_setting_opt("update_skip_version", &self.update_skip_version)?;
        db.set_setting(
            "update_check_frequency",
            update_check_frequency_to_str(self.update_check_frequency),
        )?;
        db.set_setting_opt(
            "last_update_check",
            &self.last_update_check.map(|v| v.to_string()),
        )?;
        Ok(())
    }
}
