use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

// ── Transport protocol ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransportProtocol {
    #[default]
    Udp,
    Tcp,
    Tls,
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
}

fn default_sip_port() -> u16 { 5060 }
fn default_true() -> bool { true }
fn default_no_answer_timeout() -> u32 { 20 }

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
}

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
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Reading config from {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("Parsing config at {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating config dir {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self).context("Serialising config")?;
        std::fs::write(path, raw)
            .with_context(|| format!("Writing config to {}", path.display()))
    }
}

/// Returns `~/.config/deelip/config.toml`.
pub fn default_config_path() -> anyhow::Result<PathBuf> {
    Ok(deelip_dir()?.join("config.toml"))
}

fn deelip_dir() -> anyhow::Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine user config directory"))?;
    Ok(base.join("deelip"))
}

/// Returns `~/.config/deelip/recordings`, creating it if it doesn't exist yet.
pub fn recordings_dir() -> anyhow::Result<PathBuf> {
    let dir = deelip_dir()?.join("recordings");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Creating recordings dir {}", dir.display()))?;
    Ok(dir)
}

/// `~/.config/autostart/deelip.desktop` — the standard freedesktop.org XDG
/// autostart path, honored by GNOME/KDE/XFCE alike without needing a
/// systemd unit.
fn autostart_desktop_path() -> anyhow::Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine user config directory"))?;
    Ok(base.join("autostart").join("deelip.desktop"))
}

pub fn is_autostart_enabled() -> bool {
    autostart_desktop_path().is_ok_and(|p| p.exists())
}

/// Write or remove the XDG autostart `.desktop` file. Takes effect on next
/// login; has no effect on the currently running process.
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let path = autostart_desktop_path()?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("Removing {}", path.display()))?;
        }
        return Ok(());
    }

    let exe = std::env::current_exe().context("Resolving current executable path")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("Creating {}", parent.display()))?;
    }
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=DeeLip\nExec={}\nX-GNOME-Autostart-enabled=true\n",
        exe.display(),
    );
    std::fs::write(&path, contents).with_context(|| format!("Writing {}", path.display()))
}

// ── Contact book ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Contact {
    pub name:    String,
    pub sip_uri: String,
    /// Subscribe to this contact's SIP presence (RFC 3856), shown as a
    /// colored dot in the Contacts tab. Off by default -- opt-in, like the
    /// other watch/enable toggles in this config.
    #[serde(default)]
    pub watch_presence: bool,
    /// Which account's identity subscribes on this contact's behalf,
    /// identified by username (stable across account reordering, unlike an
    /// index). `None` defaults to the first enabled account.
    #[serde(default)]
    pub presence_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContactBook {
    pub contacts: Vec<Contact>,
}

impl ContactBook {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Reading contacts from {}", path.display()))?;
        serde_json::from_str(&raw).context("Parsing contacts JSON")
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self).context("Serialising contacts")?;
        std::fs::write(path, raw)
            .with_context(|| format!("Writing contacts to {}", path.display()))
    }

    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(deelip_dir()?.join("contacts.json"))
    }

    /// Contacts whose name or URI contains `query` (case-insensitive), paired
    /// with their index in `self.contacts` so callers can edit/delete them.
    pub fn search<'a>(&'a self, query: &str) -> Vec<(usize, &'a Contact)> {
        let q = query.to_lowercase();
        self.contacts
            .iter()
            .enumerate()
            .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q) || c.sip_uri.to_lowercase().contains(&q))
            .collect()
    }
}

// ── Call history ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallStatus {
    Answered,
    Missed,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub remote_uri:    String,
    pub direction:     CallDirection,
    /// Unix timestamp (seconds) when the call was initiated/received.
    pub timestamp:     u64,
    /// Duration in seconds; 0 for unanswered calls.
    pub duration_secs: u32,
    pub status:        CallStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallHistory {
    pub records: Vec<CallRecord>,
}

impl CallHistory {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Reading history from {}", path.display()))?;
        serde_json::from_str(&raw).context("Parsing history JSON")
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self).context("Serialising history")?;
        std::fs::write(path, raw)
            .with_context(|| format!("Writing history to {}", path.display()))
    }

    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(deelip_dir()?.join("history.json"))
    }

    /// Prepend a record, keeping at most 200 entries.
    pub fn push(&mut self, record: CallRecord) {
        self.records.insert(0, record);
        self.records.truncate(200);
    }
}
