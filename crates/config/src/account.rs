use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::db::{bool_to_sql, sql_int_to_bool, sql_to_bool};
use crate::Db;

// ── Transport protocol ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransportProtocol {
    #[default]
    Udp,
    Tcp,
    Tls,
}

fn transport_to_str(t: &TransportProtocol) -> &'static str {
    match t {
        TransportProtocol::Udp => "udp",
        TransportProtocol::Tcp => "tcp",
        TransportProtocol::Tls => "tls",
    }
}
fn transport_from_str(s: &str) -> TransportProtocol {
    match s {
        "tcp" => TransportProtocol::Tcp,
        "tls" => TransportProtocol::Tls,
        _     => TransportProtocol::Udp,
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
        DtmfMode::Inband  => "inband",
    }
}
fn dtmf_mode_from_str(s: &str) -> DtmfMode {
    match s {
        "sipinfo" => DtmfMode::SipInfo,
        "inband"  => DtmfMode::Inband,
        _         => DtmfMode::Rfc2833,
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
}

fn default_sip_port() -> u16 { 5060 }
fn default_true() -> bool { true }
fn default_no_answer_timeout() -> u32 { 20 }
fn default_auto_answer_secs() -> u32 { 3 }
fn default_codec_order() -> Vec<String> {
    ["opus", "g722", "pcmu", "pcma"].map(String::from).to_vec()
}

impl Default for SipAccount {
    fn default() -> Self {
        Self {
            username:     "your_username".into(),
            password:     "your_password".into(),
            server:       "your.sip.server".into(),
            port:         5060,
            display_name: Some("Your Name".into()),
            transport:    TransportProtocol::Udp,
            enabled:      true,
            tls_insecure_skip_verify: false,
            no_answer_forward: None,
            no_answer_timeout_secs: default_no_answer_timeout(),
            dnd: false,
            forward_always: None,
            forward_on_busy: None,
            codec_order: default_codec_order(),
            dtmf_mode: DtmfMode::default(),
            auto_answer_enabled: false,
            auto_answer_secs: default_auto_answer_secs(),
            mailbox: None,
        }
    }
}

// ── Audio config ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// cpal device name to capture from; `None` uses the system default.
    /// Falls back to the default with a warning if the named device isn't found.
    pub input_device:  Option<String>,
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
}

fn default_sample_rate() -> u32 { 48_000 }
fn default_frame_ms()    -> u32 { 20 }

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device:  None,
            output_device: None,
            sample_rate:   default_sample_rate(),
            frame_size_ms: default_frame_ms(),
            echo_cancellation: false,
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
    pub turn_server:   Option<String>,
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

    /// Record every call to a stereo WAV file under `recordings_dir()`. Off
    /// by default (opt-in, like echo cancellation) — restart-required, since
    /// it's baked into `MediaEngine::start` like the other audio settings.
    #[serde(default)]
    pub recording_enabled: bool,
    /// Start the main window hidden (only the tray icon visible) — restart-required.
    #[serde(default)]
    pub start_minimized: bool,

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
}

fn default_hotkey_answer() -> String { "Ctrl+Alt+A".into() }
fn default_hotkey_hangup() -> String { "Ctrl+Alt+H".into() }
fn default_hotkey_mute()   -> String { "Ctrl+Alt+M".into() }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            accounts:       vec![SipAccount::default()],
            audio:          AudioConfig::default(),
            local_sip_port: 5060,
            stun_server:    Some("stun.l.google.com:19302".into()),
            turn_server:    None,
            turn_username:  None,
            turn_password:  None,
            dark_mode:              true,
            notifications_enabled: true,
            ringtone_enabled:      true,
            recording_enabled:     false,
            start_minimized:       false,
            blocklist:             Vec::new(),
            ice_enabled:           false,
            global_hotkeys_enabled: false,
            hotkey_answer: default_hotkey_answer(),
            hotkey_hangup: default_hotkey_hangup(),
            hotkey_mute:   default_hotkey_mute(),
        }
    }
}

