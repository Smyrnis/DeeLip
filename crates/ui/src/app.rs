use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use deelip_config::{
    AppConfig, CallDirection, CallHistory, CallStatus, Contact, ContactBook, Db, MessageLog, SipAccount,
};
use deelip_media::MediaEngine;
use deelip_sip::{CallMediaReady, MwiState, PresenceState, SipHandle};
use tokio::runtime::Handle;

use crate::platform::hotkeys::Hotkeys;
use crate::platform::ringtone::Ringtone;
use crate::platform::tray::{self, CtxSlot, QuitState};
use crate::strings::t;
use crate::theme::Palette;

// ── Tab navigation ────────────────────────────────────────────────────────────

#[derive(PartialEq, Eq, Clone, Copy, Default)]
pub(crate) enum Tab {
    #[default]
    Dialer,
    History,
    Contacts,
    Directory,
}

/// Which Settings tab is currently shown -- MicroSIP-style tabbed dialog
/// (one section visible at a time, sized to fit without scrolling) rather
/// than the earlier single long scrolling stack of 12 sections. Grouped
/// down from those 12 section methods to 8 tabs -- some sections (a lone
/// checkbox) weren't worth their own tab.
#[derive(PartialEq, Eq, Clone, Copy, Default, Debug)]
pub(crate) enum SettingsTab {
    #[default]
    General,
    Account,
    Audio,
    Video,
    Network,
    Directory,
    Hotkeys,
    Advanced,
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DeelipApp {
    /// One registered SIP identity per enabled account in `config.accounts`,
    /// each independently registering/re-registering on its own transport.
    pub(crate) accounts: Vec<AccountState>,
    pub(crate) rt: Handle,

    pub(crate) tab: Tab,
    /// Whether the Settings dialog is open -- MicroSIP-style separate modal
    /// window rather than a tab, since a settings screen the size of this
    /// one competing for tab-bar space with Dialer/History/etc. read as
    /// "just another view" rather than the distinct, out-of-the-way
    /// configuration surface MicroSIP's own Settings dialog is.
    pub(crate) settings_open: bool,
    /// Which Settings tab is currently shown -- see `SettingsTab`.
    pub(crate) settings_tab: SettingsTab,

    // Dialer
    pub(crate) call_target: String,
    /// Index into `accounts` — which identity new outgoing calls are placed
    /// from. Irrelevant (and hidden in the UI) when there's only one account.
    pub(crate) selected_account: usize,
    /// Last successfully-dialed target (already normalized), for Redial.
    pub(crate) last_dialed: Option<String>,

    // Status
    pub(crate) status_line: String,
    pub(crate) reg_ok: bool,

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
    /// Video counterpart of `media` -- `None` whenever the focused call has
    /// no negotiated video leg (every call today unless
    /// `SipAccount::video_enabled`), or while none is focused.
    pub(crate) video: Option<VideoCallState>,
    /// Not-yet-answered outgoing call (between `make_call` and `CallConnected`/
    /// `CallFailed`) — dialing a 2nd number is blocked while this is `Some`.
    pub(crate) pending_outbound: Option<PendingOutbound>,
    /// Not-yet-answered incoming call — either the only call ringing, or a
    /// "call waiting" second call while `calls` is non-empty (distinguished
    /// in the UI, not in this struct).
    pub(crate) pending_call: Option<PendingCall>,
    /// An incoming call we've sent `AcceptCall` for but haven't yet gotten
    /// `SipEvent::CallConnected` back on -- media negotiation (codec/SRTP/
    /// ICE/TURN) now happens inside `SipStack` itself, so there's a real gap
    /// between "we told it to accept" and "media is ready to start". Kept
    /// separate from `pending_outbound` (rather than reusing one slot for
    /// both directions) since an inbound ring can in principle arrive while
    /// an outbound dial is still in flight, and both would then be waiting
    /// on their own `CallConnected` simultaneously.
    pub(crate) pending_accept: Option<PendingAccept>,

    /// Inline blind-transfer box state for the focused call.
    pub(crate) transfer_target: String,
    pub(crate) showing_transfer: bool,
    /// Inline attended-transfer box state for the focused call — mirrors
    /// `transfer_target`/`showing_transfer` exactly.
    pub(crate) attended_target: String,
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
    /// Same idiom as `last_notified_call`, for `sync_window_raise` -- kept
    /// as a separate field since window-raising isn't gated on
    /// `notifications_enabled` and so can't share the same edge tracking.
    pub(crate) last_raised_call: Option<String>,

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
    /// Reveal-toggle for the account editor's password field -- purely
    /// local UI state, reset to masked whenever a different account is
    /// selected would be nice but isn't worth the extra bookkeeping; it's
    /// low-stakes since the password itself never leaves this process.
    pub(crate) show_account_password: bool,
    /// Cached (input, output) cpal device names for the Settings tab's
    /// device pickers. Populated lazily on first render and via an explicit
    /// Refresh button only -- calling cpal's device enumeration every frame
    /// (egui repaints continuously) hammered every ALSA/jack backend dozens
    /// of times a second, producing log spam and a real UI slowdown.
    pub(crate) audio_device_cache: Option<(Vec<String>, Vec<String>)>,
    /// Set while a background enumeration kicked off by `show_audio_section`
    /// is in flight -- drained (like `event_rx` elsewhere) on the next frame
    /// it resolves. Needed because cpal enumeration itself -- not just
    /// running it every frame -- can block the calling thread for hundreds
    /// of ms on some backends (measured ~620ms on first Audio-tab visit,
    /// live via PulseAudio), and that thread is the same one driving both
    /// the main window and the Settings viewport, so a synchronous call
    /// here froze the whole app for that long right as the user switched
    /// tabs. Runs once, not every frame -- see `audio_device_cache`.
    pub(crate) audio_device_rx: Option<std::sync::mpsc::Receiver<(Vec<String>, Vec<String>)>>,
    /// Same idiom as `audio_device_cache`, for the Settings tab's camera
    /// picker -- `nokhwa` enumeration is likewise too expensive to run
    /// every frame.
    pub(crate) camera_device_cache: Option<Vec<String>>,
    /// Same idiom as `audio_device_rx`, for camera enumeration.
    pub(crate) camera_device_rx: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Mirrors whether `~/.config/autostart/deelip.desktop` currently exists
    /// -- a separate on-disk file, not part of `config.toml`, so it needs
    /// its own bit of UI-bound state (checked once at startup, then kept in
    /// sync by the Settings checkbox itself).
    pub(crate) autostart_enabled: bool,
    /// One-shot flag consumed on the very first `update()` call, to send a
    /// `Visible(false)` viewport command if `config.start_minimized` -- see
    /// the comment in `main.rs` on why this can't be done via `NativeOptions`.
    pub(crate) first_frame: bool,
    /// Set once in `new()` -- Darcula is the app's only theme now (see
    /// `theme.rs`'s v3-revision doc comment), so this no longer changes per
    /// frame. Kept as a field (not a free fn call at each use site) so
    /// tab-rendering methods can reach `self.palette` without threading an
    /// extra parameter through every fn signature.
    pub(crate) palette: Palette,

    /// Shared handles for the tray's independent event-handling threads (see
    /// `tray` module docs) — `None` degrades to normal close-quits-the-app
    /// behavior if the tray icon failed to start.
    pub(crate) tray: Option<(CtxSlot, QuitState, tray::BadgeSender)>,
    /// Slot every background producer that isn't already covered by
    /// `tray`'s own copy (SIP events, global hotkeys, desktop-notification
    /// actions, the update checker, LDAP directory search, Settings'
    /// audio/camera device scans) uses to call `request_repaint()` the
    /// instant it has something, instead of DeeLip's idle repaint tick
    /// having to poll for it. Refreshed every frame in `update()`, same as
    /// `tray`'s copy was before this existed -- see `frame.rs`'s repaint-
    /// interval comment for why this matters: without it, the only way to
    /// notice a new event while idle was a periodic forced repaint of the
    /// *whole* window (including, while Settings is open, its own viewport),
    /// which is what caused the Settings-window slowdown this was added to
    /// fix.
    pub(crate) ctx_slot: CtxSlot,
    /// Missed calls not yet acknowledged by opening the History tab —
    /// mirrored to the tray icon's badge (see `sync_tray_badge`) whenever
    /// it changes; reset to 0 on switching to the History tab.
    pub(crate) unseen_missed_calls: u32,
    /// `(account, call_id)` for every entry in `calls`/`pending_call` as
    /// last mirrored into the tray's `QuitState` -- lets
    /// `process_tray_events` skip re-cloning Senders/call-ids and re-locking
    /// the shared state on every frame when nothing has actually changed
    /// since the last one. Mirrors `audio_device_cache`'s cache-and-compare
    /// idiom.
    pub(crate) tray_calls_key: Vec<(usize, String)>,
    pub(crate) tray_pending_key: Option<(usize, String)>,

    /// System-wide Answer/Hangup/Mute hotkeys (see `hotkeys` module docs) --
    /// `None` if disabled in config, or if registration failed (logged, not
    /// fatal — the app works fine without global hotkeys).
    pub(crate) hotkeys: Option<Hotkeys>,

    // History
    pub(crate) history: CallHistory,
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
    /// `(unseen_missed_calls, label)` as last rendered for the tab bar --
    /// recomputed only when the count changes, instead of `format!()`ing a
    /// fresh label every frame regardless. Same cache-and-compare idiom as
    /// `history_filter_key`.
    pub(crate) history_tab_label_cache: (u32, String),

    // Messages
    pub(crate) messages: MessageLog,
    /// Whether the Messages window is open -- same separate-native-window
    /// pattern as `settings_open`, except there's no tab-bar entry point at
    /// all: the only way to set this `true` is `message_from_list` (a
    /// right-click "Message" action on a History/Contacts/Directory row).
    pub(crate) messages_window_open: bool,
    /// Which peer's thread the Messages window is showing -- always a full
    /// SIP URI sourced from a right-click target or an existing `peer_uri`,
    /// never freehand-typed (there's no more manual "To:" field). `None`
    /// only when the window has never been scoped to anyone yet.
    pub(crate) messages_window_peer: Option<String>,
    pub(crate) message_body: String,

    // Blocklist
    pub(crate) blocklist_input: String,

    // Dial Plan rule editor (Settings' account editor) -- new-rule input
    // fields, mirroring `blocklist_input`'s shape.
    pub(crate) dialplan_pattern_input: String,
    pub(crate) dialplan_replacement_input: String,

    // Contacts
    pub(crate) contacts: ContactBook,
    pub(crate) contact_search: String,
    pub(crate) new_contact: Contact,
    /// Index into `contacts.contacts` currently loaded into `new_contact`
    /// for editing — `None` means the form is in "Add" mode.
    pub(crate) editing_contact_idx: Option<usize>,
    /// Whether the shared create/edit contact `egui::Window` is open --
    /// set from either Contacts' "+" FAB or History's right-click "Add to
    /// Contacts", so it lives on `DeelipApp` (not local to one view) and is
    /// rendered from `frame.rs::update()` alongside the other modals.
    pub(crate) contact_dialog_open: bool,
    /// Last-known presence state per watched contact, keyed by `sip_uri`
    /// (presence isn't call-scoped, so it doesn't fit any per-call state).
    pub(crate) presence: HashMap<String, PresenceState>,

    /// Startup GitHub-release check / auto-update state (see `update.rs`).
    pub(crate) update_state: crate::update::UpdateState,
    /// Channel the background check/download thread reports back on --
    /// re-created (old one just dropped) each time a new one is spawned,
    /// same one-shot-channel-per-async-op idiom as elsewhere in this app.
    pub(crate) update_rx: Option<std::sync::mpsc::Receiver<crate::update::UpdateMsg>>,

    // Directory (LDAP) -- see `views::directory`.
    pub(crate) directory_query: String,
    pub(crate) directory_state: crate::views::directory::DirectoryState,
    pub(crate) directory_rx: Option<std::sync::mpsc::Receiver<crate::views::directory::DirectoryMsg>>,
}

/// Wraps `DeelipApp` behind a lock so Settings/Messages can render as real
/// independent (`Deferred`-class) viewports instead of ones nested inside
/// the main window's own per-tick callback (`Immediate`-class) -- see
/// `show_settings_modal`'s doc comment for why that nesting was the actual
/// cause of Settings feeling slow/laggy whenever the main window was
/// unfocused (GNOME/Mutter throttles frame delivery for whichever window
/// isn't focused, and an `Immediate` child viewport can only redraw when
/// its parent's own tick runs). `eframe::App` can't be implemented directly
/// on `Arc<Mutex<DeelipApp>>` (neither side is local to this crate), hence
/// this thin newtype -- `update`/`on_exit` just lock and delegate to
/// `DeelipApp::update_inner`/`on_exit_inner`.
///
/// Locking here is a borrow-checker/orphan-rule necessity, not a real
/// concurrency mechanism -- eframe's winit event loop is single-threaded,
/// and a `Deferred` viewport's callback is only ever invoked as a separate,
/// sequential event on that same thread (confirmed against `eframe`'s own
/// source), never nested inside another locked call to this `update`, so
/// there's no reentrant-locking/deadlock risk in practice.
#[derive(Clone)]
pub struct SharedApp(pub Arc<Mutex<DeelipApp>>);

// SAFETY: see the doc comment above -- the Mutex is a borrow-checker/
// orphan-rule necessity here, not a real cross-thread concurrency
// mechanism. eframe's Deferred-viewport callbacks and the root
// update()/on_exit() are all invoked from the same single winit event-
// loop thread, sequentially, never reentrantly -- confirmed against
// eframe 0.28.1's native/{glow,wgpu}_integration.rs. DeelipApp itself
// is !Send only because it transitively holds a cpal::Stream, which
// cpal marks !Send defensively for genuine cross-thread use it never
// sees here.
unsafe impl Send for SharedApp {}
unsafe impl Sync for SharedApp {}

impl SharedApp {
    /// A method (not a bare `.0.lock()` at the call site) so that a `move`
    /// closure calling this captures the whole `SharedApp` -- which carries
    /// the `unsafe impl Send`/`Sync` above -- rather than reaching straight
    /// through to the inner `Arc<Mutex<DeelipApp>>` field. Rust's 2021
    /// disjoint-closure-capture captures the minimal path actually used,
    /// so `self_app.0.lock()` inside a closure captures just that `!Send`
    /// field, silently missing this wrapper's unsafe impl.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, DeelipApp> {
        self.0.lock().unwrap()
    }
}

