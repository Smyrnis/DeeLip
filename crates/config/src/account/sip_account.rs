//! `SipAccount` (one configured SIP identity) and `AudioConfig` (the
//! process-wide audio device/processing settings) -- the two structs a user
//! actually edits in Settings, plus their field-level defaults.

use serde::{Deserialize, Serialize};

use super::enums::{DtmfMode, MediaEncryption, TransportProtocol};
use crate::dialplan::DialPlanRule;

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
    /// React to a remote `Call-Info: ...;answer-after=N` signal (an
    /// intercom/paging-hardware convention, distinct from the always-on
    /// timer-based `auto_answer_enabled` above) by auto-answering after N
    /// seconds -- an "Auto Answer (Control Button)" behavior. Ignored unless
    /// the incoming INVITE actually carries the header; off by default.
    #[serde(default)]
    pub auto_answer_control_button: bool,
    /// Mirror of `auto_answer_control_button`: react to the same remote
    /// signal by immediately rejecting the call instead -- a "Deny Incoming
    /// (Control Button)" behavior. Takes priority if both are somehow on.
    /// Ignored unless the incoming INVITE carries the header; off by default.
    #[serde(default)]
    pub deny_incoming_control_button: bool,
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
    /// today's behavior. Only used as a fallback when no `dial_plan` rule
    /// matches (or the list is empty) -- see `apply_dial_plan`.
    #[serde(default)]
    pub dialing_prefix: Option<String>,
    /// Ordered regex match/replace rules applied to a bare (non-URI) dialed
    /// number before `dialing_prefix`'s simpler auto-prepend -- see
    /// `apply_dial_plan`. Empty by default: no rules, today's
    /// prefix-only behavior.
    #[serde(default)]
    pub dial_plan: Vec<DialPlanRule>,
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
    /// Rewrite the advertised Contact/SDP IP based on the `received=` param
    /// the registrar's REGISTER response reports on our own `Via:` header --
    /// i.e. what the server actually saw as our source IP, which can differ
    /// from our locally-known IP behind NAT. A self-discovery alternative
    /// to a separate STUN server, re-checked on every (re-)registration.
    /// Ignored while `public_address` is set (an explicit override always
    /// wins). Off by default.
    #[serde(default)]
    pub allow_ip_rewrite: bool,
    /// Publish this account's own presence status (RFC 3903 PUBLISH,
    /// `Event: presence`) as `open`/`closed` depending on `dnd` -- distinct
    /// from watching *others'* presence (`Contact::watch_presence`), which
    /// DeeLip already supported. Off by default: needs a server with a
    /// presence agent willing to accept PUBLISH, which not every PBX has.
    #[serde(default)]
    pub publish_presence: bool,
    /// Per-account override for whether to attempt ICE (RFC 8445) --
    /// `None` follows the global `AppConfig::ice_enabled` toggle (today's
    /// behavior); `Some(true)`/`Some(false)` force it on/off for just this
    /// account regardless of the global setting.
    #[serde(default)]
    pub ice_enabled: Option<bool>,
    /// RFC 4028 Session Timers -- periodic re-INVITE keep-alives so either
    /// side (or a stateful proxy in between) can detect and clean up a
    /// dialog whose signaling path died without a BYE ever arriving. On by
    /// default once implemented, matching every real UA's default; this is
    /// the "Disable Session Timers" checkbox (inverted) in the account
    /// editor.
    #[serde(default = "default_true")]
    pub session_timers_enabled: bool,
    /// A serverless, direct-IP "Local Account" calling mode. Off by default.
    /// Full picture (why UDP-only, how outgoing calls resolve a
    /// destination): `docs/crates/config.md`.
    #[serde(default)]
    pub local_account: bool,
    /// Attempt to negotiate a video leg (H.264) alongside audio on this
    /// account's calls, and (once negotiated) actually capture/encode/send/
    /// decode/render it -- see `docs/crates/media-engine.md` for the full video
    /// pipeline. Off by default like every other opt-in toggle here.
    #[serde(default)]
    pub video_enabled: bool,
}

