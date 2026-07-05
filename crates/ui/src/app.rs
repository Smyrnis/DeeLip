use std::collections::HashMap;

use deelip_config::{AppConfig, CallDirection, CallHistory, CallStatus, Contact, ContactBook, Db, MessageLog, SipAccount};
use deelip_media::MediaEngine;
use deelip_sip::{AudioCodec, MwiState, PresenceState, SipHandle, SrtpParams};
use tokio::runtime::Handle;

use deelip_nat::{IceConnection, IceGathered, TurnRelay};

use crate::platform::hotkeys::Hotkeys;
use crate::platform::tray::{self, CtxSlot, QuitState};
use crate::platform::ringtone::Ringtone;
use crate::theme::Palette;

// ── Tab navigation ────────────────────────────────────────────────────────────

#[derive(PartialEq, Eq, Clone, Copy, Default)]
pub(crate) enum Tab { #[default] Dialer, History, Messages, Contacts, Settings }

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DeelipApp {
    /// One registered SIP identity per enabled account in `config.accounts`,
    /// each independently registering/re-registering on its own transport.
    pub(crate) accounts: Vec<AccountState>,
    pub(crate) rt:  Handle,

    pub(crate) tab: Tab,

    // Dialer
    pub(crate) call_target: String,
    /// Index into `accounts` — which identity new outgoing calls are placed
    /// from. Irrelevant (and hidden in the UI) when there's only one account.
    pub(crate) selected_account: usize,
    /// Last successfully-dialed target (already normalized), for Redial.
    pub(crate) last_dialed: Option<String>,

    // Status
    pub(crate) status_line: String,
    pub(crate) reg_ok:      bool,

    /// Confirmed (connected) calls — capped at 2 (one focused + one held),
    /// matching a simple "call waiting" model rather than arbitrary
    /// multi-call/conferencing. A 3rd simultaneous incoming call is
    /// auto-rejected with 486 Busy.
    pub(crate) calls: Vec<CallSlot>,
    /// Index into `calls` currently bound to `media` (the only call with a
    /// live mic/speaker — cpal only opens one set of device streams at a
    /// time). `None` means every call in `calls` is held.
    pub(crate) focused_call: Option<usize>,
    pub(crate) media: Option<MediaEngine>,
    /// Not-yet-answered outgoing call (between `make_call` and `CallConnected`/
    /// `CallFailed`) — dialing a 2nd number is blocked while this is `Some`.
    pub(crate) pending_outbound: Option<PendingOutbound>,
    /// Not-yet-answered incoming call — either the only call ringing, or a
    /// "call waiting" second call while `calls` is non-empty (distinguished
    /// in the UI, not in this struct).
    pub(crate) pending_call: Option<PendingCall>,

    /// Inline blind-transfer box state for the focused call.
    pub(crate) transfer_target:  String,
    pub(crate) showing_transfer: bool,
    /// Inline attended-transfer box state for the focused call — mirrors
    /// `transfer_target`/`showing_transfer` exactly.
    pub(crate) attended_target:  String,
    pub(crate) showing_attended: bool,
    /// Whether the in-call screen's DTMF keypad is expanded -- hidden by
    /// default so the focused-call screen stays uncluttered, matching the
    /// redesign's "reveal on demand" treatment of secondary controls.
    pub(crate) showing_dtmf: bool,
    /// Index into `calls` of the call being attended-transferred, set when
    /// its consultation call is dialed. `None` means no attended transfer
    /// is in progress. Cleared by `remove_call` whenever either leg ends,
    /// since both must still exist for Complete Transfer to make sense.
    pub(crate) attended_transfer_original: Option<usize>,
    /// Both calls in `calls` are bridged into one local 3-way mix. While
    /// true, `focused_call` is just an arbitrary "media is running" marker
    /// (always `Some(0)`) rather than meaning "only this one is active" --
    /// both slots are simultaneously un-held.
    pub(crate) in_conference: bool,

    /// Live while a call is ringing (incoming) or dialing out (outgoing) —
    /// see `sync_ringtone`. `None` whenever neither applies.
    pub(crate) ringtone: Option<Ringtone>,
    /// Whether something was ringing/dialing as of last frame — used to
    /// attempt `Ringtone::start` only once per ringing episode (on the
    /// false→true edge), not on every frame a failed start left `ringtone`
    /// as `None` (that retried the audio backend 20x/sec on any real device
    /// failure — the ALSA/jack probe spam this was fixed after).
    pub(crate) was_ringing: bool,
    /// The `call_id` last notified about, so `sync_notifications` fires once
    /// per incoming call rather than every frame it's still ringing.
    pub(crate) last_notified_call: Option<String>,

    /// Live-edited settings draft, shown/edited in the Settings tab and
    /// saved to `db` on demand — takes effect on next restart.
    pub(crate) config: AppConfig,
    /// Handle to `~/.config/deelip/deelip.db` -- the single SQLite database
    /// backing `config`/`contacts`/`history` alike (see `deelip_config::db`).
    pub(crate) db: Db,
    /// Set after a successful Settings save; cleared on the next edit.
    pub(crate) settings_saved_notice: bool,
    /// Index into `config.accounts` currently shown in the Settings tab's
    /// Account section (distinct from `selected_account`, which picks which
    /// *running/registered* identity places outgoing calls).
    pub(crate) edit_account_idx: usize,
    /// Cached (input, output) cpal device names for the Settings tab's
    /// device pickers. Populated lazily on first render and via an explicit
    /// Refresh button only -- calling cpal's device enumeration every frame
    /// (egui repaints continuously) hammered every ALSA/jack backend dozens
    /// of times a second, producing log spam and a real UI slowdown.
    pub(crate) audio_device_cache: Option<(Vec<String>, Vec<String>)>,
    /// Mirrors whether `~/.config/autostart/deelip.desktop` currently exists
    /// -- a separate on-disk file, not part of `config.toml`, so it needs
    /// its own bit of UI-bound state (checked once at startup, then kept in
    /// sync by the Settings checkbox itself).
    pub(crate) autostart_enabled: bool,
    /// One-shot flag consumed on the very first `update()` call, to send a
    /// `Visible(false)` viewport command if `config.start_minimized` -- see
    /// the comment in `main.rs` on why this can't be done via `NativeOptions`.
    pub(crate) first_frame: bool,
    /// Refreshed once per frame from `config.dark_mode` in `update()`, before
    /// any tab is rendered -- lets tab-rendering methods reach `self.palette`
    /// without threading an extra parameter through every fn signature.
    pub(crate) palette: Palette,

    /// Shared handles for the tray's independent event-handling threads (see
    /// `tray` module docs) — `None` degrades to normal close-quits-the-app
    /// behavior if the tray icon failed to start.
    pub(crate) tray: Option<(CtxSlot, QuitState, tray::BadgeSender)>,
    /// Missed calls not yet acknowledged by opening the History tab —
    /// mirrored to the tray icon's badge (see `sync_tray_badge`) whenever
    /// it changes; reset to 0 on switching to the History tab.
    pub(crate) unseen_missed_calls: u32,

    /// System-wide Answer/Hangup/Mute hotkeys (see `hotkeys` module docs) --
    /// `None` if disabled in config, or if registration failed (logged, not
    /// fatal — the app works fine without global hotkeys).
    pub(crate) hotkeys: Option<Hotkeys>,

    // History
    pub(crate) history:      CallHistory,
    pub(crate) history_search: String,
    /// `None` = show every status.
    pub(crate) history_status_filter: Option<CallStatus>,
    /// Cache of `history_search`/`history_status_filter`/`history.records.len()`
    /// as last used to compute `history_filtered`, so a search string that
    /// allocates a lowercased copy of every record's URI isn't redone on
    /// every single frame (egui repaints continuously, and much faster than
    /// that during a scroll drag) -- only recomputed when one of the three
    /// actually changes. Mirrors the existing `audio_device_cache` idiom.
    pub(crate) history_filter_key: Option<(String, Option<CallStatus>, usize)>,
    /// Indices into `history.records` matching the current search/status
    /// filter, most-recent-first (same order as `history.records` itself).
    pub(crate) history_filtered: Vec<usize>,

    // Messages
    pub(crate) messages: MessageLog,
    /// Unseen received messages -- mirrors `unseen_missed_calls`, reset to 0
    /// on switching to the Messages tab.
    pub(crate) unseen_messages: u32,
    /// Compose box state for the Messages tab.
    pub(crate) message_to:   String,
    pub(crate) message_body: String,

    // Blocklist
    pub(crate) blocklist_input: String,

    // Contacts
    pub(crate) contacts:       ContactBook,
    pub(crate) contact_search: String,
    pub(crate) new_contact:    Contact,
    /// Index into `contacts.contacts` currently loaded into `new_contact`
    /// for editing — `None` means the form is in "Add" mode.
    pub(crate) editing_contact_idx: Option<usize>,
    /// Last-known presence state per watched contact, keyed by `sip_uri`
    /// (presence isn't call-scoped, so it doesn't fit any per-call state).
    pub(crate) presence: HashMap<String, PresenceState>,
}

/// A not-yet-answered incoming call.
pub(crate) struct PendingCall {
    /// Index into `DeelipApp::accounts` — which identity this INVITE arrived on.
    pub(crate) account:    usize,
    pub(crate) call_id:    String,
    pub(crate) from:       String,
    pub(crate) remote_sdp: String,
    pub(crate) start_time: u64,
    /// (redirect deadline as a unix timestamp, forward-to URI) if the
    /// owning account has `no_answer_forward` configured.
    pub(crate) forward: Option<(u64, String)>,
    /// Unix timestamp at which to auto-answer, if the owning account has
    /// `auto_answer_enabled`. Independent of `forward` — whichever
    /// deadline is reached first wins (checked in the same per-frame poll).
    pub(crate) auto_answer_at: Option<u64>,
}

/// A not-yet-answered outgoing call — at most one at a time (placing a 2nd
/// outbound call is blocked while this is `Some`). Which account it's on
/// doesn't need to be stored here: `CallConnected`/`CallFailed` already carry
/// that as the account index tagged onto the event itself.
pub(crate) struct PendingOutbound {
    pub(crate) remote_uri: String,
    pub(crate) start_time: u64,
    pub(crate) local_rtp:  u16,
    pub(crate) local_srtp: Option<SrtpParams>,
    pub(crate) relay:      Option<TurnRelay>,
    /// Locally-gathered ICE candidates, if ICE was attempted for this call --
    /// connectivity checks against the remote's candidates only happen once
    /// their answer SDP arrives (see `SipEvent::CallConnected`), since that's
    /// the first point we know their ICE parameters.
    pub(crate) ice_gathered: Option<IceGathered>,
}

/// A confirmed (connected) call — held or focused. Only the focused call has
/// a live `MediaEngine`; a held call keeps just enough state here to restart
/// media (with a fresh SDP offer/answer) if the user swaps back to it.
pub(crate) struct CallSlot {
    pub(crate) account:    usize,
    pub(crate) call_id:    String,
    pub(crate) remote_uri: String,
    pub(crate) direction:  CallDirection,
    pub(crate) start_time: u64,
    pub(crate) is_held:    bool,
    pub(crate) codec:      AudioCodec,
    pub(crate) dtmf_type:  Option<u8>,
    pub(crate) local_srtp: Option<SrtpParams>,
    pub(crate) relay:      Option<TurnRelay>,
    /// The winning ICE connection, if ICE was used for this call — `None`
    /// for a plain direct/TURN-relayed call, same as `relay`. Deliberately
    /// not carried across into conference mode (`start_conference` keeps
    /// using `relay` only) or attended-transfer's consultation call, per
    /// this feature's scoped-to-basic-calls design.
    pub(crate) ice:        Option<IceConnection>,
    pub(crate) local_rtp:  u16,
    /// Last known remote SDP — reused to restart media on resume (the
    /// negotiated RTP endpoint doesn't change between hold and resume).
    pub(crate) remote_sdp: String,
}

/// A single registered SIP identity: its stack handle plus the registration
/// status shown next to it in the account picker.
pub(crate) struct AccountState {
    pub(crate) handle: SipHandle,
    /// The account as spawned at startup — NOT the live Settings draft
    /// (which may have since diverged; settings are restart-required).
    pub(crate) account: SipAccount,
    /// Display label for pickers — `display_name` if set, else `user@server`.
    pub(crate) label:  String,
    pub(crate) reg_ok: bool,
    pub(crate) status: String,
    /// Last-known voicemail MWI state, if this account has `mailbox` set
    /// and a NOTIFY has arrived yet — `None` until then, or if unconfigured.
    pub(crate) mwi: Option<MwiState>,
}

impl DeelipApp {
    pub fn new(
        accounts: Vec<(SipAccount, SipHandle)>,
        rt: Handle,
        config: AppConfig,
        db: Db,
        tray: Option<(CtxSlot, QuitState, tray::BadgeSender)>,
    ) -> Self {
        let accounts = accounts.into_iter().map(|(account, handle)| AccountState {
            label: crate::helpers::account_label(&account),
            account,
            handle,
            reg_ok: false,
            status: "Registering…".into(),
            mwi: None,
        }).collect();

        let history = CallHistory::load(&db).unwrap_or_default();
        let contacts = ContactBook::load(&db).unwrap_or_default();
        let messages = MessageLog::load(&db).unwrap_or_default();

        let hotkeys = if config.global_hotkeys_enabled {
            match Hotkeys::spawn(&config.hotkey_answer, &config.hotkey_hangup, &config.hotkey_mute) {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::warn!("Global hotkeys failed to register ({e}), continuing without them");
                    None
                }
            }
        } else {
            None
        };