/// A not-yet-answered incoming call.
pub(crate) struct PendingCall {
    /// Index into `DeelipApp::accounts` — which identity this INVITE arrived on.
    pub(crate) account: usize,
    pub(crate) call_id: String,
    pub(crate) from: String,
    pub(crate) start_time: u64,
    /// (redirect deadline as a unix timestamp, forward-to URI) if the
    /// owning account has `no_answer_forward` configured.
    pub(crate) forward: Option<(u64, String)>,
    /// Unix timestamp at which to auto-answer, if the owning account has
    /// `auto_answer_enabled`. Independent of `forward` — whichever
    /// deadline is reached first wins (checked in the same per-frame poll).
    pub(crate) auto_answer_at: Option<u64>,
}

/// An incoming call we've sent `AcceptCall` for, awaiting `CallConnected`.
/// See `DeelipApp::pending_accept`'s doc comment.
pub(crate) struct PendingAccept {
    pub(crate) call_id: String,
    pub(crate) remote_uri: String,
    pub(crate) start_time: u64,
}

/// A not-yet-answered outgoing call — at most one at a time (placing a 2nd
/// outbound call is blocked while this is `Some`). Which account it's on
/// doesn't need to be stored here: `CallConnected`/`CallFailed` already carry
/// that as the account index tagged onto the event itself. SDP/codec/ICE/
/// TURN are entirely `SipStack`'s business now (see `deelip_sip::media_setup`)
/// — this just tracks enough to build history/`CallSlot` once `CallConnected`
/// arrives.
pub(crate) struct PendingOutbound {
    pub(crate) remote_uri: String,
    pub(crate) start_time: u64,
}