impl AppConfig {
    pub fn load(db: &Db) -> anyhow::Result<Self> {
        let get = |key: &str| db.get_setting(key);
        let get_bool = |key: &str, default: bool| get(key).map(|v| sql_to_bool(&v)).unwrap_or(default);
        let get_u32 = |key: &str, default: u32| get(key).and_then(|v| v.parse().ok()).unwrap_or(default);

        let mut stmt = db.conn.prepare(
            "SELECT username, password, server, port, display_name, transport, enabled, \
                    tls_insecure_skip_verify, no_answer_forward, no_answer_timeout_secs, dnd, \
                    forward_always, forward_on_busy, codec_order, dtmf_mode, auto_answer_enabled, \
                    auto_answer_secs, mailbox \
             FROM accounts ORDER BY sort_order",
        )?;
        let accounts = stmt
            .query_map([], |row| {
                let codec_order_json: String = row.get(13)?;
                let transport_str: String = row.get(5)?;
                let dtmf_mode_str: String = row.get(14)?;
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
                    codec_order: serde_json::from_str(&codec_order_json).unwrap_or_else(|_| default_codec_order()),
                    dtmf_mode: dtmf_mode_from_str(&dtmf_mode_str),
                    auto_answer_enabled: sql_int_to_bool(row.get(15)?),
                    auto_answer_secs: row.get(16)?,
                    mailbox: row.get(17)?,
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
            start_minimized: get_bool("start_minimized", false),
            blocklist: get("blocklist").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default(),
            ice_enabled: get_bool("ice_enabled", false),
            global_hotkeys_enabled: get_bool("global_hotkeys_enabled", false),
            hotkey_answer: get("hotkey_answer").unwrap_or_else(default_hotkey_answer),
            hotkey_hangup: get("hotkey_hangup").unwrap_or_else(default_hotkey_hangup),
            hotkey_mute: get("hotkey_mute").unwrap_or_else(default_hotkey_mute),
        })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.conn.execute("DELETE FROM accounts", []).context("Clearing accounts table")?;
        for (i, acc) in self.accounts.iter().enumerate() {
            db.conn.execute(
                "INSERT INTO accounts (sort_order, username, password, server, port, display_name, \
                    transport, enabled, tls_insecure_skip_verify, no_answer_forward, \
                    no_answer_timeout_secs, dnd, forward_always, forward_on_busy, codec_order, \
                    dtmf_mode, auto_answer_enabled, auto_answer_secs, mailbox) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
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
                ],
            ).with_context(|| format!("Inserting account {}", acc.username))?;
        }

        db.set_setting_opt("audio.input_device", &self.audio.input_device)?;
        db.set_setting_opt("audio.output_device", &self.audio.output_device)?;
        db.set_setting("audio.sample_rate", &self.audio.sample_rate.to_string())?;
        db.set_setting("audio.frame_size_ms", &self.audio.frame_size_ms.to_string())?;
        db.set_setting("audio.echo_cancellation", bool_to_sql(self.audio.echo_cancellation))?;

        db.set_setting("local_sip_port", &self.local_sip_port.to_string())?;
        db.set_setting_opt("stun_server", &self.stun_server)?;
        db.set_setting_opt("turn_server", &self.turn_server)?;
        db.set_setting_opt("turn_username", &self.turn_username)?;
        db.set_setting_opt("turn_password", &self.turn_password)?;
        db.set_setting("dark_mode", bool_to_sql(self.dark_mode))?;
        db.set_setting("notifications_enabled", bool_to_sql(self.notifications_enabled))?;
        db.set_setting("ringtone_enabled", bool_to_sql(self.ringtone_enabled))?;
        db.set_setting("recording_enabled", bool_to_sql(self.recording_enabled))?;
        db.set_setting("start_minimized", bool_to_sql(self.start_minimized))?;
        db.set_setting("blocklist", &serde_json::to_string(&self.blocklist)?)?;
        db.set_setting("ice_enabled", bool_to_sql(self.ice_enabled))?;
        db.set_setting("global_hotkeys_enabled", bool_to_sql(self.global_hotkeys_enabled))?;
        db.set_setting("hotkey_answer", &self.hotkey_answer)?;
        db.set_setting("hotkey_hangup", &self.hotkey_hangup)?;
        db.set_setting("hotkey_mute", &self.hotkey_mute)?;
        Ok(())
    }
}
