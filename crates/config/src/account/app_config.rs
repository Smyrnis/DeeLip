//! `AppConfig`, the top-level persisted settings struct (everything outside
//! the per-account list), plus its `Default` and the ZRTP-identity helper.
//! `load`/`save` (the SQL marshaling) live in `db.rs`, not here -- unrelated
//! concern, see that file's own module doc.

use serde::{Deserialize, Serialize};

use super::enums::{DefaultListAction, Language, RecordingFormat, UpdateCheckFrequency};
use super::sip_account::{default_sip_port, default_true, AudioConfig, SipAccount};
use crate::Db;

pub(super) fn default_ldap_port() -> u16 {
    389
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
    /// Restrict local RTP port allocation to this inclusive min–max range
    /// (e.g. for a firewall/NAT port-forward), instead of an OS-assigned
    /// ephemeral port every call -- see `deelip_sip::media_setup::NetworkConfig`.
    /// Both must be set for the range to take effect (see `rtp_port_range()`).
    #[serde(default)]
    pub rtp_port_min: Option<u16>,
    #[serde(default)]
    pub rtp_port_max: Option<u16>,
    /// Override DNS server ("ip" or "ip:port") for resolving account server
    /// hosts (and SRV records, if `dns_srv_enabled`) instead of the system
    /// resolver -- see `deelip_sip::wire::dns`. Unset: system resolver,
    /// today's behavior.
    #[serde(default)]
    pub custom_nameserver: Option<String>,
    /// Attempt SIP SRV-record (RFC 3263) service discovery for each
    /// account's server host before falling back to a plain A/AAAA lookup.
    /// Off by default, like every other opt-in networking toggle here.
    #[serde(default)]
    pub dns_srv_enabled: bool,
    /// Force single-call-only behavior: an incoming call while any account
    /// already has an active call is rejected with 486 Busy Here instead of
    /// ringing as a call-waiting second call. Applies process-wide (all
    /// accounts), unlike the per-account `forward_on_busy`, which -- when
    /// set on the account being called -- still takes priority over this.
    /// Off by default, preserving today's "up to 2 concurrent calls" behavior.
    #[serde(default)]
    pub single_call_mode: bool,

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
    /// Save a local crash-report file (`deelip_config::crashes_dir()`,
    /// `~/.config/deelip/crashes/`) if the process panics -- timestamp,
    /// version, panic message/location, and a backtrace. Purely local:
    /// never uploaded or transmitted anywhere, there's no remote endpoint at
    /// all. On by default *unlike* every other opt-in toggle here, since
    /// there's no privacy cost to weigh (nothing leaves the machine) and a
    /// crash report is only useful if it was already enabled *before* the
    /// crash happened -- read once at startup to install the panic hook, so
    /// it's restart-required like every other logging-adjacent setting.
    #[serde(default = "default_true")]
    pub crash_reporting_enabled: bool,

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
    /// Answer/hang up via a hardware headset/multimedia "hook" button
    /// (`global_hotkey`'s `Code::MediaPlayPause`, mapped to the real
    /// `XF86AudioPlay` keysym on X11 via the same mechanism as the custom
    /// hotkeys above) -- independent of `global_hotkeys_enabled` (a user
    /// may want one without the other). Off by default; restart-required,
    /// same as the custom hotkeys.
    #[serde(default)]
    pub handle_media_buttons: bool,

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
    /// What double-clicking a row's name/number in History/Contacts does --
    /// see `DefaultListAction`. Applies immediately, no restart needed.
    #[serde(default)]
    pub default_list_action: DefaultListAction,
    /// UI display language -- see `Language`. Read once at startup by
    /// `deelip_ui::strings::init` to pick which `assets/locales/*.json` to
    /// load, so changing it is restart-required like other startup-only
    /// settings.
    #[serde(default)]
    pub language: Language,
    /// Show the main window at a randomized position on the current monitor
    /// each time it's raised for an incoming call, instead of wherever it
    /// last was -- see `deelip_ui::frame::sync_window_raise`. Off by
    /// default. Applies immediately, no restart needed.
    #[serde(default)]
    pub random_popup_position: bool,
    /// This installation's ZRTP identity (RFC 6189 ZID), 12 bytes hex-encoded
    /// -- generated once on first use (`zrtp_zid_bytes`) and persisted from
    /// then on, shared by every account's ZRTP calls (see
    /// `SipAccount::wants_zrtp`). `None` until the first ZRTP call is ever
    /// attempted.
    #[serde(default)]
    pub zrtp_zid: Option<String>,

    /// Corporate/LDAP directory server host, e.g. "ldap.example.com" --
    /// presence of a value is what enables the Directory tab (same "an
    /// `Option` field's presence *is* the toggle" idiom as `mailbox`/
    /// `forward_always` elsewhere in this file), rather than a separate
    /// enabled flag. `None`/empty: Directory tab shows a "configure this in
    /// Settings" prompt instead of a search box.
    #[serde(default)]
    pub ldap_server: Option<String>,
    #[serde(default = "default_ldap_port")]
    pub ldap_port: u16,
    /// Connect via `ldaps://` (implicit TLS) instead of plain `ldap://`.
    /// Off by default, like every other opt-in toggle here -- turn on
    /// explicitly for a directory server that requires/expects it.
    #[serde(default)]
    pub ldap_use_tls: bool,
    /// Search base DN, e.g. "dc=example,dc=com". Required for search to do
    /// anything meaningful; an empty value searches the server's root DSE,
    /// which is never what's wanted here.
    #[serde(default)]
    pub ldap_base_dn: Option<String>,
    /// Bind DN for authenticating before searching, e.g.
    /// "cn=readonly,dc=example,dc=com". Empty/unset: anonymous bind, which
    /// many directories reject for search -- most deployments will need this set.
    #[serde(default)]
    pub ldap_bind_dn: Option<String>,
    #[serde(default)]
    pub ldap_bind_password: Option<String>,
    /// Search filter template (RFC 4515) with `{query}` substituted for the
    /// user's typed search text (already LDAP-escaped -- see
    /// `views::directory::escape_ldap_filter`) -- e.g.
    /// "(|(cn=*{query}*)(mail=*{query}*))". Empty/unset falls back to a
    /// built-in default matching `cn`/`displayName`/`mail`/`sn`/`givenName`
    /// against common `inetOrgPerson`/Active-Directory-style schemas.
    #[serde(default)]
    pub ldap_search_filter: Option<String>,
}