/// A confirmed (connected) call — held or focused. Only the focused call has
/// a live `MediaEngine`; a held call keeps just enough state here to restart
/// media if the user swaps back to it. `media` is the already-negotiated
/// state handed over by `SipStack` in `SipEvent::CallConnected` -- codec/
/// SRTP/ICE/TURN resolution all happened there, not here.
pub(crate) struct CallSlot {
    pub(crate) account: usize,
    pub(crate) call_id: String,
    pub(crate) remote_uri: String,
    pub(crate) direction: CallDirection,
    pub(crate) start_time: u64,
    pub(crate) is_held: bool,
    /// Whether `start_media` should start this call's `MediaEngine` with
    /// recording on -- seeded from the global `config.recording_enabled`
    /// when the call connects, but then tracks the user's own manual
    /// Record/Stop-recording toggle (`do_record_toggle`) from then on. A
    /// hold tears down and resume rebuilds a fresh `MediaEngine` (see
    /// `do_hold_slot`/`do_swap_to`), which used to mean `start_media` only
    /// ever consulted the global config again on resume -- silently
    /// re-enabling recording after the user had explicitly turned it off
    /// for this call, since nothing remembered that per-call override.
    pub(crate) recording_enabled: bool,
    pub(crate) media: CallMediaReady,
}