pub(super) fn default_sip_port() -> u16 {
    5060
}
pub(super) fn default_true() -> bool {
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
pub(super) fn default_codec_order() -> Vec<String> {
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
            auto_answer_control_button: false,
            deny_incoming_control_button: false,
            mailbox: None,
            account_name: None,
            sip_proxy: None,
            domain: None,
            auth_username: None,
            dialing_prefix: None,
            dial_plan: Vec::new(),
            hide_caller_id: false,
            register_expires: default_register_expires(),
            keepalive_secs: None,
            media_encryption: MediaEncryption::default(),
            public_address: None,
            ice_enabled: None,
            allow_ip_rewrite: false,
            publish_presence: false,
            session_timers_enabled: true,
            local_account: false,
            video_enabled: false,
        }
    }
}

impl SipAccount {
    /// SIP domain used in From/To/Contact/Request-URIs -- `domain` if set,
    /// otherwise `server` (the common case: registrar and domain are the
    /// same host).
    pub fn domain(&self) -> &str {
        self.domain.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&self.server)
    }

    /// Digest-auth username -- `auth_username` if set, otherwise `username`.
    pub fn auth_username(&self) -> &str {
        self.auth_username.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&self.username)
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
            // ZRTP negotiates its own SRTP keys in-band -- never via SDES.
            MediaEncryption::Zrtp => false,
        }
    }

    /// Whether to attempt RFC 6189 ZRTP key agreement on this account's
    /// calls -- see `MediaEncryption::Zrtp`.
    pub fn wants_zrtp(&self) -> bool {
        self.media_encryption == MediaEncryption::Zrtp
    }

    /// Whether to attempt ICE for this account -- `ice_enabled` override if
    /// set, otherwise the process-wide `global_default` (`AppConfig::ice_enabled`).
    pub fn wants_ice(&self, global_default: bool) -> bool {
        self.ice_enabled.unwrap_or(global_default)
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
    /// `nokhwa` camera human-readable name (as `video_capture::list_cameras()`
    /// returns) to capture video calls from; `None` uses the first
    /// enumerated camera. Same by-name persistence idiom as the audio
    /// device fields above -- resolved back to a `CameraIndex` via
    /// `video_capture::find_camera_by_name` at call-start time.
    #[serde(default)]
    pub camera_device: Option<String>,
    /// Camera capture width/height in pixels -- must be an even number in
    /// both dimensions (I420's chroma planes are half-resolution). Restart
    /// required, same as `camera_device`. Defaults match the values every
    /// video call used before this became configurable.
    #[serde(default = "default_video_capture_width")]
    pub video_capture_width: u32,
    #[serde(default = "default_video_capture_height")]
    pub video_capture_height: u32,
    /// Target encode/capture frame rate.
    #[serde(default = "default_video_fps")]
    pub video_fps: u32,
    /// Target H.264 encode bitrate, in bits per second.
    #[serde(default = "default_video_bitrate_bps")]
    pub video_bitrate_bps: u32,
}

pub(super) fn default_sample_rate() -> u32 {
    48_000
}
pub(super) fn default_frame_ms() -> u32 {
    20
}
pub(super) fn default_ringtone_volume() -> f32 {
    1.0
}
pub(super) fn default_video_capture_width() -> u32 {
    640
}
pub(super) fn default_video_capture_height() -> u32 {
    480
}
pub(super) fn default_video_fps() -> u32 {
    15
}
pub(super) fn default_video_bitrate_bps() -> u32 {
    500_000
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
            camera_device: None,
            video_capture_width: default_video_capture_width(),
            video_capture_height: default_video_capture_height(),
            video_fps: default_video_fps(),
            video_bitrate_bps: default_video_bitrate_bps(),
        }
    }
}