        Self {
            accounts,
            rt,
            tab:              Tab::Dialer,
            call_target:      String::new(),
            selected_account: 0,
            last_dialed:      None,
            status_line:      "Registering…".into(),
            reg_ok:           false,
            calls:            Vec::new(),
            focused_call:     None,
            media:            None,
            pending_outbound: None,
            pending_call:     None,
            transfer_target:  String::new(),
            showing_transfer: false,
            attended_target:  String::new(),
            showing_attended: false,
            showing_dtmf:     false,
            attended_transfer_original: None,
            in_conference: false,
            ringtone:            None,
            was_ringing:         false,
            last_notified_call:  None,
            config,
            db,
            settings_saved_notice: false,
            edit_account_idx: 0,
            audio_device_cache: None,
            autostart_enabled: deelip_config::is_autostart_enabled(),
            first_frame: true,
            palette: Palette::dark(),
            tray,
            unseen_missed_calls: 0,
            hotkeys,
            history,
            history_search:         String::new(),
            history_status_filter:  None,
            history_filter_key:     None,
            history_filtered:       Vec::new(),
            messages,
            unseen_messages: 0,
            message_to:   String::new(),
            message_body: String::new(),
            blocklist_input:        String::new(),
            contacts,
            contact_search:   String::new(),
            new_contact:      Contact::default(),
            editing_contact_idx: None,
            presence: HashMap::new(),
        }
    }
}