/// Live video state for the focused call, mirroring `MediaEngine`/`self.media`'s
/// placement -- only the focused call has a running `VideoEngine`. Bundles
/// the engine, an optional camera capture handle (`None` if no camera was
/// available -- video still receives/displays the remote side in that
/// case, see `media.rs::start_video`), and per-side cached-frame+texture
/// state so `dialer.rs`'s per-repaint render doesn't reconvert/re-upload a
/// YUV420 frame that hasn't changed since the last one.
pub(crate) struct VideoCallState {
    pub(crate) engine: deelip_media::video_engine::VideoEngine,
    pub(crate) camera: Option<deelip_media::video_capture::CaptureHandle>,
    pub(crate) remote: VideoViewCache,
    pub(crate) local: VideoViewCache,
}

#[derive(Default)]
pub(crate) struct VideoViewCache {
    pub(crate) frame: Option<deelip_media::video_codec::Yuv420Frame>,
    pub(crate) texture: Option<egui::TextureHandle>,
}

/// A single registered SIP identity: its stack handle plus the registration
/// status shown next to it in the account picker.
pub(crate) struct AccountState {
    pub(crate) handle: SipHandle,
    /// The account as spawned at startup — NOT the live Settings draft
    /// (which may have since diverged; settings are restart-required).
    pub(crate) account: SipAccount,
    /// Display label for pickers — `display_name` if set, else `user@server`.
    pub(crate) label: String,
    pub(crate) reg_ok: bool,
    pub(crate) status: String,
    /// Last-known voicemail MWI state, if this account has `mailbox` set
    /// and a NOTIFY has arrived yet — `None` until then, or if unconfigured.
    pub(crate) mwi: Option<MwiState>,
}