pub(super) fn default_hotkey_answer() -> String {
    "Ctrl+Alt+A".into()
}
pub(super) fn default_hotkey_hangup() -> String {
    "Ctrl+Alt+H".into()
}
pub(super) fn default_hotkey_mute() -> String {
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
            rtp_port_min: None,
            rtp_port_max: None,
            custom_nameserver: None,
            dns_srv_enabled: false,
            single_call_mode: false,
            dark_mode: true,
            notifications_enabled: true,
            ringtone_enabled: true,
            recording_enabled: false,
            recording_format: RecordingFormat::default(),
            recordings_dir_override: None,
            start_minimized: false,
            log_to_file: false,
            crash_reporting_enabled: true,
            blocklist: Vec::new(),
            ice_enabled: false,
            global_hotkeys_enabled: false,
            hotkey_answer: default_hotkey_answer(),
            hotkey_hangup: default_hotkey_hangup(),
            hotkey_mute: default_hotkey_mute(),
            handle_media_buttons: false,
            auto_update_enabled: false,
            update_skip_version: None,
            update_check_frequency: UpdateCheckFrequency::default(),
            last_update_check: None,
            default_list_action: DefaultListAction::default(),
            language: Language::default(),
            random_popup_position: false,
            zrtp_zid: None,
            ldap_server: None,
            ldap_port: default_ldap_port(),
            ldap_use_tls: false,
            ldap_base_dn: None,
            ldap_bind_dn: None,
            ldap_bind_password: None,
            ldap_search_filter: None,
        }
    }
}

impl AppConfig {
    /// This installation's persistent ZRTP identity, generating and saving
    /// one on first use if `zrtp_zid` is unset. Returns the raw 12 bytes;
    /// `db` is only touched when a fresh ZID actually needs saving.
    pub fn zrtp_zid_bytes(&mut self, db: &Db) -> anyhow::Result<[u8; 12]> {
        if let Some(hex) = &self.zrtp_zid {
            if let Some(bytes) = parse_zid_hex(hex) {
                return Ok(bytes);
            }
        }
        let mut bytes = [0u8; 12];
        rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
        self.zrtp_zid = Some(bytes.iter().map(|b| format!("{b:02x}")).collect());
        db.set_setting_opt("zrtp_zid", &self.zrtp_zid)?;
        Ok(bytes)
    }
}

fn parse_zid_hex(hex: &str) -> Option<[u8; 12]> {
    if hex.len() != 24 {
        return None;
    }
    let mut bytes = [0u8; 12];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}