impl DeelipApp {
    pub fn new(
        accounts: Vec<(SipAccount, SipHandle)>, rt: Handle, config: AppConfig, db: Db,
        tray: Option<(CtxSlot, QuitState, tray::BadgeSender)>, ctx_slot: CtxSlot,
    ) -> Self {
        crate::strings::init(config.language);

        let accounts = accounts
            .into_iter()
            .map(|(account, handle)| AccountState {
                label: crate::helpers::account_label(&account),
                account,
                handle,
                reg_ok: false,
                status: t("status.registering"),
                mwi: None,
            })
            .collect();

        let history = CallHistory::load(&db).unwrap_or_default();
        let contacts = ContactBook::load(&db).unwrap_or_default();
        let messages = MessageLog::load(&db).unwrap_or_default();

        let hotkeys = if config.global_hotkeys_enabled || config.handle_media_buttons {
            let custom = config.global_hotkeys_enabled.then_some((
                config.hotkey_answer.as_str(),
                config.hotkey_hangup.as_str(),
                config.hotkey_mute.as_str(),
            ));
            match Hotkeys::spawn(custom, config.handle_media_buttons, ctx_slot.clone()) {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::warn!("Global hotkeys failed to register ({e}), continuing without them");
                    None
                }
            }
        } else {
            None
        };

        let mut app = Self {
            accounts,
            rt,
            tab: Tab::Dialer,
            settings_open: false,
            settings_tab: SettingsTab::default(),
            call_target: String::new(),
            selected_account: 0,
            last_dialed: None,
            status_line: t("status.registering"),
            reg_ok: false,
            calls: Vec::new(),
            focused_call: None,
            media: None,
            video: None,
            pending_outbound: None,
            pending_call: None,
            pending_accept: None,
            transfer_target: String::new(),
            showing_transfer: false,
            attended_target: String::new(),
            showing_attended: false,
            showing_dtmf: false,
            attended_transfer_original: None,
            in_conference: false,
            ringtone: None,
            was_ringing: false,
            last_notified_call: None,
            last_raised_call: None,
            config,
            db,
            settings_saved_notice: false,
            edit_account_idx: 0,
            show_account_password: false,
            audio_device_cache: None,
            audio_device_rx: None,
            camera_device_cache: None,
            camera_device_rx: None,
            autostart_enabled: deelip_config::is_autostart_enabled(),
            first_frame: true,
            palette: Palette::light(),
            tray,
            ctx_slot,
            unseen_missed_calls: 0,
            tray_calls_key: Vec::new(),
            tray_pending_key: None,
            hotkeys,
            history,
            history_search: String::new(),
            history_status_filter: None,
            history_filter_key: None,
            history_filtered: Vec::new(),
            // `u32::MAX` is a "never computed yet" sentinel, not a real
            // count -- guarantees the very first frame's mismatch check
            // computes the real label instead of leaving it empty.
            history_tab_label_cache: (u32::MAX, String::new()),
            messages,
            messages_window_open: false,
            messages_window_peer: None,
            message_body: String::new(),
            blocklist_input: String::new(),
            dialplan_pattern_input: String::new(),
            dialplan_replacement_input: String::new(),
            contacts,
            contact_search: String::new(),
            new_contact: Contact::default(),
            editing_contact_idx: None,
            contact_dialog_open: false,
            presence: HashMap::new(),
            update_state: crate::update::UpdateState::Idle,
            update_rx: None,
            directory_query: String::new(),
            directory_state: crate::views::directory::DirectoryState::default(),
            directory_rx: None,
        };

        let now = crate::helpers::unix_now();
        if app.config.update_check_frequency.is_due(app.config.last_update_check, now) {
            app.start_update_check();
        }
        app
    }
}
