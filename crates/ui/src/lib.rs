use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use deelip_config::{
    AppConfig, CallDirection, CallHistory, CallRecord, CallStatus,
    Contact, ContactBook, SipAccount, TransportProtocol,
};
use deelip_media::{alloc_rtp_port, MediaEngine};
use deelip_sip::{
    build_answer, build_hold_offer, build_offer, build_resume_offer,
    parse_sdp, AudioCodec, PresenceState, SipEvent, SipHandle, SrtpParams, SrtpSession,
};
use egui::{FontId, RichText, Ui};
use tokio::runtime::Handle;

use deelip_nat::TurnRelay;

pub mod tray;
use tray::{CtxSlot, QuitState};

mod notify;
mod ringtone;
use ringtone::{RingKind, Ringtone};

mod theme;
use theme::Palette;

/// Embedded Cantarell (GNOME's own default UI font, SIL OFL 1.1 -- see
/// `assets/OFL.txt`) as the app's proportional font, replacing egui's
/// built-in default; plus the Phosphor icon font for a coherent icon set
/// instead of ad hoc Unicode/emoji glyphs. Call once from the `eframe`
/// creation callback, before the app's first frame.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "cantarell".into(),
        egui::FontData::from_static(include_bytes!("../../../assets/Cantarell-VF.otf")),
    );
    fonts.families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "cantarell".into());

    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    ctx.set_fonts(fonts);
}

// ── Tab navigation ────────────────────────────────────────────────────────────

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Tab { #[default] Dialer, History, Contacts, Settings }

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DeelipApp {
    /// One registered SIP identity per enabled account in `config.accounts`,
    /// each independently registering/re-registering on its own transport.
    accounts: Vec<AccountState>,
    rt:  Handle,

    tab: Tab,

    // Dialer
    call_target: String,
    /// Index into `accounts` — which identity new outgoing calls are placed
    /// from. Irrelevant (and hidden in the UI) when there's only one account.
    selected_account: usize,
    /// Last successfully-dialed target (already normalized), for Redial.
    last_dialed: Option<String>,

    // Status
    status_line: String,
    reg_ok:      bool,

    /// Confirmed (connected) calls — capped at 2 (one focused + one held),
    /// matching a simple "call waiting" model rather than arbitrary
    /// multi-call/conferencing. A 3rd simultaneous incoming call is
    /// auto-rejected with 486 Busy.
    calls: Vec<CallSlot>,
    /// Index into `calls` currently bound to `media` (the only call with a
    /// live mic/speaker — cpal only opens one set of device streams at a
    /// time). `None` means every call in `calls` is held.
    focused_call: Option<usize>,
    media: Option<MediaEngine>,
    /// Not-yet-answered outgoing call (between `make_call` and `CallConnected`/
    /// `CallFailed`) — dialing a 2nd number is blocked while this is `Some`.
    pending_outbound: Option<PendingOutbound>,
    /// Not-yet-answered incoming call — either the only call ringing, or a
    /// "call waiting" second call while `calls` is non-empty (distinguished
    /// in the UI, not in this struct).
    pending_call: Option<PendingCall>,

    /// Inline blind-transfer box state for the focused call.
    transfer_target:  String,
    showing_transfer: bool,

    /// Live while a call is ringing (incoming) or dialing out (outgoing) —
    /// see `sync_ringtone`. `None` whenever neither applies.
    ringtone: Option<Ringtone>,
    /// Whether something was ringing/dialing as of last frame — used to
    /// attempt `Ringtone::start` only once per ringing episode (on the
    /// false→true edge), not on every frame a failed start left `ringtone`
    /// as `None` (that retried the audio backend 20x/sec on any real device
    /// failure — the ALSA/jack probe spam this was fixed after).
    was_ringing: bool,
    /// The `call_id` last notified about, so `sync_notifications` fires once
    /// per incoming call rather than every frame it's still ringing.
    last_notified_call: Option<String>,

    /// Live-edited settings draft, shown/edited in the Settings tab and
    /// saved to `config_path` on demand — takes effect on next restart.
    config: AppConfig,
    config_path: PathBuf,
    /// Set after a successful Settings save; cleared on the next edit.
    settings_saved_notice: bool,
    /// Index into `config.accounts` currently shown in the Settings tab's
    /// Account section (distinct from `selected_account`, which picks which
    /// *running/registered* identity places outgoing calls).
    edit_account_idx: usize,
    /// Cached (input, output) cpal device names for the Settings tab's
    /// device pickers. Populated lazily on first render and via an explicit
    /// Refresh button only -- calling cpal's device enumeration every frame
    /// (egui repaints continuously) hammered every ALSA/jack backend dozens
    /// of times a second, producing log spam and a real UI slowdown.
    audio_device_cache: Option<(Vec<String>, Vec<String>)>,
    /// Mirrors whether `~/.config/autostart/deelip.desktop` currently exists
    /// -- a separate on-disk file, not part of `config.toml`, so it needs
    /// its own bit of UI-bound state (checked once at startup, then kept in
    /// sync by the Settings checkbox itself).
    autostart_enabled: bool,
    /// One-shot flag consumed on the very first `update()` call, to send a
    /// `Visible(false)` viewport command if `config.start_minimized` -- see
    /// the comment in `main.rs` on why this can't be done via `NativeOptions`.
    first_frame: bool,
    /// Refreshed once per frame from `config.dark_mode` in `update()`, before
    /// any tab is rendered -- lets tab-rendering methods reach `self.palette`
    /// without threading an extra parameter through every fn signature.
    palette: Palette,

    /// Shared handles for the tray's independent event-handling threads (see
    /// `tray` module docs) — `None` degrades to normal close-quits-the-app
    /// behavior if the tray icon failed to start.
    tray: Option<(CtxSlot, QuitState)>,

    // History
    history:      CallHistory,
    history_path: Option<PathBuf>,
    history_search: String,
    /// `None` = show every status.
    history_status_filter: Option<CallStatus>,

    // Contacts
    contacts:       ContactBook,
    contacts_path:  Option<PathBuf>,
    contact_search: String,
    new_contact:    Contact,
    /// Index into `contacts.contacts` currently loaded into `new_contact`
    /// for editing — `None` means the form is in "Add" mode.
    editing_contact_idx: Option<usize>,
    /// Last-known presence state per watched contact, keyed by `sip_uri`
    /// (presence isn't call-scoped, so it doesn't fit any per-call state).
    presence: HashMap<String, PresenceState>,
}

/// A not-yet-answered incoming call.
struct PendingCall {
    /// Index into `DeelipApp::accounts` — which identity this INVITE arrived on.
    account:    usize,
    call_id:    String,
    from:       String,
    remote_sdp: String,
    start_time: u64,
    /// (redirect deadline as a unix timestamp, forward-to URI) if the
    /// owning account has `no_answer_forward` configured.
    forward: Option<(u64, String)>,
}

/// A not-yet-answered outgoing call — at most one at a time (placing a 2nd
/// outbound call is blocked while this is `Some`). Which account it's on
/// doesn't need to be stored here: `CallConnected`/`CallFailed` already carry
/// that as the account index tagged onto the event itself.
struct PendingOutbound {
    remote_uri: String,
    start_time: u64,
    local_rtp:  u16,
    local_srtp: Option<SrtpParams>,
    relay:      Option<TurnRelay>,
}

/// A confirmed (connected) call — held or focused. Only the focused call has
/// a live `MediaEngine`; a held call keeps just enough state here to restart
/// media (with a fresh SDP offer/answer) if the user swaps back to it.
struct CallSlot {
    account:    usize,
    call_id:    String,
    remote_uri: String,
    direction:  CallDirection,
    start_time: u64,
    is_held:    bool,
    codec:      AudioCodec,
    dtmf_type:  Option<u8>,
    local_srtp: Option<SrtpParams>,
    relay:      Option<TurnRelay>,
    local_rtp:  u16,
    /// Last known remote SDP — reused to restart media on resume (the
    /// negotiated RTP endpoint doesn't change between hold and resume).
    remote_sdp: String,
}

/// A single registered SIP identity: its stack handle plus the registration
/// status shown next to it in the account picker.
struct AccountState {
    handle: SipHandle,
    /// The account as spawned at startup — NOT the live Settings draft
    /// (which may have since diverged; settings are restart-required).
    account: SipAccount,
    /// Display label for pickers — `display_name` if set, else `user@server`.
    label:  String,
    reg_ok: bool,
    status: String,
}

impl DeelipApp {
    pub fn new(
        accounts: Vec<(SipAccount, SipHandle)>,
        rt: Handle,
        config: AppConfig,
        config_path: PathBuf,
        tray: Option<(CtxSlot, QuitState)>,
    ) -> Self {
        let accounts = accounts.into_iter().map(|(account, handle)| AccountState {
            label: account_label(&account),
            account,
            handle,
            reg_ok: false,
            status: "Registering…".into(),
        }).collect();

        let history_path = CallHistory::default_path().ok();
        let history = history_path.as_deref()
            .and_then(|p| CallHistory::load(p).ok())
            .unwrap_or_default();

        let contacts_path = ContactBook::default_path().ok();
        let contacts = contacts_path.as_deref()
            .and_then(|p| ContactBook::load(p).ok())
            .unwrap_or_default();

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
            ringtone:            None,
            was_ringing:         false,
            last_notified_call:  None,
            config,
            config_path,
            settings_saved_notice: false,
            edit_account_idx: 0,
            audio_device_cache: None,
            autostart_enabled: deelip_config::is_autostart_enabled(),
            first_frame: true,
            palette: Palette::dark(),
            tray,
            history,
            history_path,
            history_search:         String::new(),
            history_status_filter:  None,
            contacts,
            contacts_path,
            contact_search:   String::new(),
            new_contact:      Contact::default(),
            editing_contact_idx: None,
            presence: HashMap::new(),
        }
    }

    // ── SIP event processing ─────────────────────────────────────────────────

    /// Drain every account's event queue first, tagging each event with the
    /// account index it came from, then process them — a single loop can't
    /// borrow `self.accounts[i].handle.event_rx` mutably while also calling
    /// `&mut self` methods to react to what it received.
    fn process_sip_events(&mut self) {
        let mut events: Vec<(usize, SipEvent)> = Vec::new();
        for (i, acc) in self.accounts.iter_mut().enumerate() {
            while let Ok(event) = acc.handle.event_rx.try_recv() {
                events.push((i, event));
            }
        }
        for (account, event) in events {
            self.handle_sip_event(account, event);
        }
    }

    fn handle_sip_event(&mut self, account: usize, event: SipEvent) {
        match event {
            SipEvent::Registered { expires } => {
                self.accounts[account].reg_ok = true;
                self.accounts[account].status = format!("Registered (expires {expires}s)");
                self.refresh_idle_status();
                self.subscribe_account_contacts(account);
            }
            SipEvent::RegistrationFailed { reason } => {
                self.accounts[account].reg_ok = false;
                self.accounts[account].status = format!("Registration failed: {reason}");
                self.refresh_idle_status();
            }
            SipEvent::CallRinging { .. } => {
                self.status_line = "Ringing…".into();
            }
            SipEvent::CallConnected { call_id, remote_sdp } => {
                let Some(out) = self.pending_outbound.take() else {
                    tracing::warn!(call_id, "CallConnected with no pending outbound call — ignoring");
                    return;
                };
                let slot = CallSlot {
                    account, call_id, remote_uri: out.remote_uri.clone(),
                    direction: CallDirection::Outbound, start_time: out.start_time, is_held: false,
                    codec: AudioCodec::Pcmu, dtmf_type: None,
                    local_srtp: out.local_srtp, relay: out.relay, local_rtp: out.local_rtp,
                    remote_sdp: remote_sdp.clone(),
                };
                self.calls.push(slot);
                let idx = self.calls.len() - 1;
                self.status_line = format!("In call — {}", short_uri(&out.remote_uri));
                self.start_media(idx, &remote_sdp);
            }
            SipEvent::IncomingCall { call_id, from, remote_sdp } => {
                if self.calls.len() >= 2 || self.pending_call.is_some() {
                    // Already at capacity (2 concurrent + at most 1 ringing) — decline immediately.
                    self.accounts[account].handle.reject_call(&call_id);
                    return;
                }
                let waiting = !self.calls.is_empty();
                self.status_line = if waiting {
                    format!("Call waiting: {}", short_uri(&from))
                } else {
                    format!("Incoming from {}", short_uri(&from))
                };
                let acc = &self.accounts[account].account;
                let forward = acc.no_answer_forward.clone()
                    .filter(|s| !s.is_empty())
                    .map(|target| {
                        let target = normalize_target(&target, &acc.server);
                        (unix_now() + acc.no_answer_timeout_secs as u64, target)
                    });
                self.pending_call = Some(PendingCall {
                    account, call_id, from, remote_sdp, start_time: unix_now(), forward,
                });
            }
            SipEvent::CallEnded { call_id } => {
                self.on_call_terminated(&call_id, None);
                tracing::debug!(call_id, "Call ended normally");
            }
            SipEvent::CallFailed { call_id, code, reason } => {
                self.on_call_terminated(&call_id, Some((code, reason)));
            }
            SipEvent::CallHeld { call_id } => {
                if let Some(slot) = self.calls.iter_mut().find(|c| c.call_id == call_id) {
                    slot.is_held = true;
                }
                self.refresh_call_status();
            }
            SipEvent::CallResumed { call_id } => {
                if let Some(slot) = self.calls.iter_mut().find(|c| c.call_id == call_id) {
                    slot.is_held = false;
                }
                self.refresh_call_status();
            }
            SipEvent::RemoteHeld { .. } => {
                self.status_line = "Remote party put you on hold".into();
            }
            SipEvent::RemoteResumed { .. } => {
                self.status_line = "Call resumed by remote party".into();
            }
            SipEvent::TransferAccepted { call_id } => {
                tracing::info!(call_id, "Blind transfer accepted");
                self.status_line = "Transfer accepted".into();
            }
            SipEvent::TransferFailed { call_id, reason } => {
                tracing::warn!(call_id, reason, "Blind transfer failed");
                self.status_line = format!("Transfer failed: {reason}");
            }
            SipEvent::PresenceSubscribed { uri, expires } => {
                tracing::debug!(uri, expires, "Presence subscribed");
            }
            SipEvent::PresenceSubscribeFailed { uri, reason } => {
                tracing::warn!(uri, reason, "Presence subscribe failed");
                self.presence.insert(uri, PresenceState::Unknown);
            }
            SipEvent::PresenceUpdate { uri, state } => {
                self.presence.insert(uri, state);
            }
        }
    }

    /// Which account subscribes on a contact's behalf: `presence_account`
    /// (matched by username, stable across account reordering) if set, else
    /// the first configured account -- covers the common single-account case
    /// with no extra clicks.
    fn resolve_presence_account(&self, contact: &Contact) -> Option<usize> {
        match &contact.presence_account {
            Some(username) => self.accounts.iter().position(|a| &a.account.username == username),
            None => if self.accounts.is_empty() { None } else { Some(0) },
        }
    }

    /// Subscribe every `watch_presence` contact resolved to `account`,
    /// called once that account has actually registered (subscribing before
    /// then would just hit the same 401/407 retry path unnecessarily).
    fn subscribe_account_contacts(&mut self, account: usize) {
        let targets: Vec<String> = self.contacts.contacts.iter()
            .filter(|c| c.watch_presence && self.resolve_presence_account(c) == Some(account))
            .map(|c| c.sip_uri.clone())
            .collect();
        for uri in targets {
            self.accounts[account].handle.subscribe_presence(uri);
        }
    }

    /// A call in `calls` ended or an outstanding attempt failed — figure out
    /// which of `pending_call` / `calls` / `pending_outbound` `call_id`
    /// refers to, tear it down, and record it in history.
    fn on_call_terminated(&mut self, call_id: &str, failure: Option<(u16, String)>) {
        if self.pending_call.as_ref().is_some_and(|p| p.call_id == call_id) {
            let pending = self.pending_call.take().unwrap();
            self.record_history(pending.from, CallDirection::Inbound, pending.start_time, CallStatus::Missed);
            self.refresh_call_status();
            return;
        }
        if let Some(idx) = self.calls.iter().position(|c| c.call_id == call_id) {
            let status = if let Some((code, reason)) = &failure {
                self.status_line = format!("Call failed ({code}): {reason}");
                CallStatus::Failed
            } else {
                CallStatus::Answered
            };
            let slot = self.remove_call(idx);
            self.record_history(slot.remote_uri, slot.direction, slot.start_time, status);
            self.refresh_call_status();
            return;
        }
        if let Some(out) = self.pending_outbound.take() {
            if let Some((code, reason)) = &failure {
                self.status_line = format!("Call failed ({code}): {reason}");
            }
            self.record_history(out.remote_uri, CallDirection::Outbound, out.start_time, CallStatus::Failed);
            self.refresh_call_status();
        }
    }

    /// Remove call `idx`, stopping its media first if it was the focused one,
    /// and fixing up `focused_call` for the index shift. Returns the removed
    /// slot so the caller can record history from it.
    fn remove_call(&mut self, idx: usize) -> CallSlot {
        if self.focused_call == Some(idx) {
            if let Some(engine) = self.media.take() { engine.stop(); }
        }
        let slot = self.calls.remove(idx);
        self.focused_call = match self.focused_call {
            Some(f) if f == idx => None,
            Some(f) if f > idx  => Some(f - 1),
            other               => other,
        };
        slot
    }

    /// Recompute the top status line from current call state: in-call text
    /// for the focused call, a "held" hint if calls remain but none is
    /// focused, or the idle registration summary once everything's cleared.
    fn refresh_call_status(&mut self) {
        if let Some(idx) = self.focused_call {
            self.status_line = format!("In call — {}", short_uri(&self.calls[idx].remote_uri));
        } else if !self.calls.is_empty() {
            self.status_line = "On hold — tap Resume to continue".into();
        } else if self.pending_call.is_none() {
            self.refresh_idle_status();
        }
    }

    /// Recompute the top status bar from the *selected* account's
    /// registration state — a no-op while a call or incoming ring is in
    /// progress, since call-related events drive `status_line` directly
    /// during that time. Call after any registration change or whenever
    /// `selected_account` changes (e.g. the dialer's account picker).
    fn refresh_idle_status(&mut self) {
        if !self.calls.is_empty() || self.pending_call.is_some() { return; }
        match self.accounts.get(self.selected_account) {
            Some(acc) => {
                self.reg_ok       = acc.reg_ok;
                self.status_line  = if acc.reg_ok { "Ready".into() } else { "Not registered".into() };
            }
            None => {
                self.reg_ok      = false;
                self.status_line = "No accounts configured".into();
            }
        }
    }

    /// (server, username, password) if a TURN relay is configured, derived
    /// from the current settings draft.
    fn turn_config(&self) -> Option<(String, String, String)> {
        self.config.turn_server.clone().map(|server| (
            server,
            self.config.turn_username.clone().unwrap_or_default(),
            self.config.turn_password.clone().unwrap_or_default(),
        ))
    }

    /// Start (or restart, on resume) media for `calls[idx]`, using its own
    /// stored codec/srtp/relay/local_rtp — marks it `focused_call` on success.
    fn start_media(&mut self, idx: usize, remote_sdp: &str) {
        let Some(parsed) = parse_sdp(remote_sdp) else {
            tracing::error!("Cannot parse remote SDP");
            return;
        };
        self.calls[idx].codec     = parsed.codec;
        self.calls[idx].dtmf_type = parsed.dtmf_type;

        let secure = self.accounts.get(self.calls[idx].account).is_some_and(|a| a.handle.secure);
        let srtp_session = match (&self.calls[idx].local_srtp, &parsed.srtp) {
            (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
            _ => {
                if secure {
                    tracing::warn!("TLS signaling active but remote SDP has no a=crypto — falling back to plaintext RTP");
                }
                None
            }
        };

        let port    = self.calls[idx].local_rtp;
        let relay   = self.calls[idx].relay.as_ref().map(|r| r.conn.clone());
        let rt      = self.rt.clone();
        let input_device  = self.config.audio.input_device.clone();
        let output_device = self.config.audio.output_device.clone();
        let engine  = rt.block_on(MediaEngine::start(
            port, parsed.rtp_addr, parsed.codec, parsed.dtmf_type, srtp_session, relay,
            self.config.audio.echo_cancellation,
            input_device.as_deref(), output_device.as_deref(),
            self.config.recording_enabled, &self.calls[idx].call_id,
        ));
        match engine {
            Ok(e)  => { self.media = Some(e); self.focused_call = Some(idx); }
            Err(e) => { tracing::error!("MediaEngine failed: {e}"); }
        }
    }

    /// Resolve the (ip, port) to advertise in an SDP offer/answer, using
    /// `advertised_ip` as the direct-path fallback. Allocates a TURN relay on
    /// first use if one is configured, storing it into `relay` for reuse
    /// across hold/resume within that same call. Not a method (despite living
    /// in `impl DeelipApp`) so it can be called with `relay` borrowed from
    /// `self.calls[idx].relay` without aliasing `self`.
    fn resolve_rtp_endpoint(
        rt: &Handle,
        turn_config: Option<(String, String, String)>,
        advertised_ip: &str,
        local_rtp: u16,
        relay: &mut Option<TurnRelay>,
    ) -> (String, u16) {
        if relay.is_none() {
            if let Some((server, username, password)) = turn_config {
                match rt.block_on(deelip_nat::allocate_relay(&server, &username, &password)) {
                    Ok(r) => *relay = Some(r),
                    Err(e) => tracing::warn!("TURN allocation failed ({e}), falling back to direct"),
                }
            }
        }
        match relay {
            Some(r) => (r.relayed_addr.ip().to_string(), r.relayed_addr.port()),
            None => (advertised_ip.to_string(), local_rtp),
        }
    }

    fn record_history(&mut self, remote_uri: String, direction: CallDirection, start_time: u64, status: CallStatus) {
        let duration = if matches!(status, CallStatus::Answered) {
            (unix_now().saturating_sub(start_time)) as u32
        } else {
            0
        };
        let record = CallRecord { remote_uri, direction, timestamp: start_time, duration_secs: duration, status };
        self.history.push(record);
        if let Some(path) = &self.history_path {
            let _ = self.history.save(path);
        }
    }

    // ── Call actions ─────────────────────────────────────────────────────────

    fn do_call(&mut self, target: Option<String>) {
        let raw = target.unwrap_or_else(|| self.call_target.trim().to_string());
        if raw.is_empty() { return; }
        let Some(acc) = self.selected_account_idx() else { return };
        let domain = self.accounts[acc].handle.domain.clone();
        let secure = self.accounts[acc].handle.secure;
        let advertised_ip = self.accounts[acc].handle.advertised_ip.clone();
        let t = normalize_target(&raw, &domain);
        let local_rtp = alloc_rtp_port();
        let mut relay: Option<TurnRelay> = None;
        let rt = self.rt.clone();
        let turn_config = self.turn_config();
        let (rtp_ip, rtp_port) = Self::resolve_rtp_endpoint(&rt, turn_config, &advertised_ip, local_rtp, &mut relay);
        let srtp = if secure { Some(SrtpParams::generate()) } else { None };
        let sdp = build_offer(&rtp_ip, rtp_port, srtp.as_ref());
        self.accounts[acc].handle.make_call(&t, sdp);
        self.last_dialed = Some(t.clone());
        self.pending_outbound = Some(PendingOutbound {
            remote_uri: t.clone(), start_time: unix_now(),
            local_rtp, local_srtp: srtp, relay,
        });
        self.status_line = format!("Calling {}…", short_uri(&t));
    }

    fn do_redial(&mut self) {
        if let Some(target) = self.last_dialed.clone() {
            self.do_call(Some(target));
        }
    }

    fn do_accept(&mut self) {
        let Some(pending) = self.pending_call.take() else { return };
        let acc = pending.account;
        // Free the audio device for the new call if another one is focused.
        if let Some(cur) = self.focused_call {
            self.send_hold(cur);
            if let Some(engine) = self.media.take() { engine.stop(); }
            self.focused_call = None;
        }
        let codec = parse_sdp(&pending.remote_sdp).map(|p| p.codec).unwrap_or(AudioCodec::Pcmu);
        let local_rtp = alloc_rtp_port();
        let mut relay: Option<TurnRelay> = None;
        let advertised_ip = self.accounts[acc].handle.advertised_ip.clone();
        let rt = self.rt.clone();
        let turn_config = self.turn_config();
        let (rtp_ip, rtp_port) = Self::resolve_rtp_endpoint(&rt, turn_config, &advertised_ip, local_rtp, &mut relay);
        let secure = self.accounts[acc].handle.secure;
        let srtp   = if secure { Some(SrtpParams::generate()) } else { None };
        let sdp    = build_answer(&rtp_ip, rtp_port, codec, srtp.as_ref());
        self.accounts[acc].handle.accept_call(&pending.call_id, sdp);
        let slot = CallSlot {
            account: acc, call_id: pending.call_id.clone(), remote_uri: pending.from.clone(),
            direction: CallDirection::Inbound, start_time: pending.start_time, is_held: false,
            codec, dtmf_type: None, local_srtp: srtp, relay, local_rtp,
            remote_sdp: pending.remote_sdp.clone(),
        };
        self.calls.push(slot);
        let idx = self.calls.len() - 1;
        self.status_line = "Accepted — connecting…".into();
        self.start_media(idx, &pending.remote_sdp);
    }

    fn do_reject(&mut self) {
        if let Some(pending) = self.pending_call.take() {
            self.record_history(pending.from, CallDirection::Inbound, pending.start_time, CallStatus::Rejected);
            self.accounts[pending.account].handle.reject_call(&pending.call_id);
            self.refresh_call_status();
        }
    }

    fn do_hangup(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let acc     = self.calls[idx].account;
        self.accounts[acc].handle.hang_up(&call_id);
        let slot = self.remove_call(idx);
        self.record_history(slot.remote_uri, slot.direction, slot.start_time, CallStatus::Answered);
        self.refresh_call_status();
    }

    /// Send the hold re-INVITE for `idx` (optimistic — doesn't wait for the
    /// confirming `SipEvent::CallHeld`). Doesn't touch `media`/`focused_call`;
    /// callers that are actually switching audio away from this call do that
    /// themselves (see `do_hold_slot`/`do_accept`/`do_swap_to`).
    fn send_hold(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let acc     = self.calls[idx].account;
        let advertised_ip = self.accounts[acc].handle.advertised_ip.clone();
        let local_rtp = self.calls[idx].local_rtp;
        let rt = self.rt.clone();
        let turn_config = self.turn_config();
        let (rtp_ip, rtp_port) = Self::resolve_rtp_endpoint(&rt, turn_config, &advertised_ip, local_rtp, &mut self.calls[idx].relay);
        let sdp = build_hold_offer(&rtp_ip, rtp_port, self.calls[idx].codec, self.calls[idx].local_srtp.as_ref());
        self.calls[idx].is_held = true;
        self.accounts[acc].handle.hold_call(&call_id, sdp);
    }

    fn send_resume(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let acc     = self.calls[idx].account;
        let advertised_ip = self.accounts[acc].handle.advertised_ip.clone();
        let local_rtp = self.calls[idx].local_rtp;
        let rt = self.rt.clone();
        let turn_config = self.turn_config();
        let (rtp_ip, rtp_port) = Self::resolve_rtp_endpoint(&rt, turn_config, &advertised_ip, local_rtp, &mut self.calls[idx].relay);
        let sdp = build_resume_offer(&rtp_ip, rtp_port, self.calls[idx].codec, self.calls[idx].local_srtp.as_ref());
        self.accounts[acc].handle.resume_call(&call_id, sdp);
    }

    /// Hold call `idx` — if it's the focused one, its media stops and no
    /// call has live audio until the user swaps back to something.
    fn do_hold_slot(&mut self, idx: usize) {
        self.send_hold(idx);
        if self.focused_call == Some(idx) {
            if let Some(engine) = self.media.take() { engine.stop(); }
            self.focused_call = None;
        }
        self.refresh_call_status();
    }

    /// Switch live audio to call `idx`: holds whatever's currently focused
    /// (there's at most one other call), then resumes and restarts media
    /// for `idx` using its last-known remote SDP.
    fn do_swap_to(&mut self, idx: usize) {
        if self.focused_call == Some(idx) { return; }
        if let Some(cur) = self.focused_call {
            self.send_hold(cur);
            if let Some(engine) = self.media.take() { engine.stop(); }
            self.focused_call = None;
        }
        self.send_resume(idx);
        self.calls[idx].is_held = false;
        let remote_sdp = self.calls[idx].remote_sdp.clone();
        self.start_media(idx, &remote_sdp);
        self.refresh_call_status();
    }

    /// `selected_account` clamped to a valid index — `None` if there are no
    /// accounts at all (nothing to call from).
    fn selected_account_idx(&self) -> Option<usize> {
        if self.accounts.is_empty() { return None; }
        Some(self.selected_account.min(self.accounts.len() - 1))
    }

    fn do_dtmf(&self, digit: char) {
        if let Some(engine) = &self.media {
            engine.send_dtmf(digit);
        }
    }

    fn is_muted(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.is_muted())
    }

    fn do_mute_toggle(&self) {
        if let Some(engine) = &self.media {
            engine.set_muted(!engine.is_muted());
        }
    }

    /// Blind-transfer the focused call to `self.transfer_target`.
    fn do_transfer(&mut self) {
        let Some(idx) = self.focused_call else { return };
        let raw = self.transfer_target.trim().to_string();
        if raw.is_empty() { return; }
        let acc    = self.calls[idx].account;
        let domain = self.accounts[acc].handle.domain.clone();
        let target = normalize_target(&raw, &domain);
        let call_id = self.calls[idx].call_id.clone();
        self.accounts[acc].handle.blind_transfer(&call_id, target);
        self.status_line      = "Transferring…".into();
        self.transfer_target.clear();
        self.showing_transfer = false;
    }

    /// If the pending incoming call has a no-answer-forward deadline and
    /// it's elapsed, redirect it (302) instead of leaving it ringing forever.
    /// Called once per frame from `update()`.
    fn check_pending_call_timeout(&mut self) {
        let Some(pending) = &self.pending_call else { return };
        let Some((deadline, target)) = &pending.forward else { return };
        if unix_now() < *deadline { return; }
        let target = target.clone();
        let pending = self.pending_call.take().unwrap();
        self.accounts[pending.account].handle.redirect_call(&pending.call_id, target);
        self.record_history(pending.from, CallDirection::Inbound, pending.start_time, CallStatus::Missed);
        self.refresh_call_status();
    }

    /// Start/stop the ringtone to match current call state — a no-op if it's
    /// already playing the right thing (or nothing). Called once per frame.
    fn sync_ringtone(&mut self) {
        let desired = if !self.config.ringtone_enabled {
            None
        } else if self.pending_call.is_some() {
            Some(RingKind::Incoming)
        } else if self.pending_outbound.is_some() {
            Some(RingKind::Outgoing)
        } else {
            None
        };

        let is_ringing = desired.is_some();
        if is_ringing && !self.was_ringing {
            // Rising edge — attempt exactly once per ringing episode. A
            // failure here must NOT leave room for a retry next frame (see
            // `was_ringing` doc comment) — it's still `None` either way.
            match Ringtone::start(desired.unwrap()) {
                Ok(r) => self.ringtone = Some(r),
                Err(e) => tracing::warn!("Ringtone failed to start: {e}"),
            }
        } else if !is_ringing {
            self.ringtone = None;
        }
        self.was_ringing = is_ringing;
    }

    /// Fire a desktop notification once per incoming call (not every frame
    /// it's still ringing). Called once per frame.
    fn sync_notifications(&mut self) {
        if !self.config.notifications_enabled {
            self.last_notified_call = None;
            return;
        }
        match &self.pending_call {
            Some(p) if self.last_notified_call.as_deref() != Some(p.call_id.as_str()) => {
                self.last_notified_call = Some(p.call_id.clone());
                notify::notify_incoming_call(&p.from);
            }
            None => self.last_notified_call = None,
            _ => {}
        }
    }

    /// Persist `config` immediately, without the Settings tab's "restart to
    /// apply" notice — for the appearance/notification toggles that apply
    /// live and don't go through the explicit Save button.
    fn save_config_quietly(&self) {
        if let Err(e) = self.config.save(&self.config_path) {
            tracing::error!("Failed to save config: {e}");
        }
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl DeelipApp {
    /// Minimize-to-tray: hide instead of quitting on window close, and
    /// restore/quit in response to tray icon clicks or menu selections.
    /// No-op (falls back to normal close-quits-the-app behavior) if the
    /// tray icon failed to start. Actual click/menu handling happens on
    /// independent background threads (see `tray` module docs) — this just
    /// (a) intercepts close-to-minimize, which can only happen from inside
    /// `update()`, and (b) keeps the background threads' shared state fresh
    /// for whenever they do run.
    fn process_tray_events(&mut self, ctx: &egui::Context) {
        let Some((ctx_slot, quit_state)) = &self.tray else { return };

        *ctx_slot.lock().unwrap() = Some(ctx.clone());
        *quit_state.calls.lock().unwrap() = self.calls.iter()
            .map(|c| (self.accounts[c.account].handle.cmd_tx.clone(), c.call_id.clone()))
            .collect();
        *quit_state.pending.lock().unwrap() = self.pending_call.as_ref()
            .map(|p| (self.accounts[p.account].handle.cmd_tx.clone(), p.call_id.clone()));

        if ctx.input(|i| i.viewport().close_requested()) {
            tracing::debug!("Tray: close requested, hiding to tray instead");
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // Visible(false), not Minimized(true) -- see tray::restore_window's
            // doc comment for why: Mutter's XWayland iconify handling is
            // unreliable, but window mapping (Visible) isn't.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }
}

impl eframe::App for DeelipApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.first_frame) && self.config.start_minimized {
            // Must run on the first frame, not before -- eframe force-shows
            // the window right after this frame renders regardless of any
            // NativeOptions visibility hint, so queuing this command here
            // (applied after that forced show, per eframe's own event-loop
            // ordering) is what actually makes it stick. See main.rs.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        self.process_sip_events();
        self.check_pending_call_timeout();
        self.sync_ringtone();
        self.sync_notifications();
        self.process_tray_events(ctx);

        self.palette = Palette::for_theme(self.config.dark_mode);
        let mut visuals = if self.config.dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() };
        theme::apply_style(ctx, &mut visuals, &self.palette);
        ctx.set_visuals(visuals);

        // ── Status bar ───────────────────────────────────────────────────────
        let on_hold = self.focused_call.is_none() && !self.calls.is_empty();
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            status_bar(ui, &self.palette, &self.status_line, self.reg_ok, on_hold);
        });

        // ── Tab bar ──────────────────────────────────────────────────────────
        // Selected tab gets an accent-tinted background for free, via
        // `visuals.selection.bg_fill` (set to `palette.accent` in
        // `theme::apply_style` above) -- the same highlight every other
        // selectable widget in the app uses, not a one-off tab-bar special case.
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Dialer,   format!("{}  Dialer",   egui_phosphor::regular::PHONE));
                ui.selectable_value(&mut self.tab, Tab::History,  format!("{}  History",  egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE));
                ui.selectable_value(&mut self.tab, Tab::Contacts, format!("{}  Contacts", egui_phosphor::regular::ADDRESS_BOOK));
                ui.selectable_value(&mut self.tab, Tab::Settings, format!("{}  Settings", egui_phosphor::regular::GEAR));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icon = if self.config.dark_mode { egui_phosphor::regular::SUN } else { egui_phosphor::regular::MOON };
                    if ui.button(icon).on_hover_text("Toggle light/dark theme").clicked() {
                        self.config.dark_mode = !self.config.dark_mode;
                        self.save_config_quietly();
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                Tab::Dialer   => self.show_dialer(ui),
                Tab::History  => self.show_history(ui, ctx),
                Tab::Contacts => self.show_contacts(ui, ctx),
                Tab::Settings => self.show_settings(ui),
            }
        });

        ctx.request_repaint_after(Duration::from_millis(50));
    }

    /// Hang up any in-progress call before the process exits, so the remote
    /// side and server don't keep a dangling channel around.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.hangup_before_exit();
    }
}

impl DeelipApp {
    /// Hang up any in-progress call (or reject a pending incoming one).
    /// Sending BYE only queues it on the SipStack's command channel; block
    /// briefly so the background task actually transmits it before the
    /// runtime is torn down.
    fn hangup_before_exit(&mut self) {
        let mut sent = false;
        for call in &self.calls {
            self.accounts[call.account].handle.hang_up(&call.call_id);
            sent = true;
        }
        if let Some(pending) = self.pending_call.take() {
            self.accounts[pending.account].handle.reject_call(&pending.call_id);
            sent = true;
        }
        if sent {
            self.rt.block_on(tokio::time::sleep(Duration::from_millis(200)));
        }
    }
}

// ── Tab: Dialer ───────────────────────────────────────────────────────────────

impl DeelipApp {
    fn show_dialer(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        let can_dial = self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();

        // ── Account picker (only shown with more than one account) ──────────
        if self.accounts.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Call from:");
                let current = self.selected_account_idx().unwrap_or(0);
                let selected_label = {
                    let acc = &self.accounts[current];
                    format!("{} {}", if acc.reg_ok { "●" } else { "○" }, acc.label)
                };
                egui::ComboBox::from_id_source("dialer_account_picker")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for i in 0..self.accounts.len() {
                            let acc = &self.accounts[i];
                            let label = format!("{} {}", if acc.reg_ok { "●" } else { "○" }, acc.label);
                            if ui.add_enabled(can_dial, egui::SelectableLabel::new(current == i, label)).clicked() {
                                self.selected_account = i;
                                self.refresh_idle_status();
                            }
                        }
                    });
            });
            ui.add_space(6.0);
        }

        // ── Waiting/incoming call banner ──────────────────────────────────────
        if let Some(pending) = &self.pending_call {
            let waiting = !self.calls.is_empty();
            let from = pending.from.clone();
            let account_suffix = if self.accounts.len() > 1 {
                format!(" (on {})", self.accounts[pending.account].label)
            } else {
                String::new()
            };
            let heading = if waiting {
                format!("Call waiting: {}{account_suffix}", short_uri(&from))
            } else {
                format!("Incoming call from {}{account_suffix}", short_uri(&from))
            };
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(heading)
                        .color(self.palette.warn)
                        .font(FontId::proportional(17.0)),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let accept = format!("{}  Accept", egui_phosphor::regular::PHONE);
                    if ui.button(RichText::new(accept).color(self.palette.accent)).clicked() {
                        self.do_accept();
                    }
                    let reject = format!("{}  Reject", egui_phosphor::regular::PHONE_X);
                    if ui.button(RichText::new(reject).color(self.palette.danger)).clicked() {
                        self.do_reject();
                    }
                });
            });
            ui.add_space(8.0);
        }

        // ── Active/held calls ────────────────────────────────────────────────
        if !self.calls.is_empty() {
            let mut hangup_idx: Option<usize> = None;
            let mut hold_idx:   Option<usize> = None;
            let mut swap_idx:   Option<usize> = None;

            for idx in 0..self.calls.len() {
                let focused = self.focused_call == Some(idx);
                let (icon, color, uri) = {
                    let call = &self.calls[idx];
                    let (icon, color) = match call.direction {
                        CallDirection::Inbound  => (egui_phosphor::regular::PHONE_INCOMING, self.palette.info),
                        CallDirection::Outbound => (egui_phosphor::regular::PHONE_OUTGOING, self.palette.accent),
                    };
                    (icon, color, call.remote_uri.clone())
                };
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon).color(color));
                        ui.label(short_uri(&uri));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hang_up = format!("{}  Hang Up", egui_phosphor::regular::PHONE_X);
                            if ui.button(RichText::new(hang_up).color(self.palette.danger)).clicked() {
                                hangup_idx = Some(idx);
                            }
                            ui.add_space(4.0);
                            if focused {
                                let hold = format!("{}  Hold", egui_phosphor::regular::PHONE_PAUSE);
                                if ui.button(hold).clicked() { hold_idx = Some(idx); }
                            } else {
                                let resume = format!("{}  Resume", egui_phosphor::regular::PLAY);
                                if ui.button(resume).clicked() { swap_idx = Some(idx); }
                            }
                            ui.add_space(4.0);
                            let state = if focused { "Active" } else { "On hold" };
                            ui.label(RichText::new(state).color(self.palette.muted));
                            if focused && self.config.recording_enabled {
                                ui.add_space(4.0);
                                ui.label(RichText::new("● REC").color(self.palette.danger));
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }

            if let Some(idx) = hangup_idx { self.do_hangup(idx); }
            if let Some(idx) = hold_idx   { self.do_hold_slot(idx); }
            if let Some(idx) = swap_idx   { self.do_swap_to(idx); }
        }

        // ── Call target + Call/Redial buttons ────────────────────────────────
        ui.group(|ui| {
            ui.label("SIP address / number:");
            ui.add_space(4.0);
            let resp = ui.add_enabled(
                can_dial,
                egui::TextEdit::singleline(&mut self.call_target)
                    .hint_text("sip:bob@example.com")
                    .desired_width(f32::INFINITY),
            );
            if resp.lost_focus() && ctx_key_enter(ui) && can_dial {
                self.do_call(None);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let call_text = RichText::new(format!("{}  Call", egui_phosphor::regular::PHONE)).color(self.palette.accent);
                if ui.add_enabled(can_dial && self.reg_ok, egui::Button::new(call_text)).clicked() {
                    self.do_call(None);
                }
                ui.add_space(4.0);
                let can_redial = can_dial && self.reg_ok && self.last_dialed.is_some();
                let redial_text = format!("{}  Redial", egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE);
                if ui.add_enabled(can_redial, egui::Button::new(redial_text)).clicked() {
                    self.do_redial();
                }
            });
        });

        // ── Compose keypad (build up a number by clicking, not typing) ───────
        if can_dial {
            ui.add_space(8.0);
            let palette = self.palette;
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                phone_keypad(ui, palette, |digit| self.call_target.push(digit));
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.horizontal(|ui| {
                        if ui.button(egui_phosphor::regular::BACKSPACE).clicked() { self.call_target.pop(); }
                        if ui.button("Clear").clicked() { self.call_target.clear(); }
                    });
                });
            });
        }

        // ── Focused-call controls: Mute, Transfer, DTMF ──────────────────────
        if self.focused_call.is_some() {
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mute_icon = if self.is_muted() { egui_phosphor::regular::MICROPHONE_SLASH } else { egui_phosphor::regular::MICROPHONE };
                    let mute_label = format!("{mute_icon}  {}", if self.is_muted() { "Unmute" } else { "Mute" });
                    if ui.button(mute_label).clicked() { self.do_mute_toggle(); }
                    ui.add_space(4.0);
                    let transfer_label = format!("{}  {}", egui_phosphor::regular::ARROW_BEND_UP_RIGHT, if self.showing_transfer { "Cancel transfer" } else { "Transfer" });
                    if ui.button(transfer_label).clicked() {
                        self.showing_transfer = !self.showing_transfer;
                    }
                });
                if self.showing_transfer {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.transfer_target)
                            .hint_text("sip:carol@example.com")
                            .desired_width(f32::INFINITY));
                        if ui.button("Send").clicked() {
                            self.do_transfer();
                        }
                    });
                }
            });

            ui.add_space(8.0);
            let palette = self.palette;
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.label("DTMF:");
                ui.add_space(4.0);
                phone_keypad(ui, palette, |digit| self.do_dtmf(digit));
            });
        }
    }
}

// ── Tab: History ──────────────────────────────────────────────────────────────

impl DeelipApp {
    fn show_history(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        ui.add_space(8.0);
        if self.history.records.is_empty() {
            ui.label("No call history yet.");
            return;
        }

        // ── Search / filter / export bar ─────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.history_search)
                    .desired_width(140.0)
                    .hint_text("name or URI"),
            );
            ui.label("Status:");
            egui::ComboBox::from_id_source("history_status_filter")
                .selected_text(status_filter_label(&self.history_status_filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.history_status_filter, None, "All");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Answered), "Answered");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Missed), "Missed");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Rejected), "Rejected");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Failed), "Failed");
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let export = format!("{}  Export CSV…", egui_phosphor::regular::DOWNLOAD_SIMPLE);
                if ui.button(export).clicked() {
                    self.export_history_csv();
                }
            });
        });
        ui.add_space(4.0);

        let query = self.history_search.to_lowercase();
        let filtered: Vec<&CallRecord> = self.history.records.iter()
            .filter(|r| self.history_status_filter.as_ref().is_none_or(|s| *s == r.status))
            .filter(|r| query.is_empty() || r.remote_uri.to_lowercase().contains(&query))
            .collect();

        let mut call_target: Option<String> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            if filtered.is_empty() {
                ui.label(RichText::new("No matching calls.").color(self.palette.muted));
            }
            for record in filtered {
                let (dir_icon, dir_color) = match record.direction {
                    CallDirection::Inbound  => (egui_phosphor::regular::PHONE_INCOMING, self.palette.info),
                    CallDirection::Outbound => (egui_phosphor::regular::PHONE_OUTGOING, self.palette.accent),
                };
                let status_str = match record.status {
                    CallStatus::Answered => format_duration(record.duration_secs),
                    CallStatus::Missed   => "Missed".into(),
                    CallStatus::Rejected => "Rejected".into(),
                    CallStatus::Failed   => "Failed".into(),
                };

                ui.horizontal(|ui| {
                    ui.label(RichText::new(dir_icon).color(dir_color));
                    ui.label(short_uri(&record.remote_uri));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Call").clicked() {
                            call_target = Some(record.remote_uri.clone());
                        }
                        ui.label(RichText::new(&status_str).color(self.palette.muted));
                        ui.label(RichText::new(format_age(record.timestamp)).color(self.palette.muted));
                    });
                });
                ui.separator();
            }
        });

        if let Some(target) = call_target {
            self.tab         = Tab::Dialer;
            self.call_target = target.clone();
            let can_dial = self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
            if can_dial && self.reg_ok {
                self.do_call(Some(target));
            }
        }
    }

    /// Export the currently filtered history view (respecting the search box
    /// and status dropdown) to a CSV file via a native save dialog.
    fn export_history_csv(&self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("deelip_history.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
        else {
            return;
        };

        let query = self.history_search.to_lowercase();
        let filtered = self.history.records.iter()
            .filter(|r| self.history_status_filter.as_ref().is_none_or(|s| *s == r.status))
            .filter(|r| query.is_empty() || r.remote_uri.to_lowercase().contains(&query));

        let mut csv = String::from("timestamp,direction,remote_uri,status,duration_secs\n");
        for r in filtered {
            let direction = match r.direction {
                CallDirection::Inbound  => "inbound",
                CallDirection::Outbound => "outbound",
            };
            let status = match r.status {
                CallStatus::Answered => "answered",
                CallStatus::Missed   => "missed",
                CallStatus::Rejected => "rejected",
                CallStatus::Failed   => "failed",
            };
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                r.timestamp, direction, csv_escape(&r.remote_uri), status, r.duration_secs,
            ));
        }

        if let Err(e) = std::fs::write(&path, csv) {
            tracing::error!("Failed to export history to {}: {e}", path.display());
        }
    }
}

// ── Tab: Contacts ─────────────────────────────────────────────────────────────

impl DeelipApp {
    fn show_contacts(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        ui.add_space(8.0);

        // Search bar
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.contact_search)
                    .desired_width(200.0)
                    .hint_text("name or sip URI"),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(egui_phosphor::regular::UPLOAD_SIMPLE).on_hover_text("Import contacts (CSV or vCard)").clicked() {
                    self.import_contacts();
                }
                if ui.button(format!("{} vCard", egui_phosphor::regular::DOWNLOAD_SIMPLE)).on_hover_text("Export as vCard").clicked() {
                    self.export_contacts_vcard();
                }
                if ui.button(format!("{} CSV", egui_phosphor::regular::DOWNLOAD_SIMPLE)).on_hover_text("Export as CSV").clicked() {
                    self.export_contacts_csv();
                }
            });
        });
        ui.add_space(4.0);

        let mut call_target: Option<String> = None;
        let mut edit_idx:    Option<usize>   = None;
        let mut delete_idx:  Option<usize>   = None;

        // Contact list
        let results: Vec<(usize, String, String, bool)> = self.contacts
            .search(&self.contact_search)
            .into_iter()
            .map(|(i, c)| (i, c.name.clone(), c.sip_uri.clone(), c.watch_presence))
            .collect();

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                if results.is_empty() {
                    ui.label(RichText::new("No contacts found.").color(self.palette.muted));
                }
                for (idx, name, uri, watch_presence) in &results {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if *watch_presence {
                            let color = match self.presence.get(uri) {
                                Some(PresenceState::Available) => self.palette.accent,
                                _ => self.palette.muted,
                            };
                            ui.label(RichText::new("●").color(color))
                                .on_hover_text(match self.presence.get(uri) {
                                    Some(PresenceState::Available) => "Available",
                                    Some(PresenceState::Offline) => "Offline",
                                    _ => "Unknown",
                                });
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(egui_phosphor::regular::PHONE).clicked() {
                                call_target = Some(uri.clone());
                            }
                            if ui.small_button(RichText::new(egui_phosphor::regular::TRASH).color(self.palette.danger)).clicked() {
                                delete_idx = Some(*idx);
                            }
                            if ui.small_button(egui_phosphor::regular::PENCIL_SIMPLE).clicked() {
                                edit_idx = Some(*idx);
                            }
                            ui.label(RichText::new(uri).color(self.palette.muted));
                        });
                    });
                    ui.separator();
                }
            });

        if let Some(idx) = edit_idx {
            self.editing_contact_idx = Some(idx);
            self.new_contact = self.contacts.contacts[idx].clone();
        }
        if let Some(idx) = delete_idx {
            let removed = self.contacts.contacts.remove(idx);
            self.unsubscribe_contact_presence(&removed);
            if self.editing_contact_idx == Some(idx) {
                self.editing_contact_idx = None;
                self.new_contact = Contact::default();
            }
            if let Some(path) = &self.contacts_path {
                let _ = self.contacts.save(path);
            }
        }

        ui.add_space(8.0);
        ui.separator();

        // Add/Edit contact form
        let heading = if self.editing_contact_idx.is_some() { "Edit Contact" } else { "Add Contact" };
        ui.label(RichText::new(heading).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add(egui::TextEdit::singleline(&mut self.new_contact.name)
                .desired_width(120.0));
            ui.label("URI:");
            ui.add(egui::TextEdit::singleline(&mut self.new_contact.sip_uri)
                .hint_text("sip:alice@example.com")
                .desired_width(f32::INFINITY));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.new_contact.watch_presence, "Watch presence");
            if self.accounts.len() > 1 {
                ui.label("via:");
                let current_label = match &self.new_contact.presence_account {
                    Some(username) => self.accounts.iter()
                        .find(|a| &a.account.username == username)
                        .map(|a| a.label.clone())
                        .unwrap_or_else(|| username.clone()),
                    None => self.accounts.first()
                        .map(|a| format!("{} (default)", a.label))
                        .unwrap_or_default(),
                };
                egui::ComboBox::from_id_source("contact_presence_account_picker")
                    .selected_text(current_label)
                    .show_ui(ui, |ui| {
                        for acc in &self.accounts {
                            let is_sel = self.new_contact.presence_account.as_deref() == Some(acc.account.username.as_str());
                            if ui.selectable_label(is_sel, &acc.label).clicked() {
                                self.new_contact.presence_account = Some(acc.account.username.clone());
                            }
                        }
                    });
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_save = !self.new_contact.name.is_empty() && !self.new_contact.sip_uri.is_empty();
            if ui.add_enabled(can_save, egui::Button::new("Save Contact")).clicked() {
                let c = std::mem::take(&mut self.new_contact);
                if let Some(idx) = self.editing_contact_idx.take() {
                    let old = self.contacts.contacts[idx].clone();
                    self.contacts.contacts[idx] = c.clone();
                    self.unsubscribe_contact_presence(&old);
                    self.subscribe_contact_presence(&c);
                } else {
                    self.contacts.contacts.push(c.clone());
                    self.subscribe_contact_presence(&c);
                }
                if let Some(path) = &self.contacts_path {
                    let _ = self.contacts.save(path);
                }
            }
            if self.editing_contact_idx.is_some() && ui.button("Cancel").clicked() {
                self.editing_contact_idx = None;
                self.new_contact = Contact::default();
            }
        });

        if let Some(target) = call_target {
            self.tab         = Tab::Dialer;
            self.call_target = target.clone();
            let can_dial = self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
            if can_dial && self.reg_ok {
                self.do_call(Some(target));
            }
        }
    }

    fn subscribe_contact_presence(&mut self, contact: &Contact) {
        if !contact.watch_presence { return; }
        if let Some(idx) = self.resolve_presence_account(contact) {
            self.accounts[idx].handle.subscribe_presence(contact.sip_uri.clone());
        }
    }

    fn unsubscribe_contact_presence(&mut self, contact: &Contact) {
        if contact.watch_presence {
            if let Some(idx) = self.resolve_presence_account(contact) {
                self.accounts[idx].handle.unsubscribe_presence(contact.sip_uri.clone());
            }
        }
        self.presence.remove(&contact.sip_uri);
    }

    fn export_contacts_csv(&self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("deelip_contacts.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
        else {
            return;
        };

        let mut csv = String::from("name,sip_uri\n");
        for c in &self.contacts.contacts {
            csv.push_str(&format!("{},{}\n", csv_escape(&c.name), csv_escape(&c.sip_uri)));
        }

        if let Err(e) = std::fs::write(&path, csv) {
            tracing::error!("Failed to export contacts to {}: {e}", path.display());
        }
    }

    fn export_contacts_vcard(&self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("deelip_contacts.vcf")
            .add_filter("vCard", &["vcf"])
            .save_file()
        else {
            return;
        };

        let mut vcf = String::new();
        for c in &self.contacts.contacts {
            vcf.push_str("BEGIN:VCARD\r\n");
            vcf.push_str("VERSION:3.0\r\n");
            vcf.push_str(&format!("FN:{}\r\n", c.name));
            vcf.push_str(&format!("IMPP:{}\r\n", c.sip_uri));
            vcf.push_str("END:VCARD\r\n");
        }

        if let Err(e) = std::fs::write(&path, vcf) {
            tracing::error!("Failed to export contacts to {}: {e}", path.display());
        }
    }

    /// Import contacts from a CSV or vCard file (detected by extension,
    /// falling back to content sniffing). Appended to the existing contact
    /// list with no dedup, matching the manual Add-contact flow's behavior.
    fn import_contacts(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Contacts", &["csv", "vcf"])
            .pick_file()
        else {
            return;
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => { tracing::error!("Failed to read {}: {e}", path.display()); return; }
        };

        let is_vcard = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("vcf"))
            || content.contains("BEGIN:VCARD");

        let imported = if is_vcard {
            parse_vcard(&content)
        } else {
            parse_contacts_csv(&content)
        };

        if imported.is_empty() {
            tracing::warn!("No contacts found in {}", path.display());
            return;
        }

        self.contacts.contacts.extend(imported);
        if let Some(path) = &self.contacts_path {
            let _ = self.contacts.save(path);
        }
    }
}

/// Parse a CSV contact file with a `name,sip_uri` header, using
/// `parse_csv_line` for each data row.
fn parse_contacts_csv(content: &str) -> Vec<Contact> {
    content.lines().skip(1)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields = parse_csv_line(line);
            let name    = fields.first()?.clone();
            let sip_uri = fields.get(1)?.clone();
            Some(Contact { name, sip_uri, ..Default::default() })
        })
        .collect()
}

/// Split one CSV line into fields, honoring double-quoted fields and
/// doubled-quote escaping -- the inverse of `csv_escape`.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => { field.push('"'); chars.next(); }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => { fields.push(std::mem::take(&mut field)); }
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// Minimal vCard 2.1/3.0 parser: pulls `FN` for the name and the first
/// `TEL`/`IMPP` line (any `;PARAM=...` suffix on the property name is
/// ignored) for the URI, from each `BEGIN:VCARD`/`END:VCARD` block.
fn parse_vcard(content: &str) -> Vec<Contact> {
    let mut contacts = Vec::new();
    let mut name: Option<String> = None;
    let mut uri: Option<String> = None;

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.eq_ignore_ascii_case("BEGIN:VCARD") {
            name = None;
            uri = None;
            continue;
        }
        if line.eq_ignore_ascii_case("END:VCARD") {
            if let (Some(n), Some(u)) = (name.take(), uri.take()) {
                contacts.push(Contact { name: n, sip_uri: u, ..Default::default() });
            }
            continue;
        }
        let Some((prop, value)) = line.split_once(':') else { continue };
        let prop_name = prop.split(';').next().unwrap_or(prop);
        if name.is_none() && prop_name.eq_ignore_ascii_case("FN") {
            name = Some(value.to_string());
        } else if uri.is_none() && (prop_name.eq_ignore_ascii_case("TEL") || prop_name.eq_ignore_ascii_case("IMPP")) {
            uri = Some(value.to_string());
        }
    }
    contacts
}

// ── Tab: Settings ─────────────────────────────────────────────────────────────

impl DeelipApp {
    fn show_settings(&mut self, ui: &mut Ui) {
        if self.config.accounts.is_empty() {
            self.config.accounts.push(deelip_config::SipAccount::default());
        }
        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
        let mut edited = false;
        let palette = self.palette;

        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // ── Notifications & Ringtone (applies immediately) ──────────────
            ui.label(RichText::new("Notifications & Ringtone").strong());
            ui.group(|ui| {
                if ui.checkbox(&mut self.config.notifications_enabled, "Desktop notification on incoming calls").changed() {
                    self.save_config_quietly();
                }
                if ui.checkbox(&mut self.config.ringtone_enabled, "Ringtone (incoming) / ringback (outgoing)").changed() {
                    self.save_config_quietly();
                }
                ui.label(RichText::new("Applies immediately — no restart needed.").color(palette.muted).small());
            });
            ui.add_space(10.0);

            // ── Startup ───────────────────────────────────────────────────
            ui.label(RichText::new("Startup").strong());
            ui.group(|ui| {
                edited |= ui.checkbox(&mut self.config.start_minimized, "Start minimized (to tray)").changed();
                ui.label(RichText::new("Restart to apply.").color(palette.muted).small());
                ui.add_space(4.0);
                if ui.checkbox(&mut self.autostart_enabled, "Start DeeLip on login").changed() {
                    if let Err(e) = deelip_config::set_autostart(self.autostart_enabled) {
                        tracing::error!("Failed to update autostart: {e}");
                        self.autostart_enabled = deelip_config::is_autostart_enabled();
                    }
                }
                ui.label(RichText::new("Applies immediately — no restart needed.").color(palette.muted).small());
            });
            ui.add_space(10.0);

            // ── Account ───────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(RichText::new("Accounts").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_remove = self.config.accounts.len() > 1;
                    if ui.add_enabled(can_remove, egui::Button::new("Remove")).clicked() {
                        self.config.accounts.remove(self.edit_account_idx);
                        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
                        edited = true;
                    }
                    if ui.button("+ Add account").clicked() {
                        self.config.accounts.push(SipAccount::default());
                        self.edit_account_idx = self.config.accounts.len() - 1;
                        edited = true;
                    }
                });
            });
            ui.add_space(4.0);
            egui::ComboBox::from_id_source("settings_account_picker")
                .selected_text(format!(
                    "{}. {}",
                    self.edit_account_idx + 1,
                    account_label(&self.config.accounts[self.edit_account_idx]),
                ))
                .show_ui(ui, |ui| {
                    for i in 0..self.config.accounts.len() {
                        let label = format!("{}. {}", i + 1, account_label(&self.config.accounts[i]));
                        ui.selectable_value(&mut self.edit_account_idx, i, label);
                    }
                });
            ui.add_space(6.0);

            ui.group(|ui| {
                let account = &mut self.config.accounts[self.edit_account_idx];

                edited |= ui.checkbox(&mut account.enabled, "Enabled (register this account on next restart)").changed();
                ui.add_space(4.0);

                egui::Grid::new("settings_account_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Username:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.username)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Password:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.password)
                            .password(true)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Server:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.server)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Port:");
                        edited |= ui.add(egui::DragValue::new(&mut account.port)).changed();
                        ui.end_row();

                        ui.label("Display name:");
                        let mut display_name = account.display_name.clone().unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut display_name)
                            .desired_width(f32::INFINITY)).changed()
                        {
                            account.display_name = if display_name.is_empty() { None } else { Some(display_name) };
                            edited = true;
                        }
                        ui.end_row();

                        ui.label("Transport:");
                        egui::ComboBox::from_id_source("settings_transport")
                            .selected_text(match account.transport {
                                TransportProtocol::Udp => "UDP",
                                TransportProtocol::Tcp => "TCP",
                                TransportProtocol::Tls => "TLS",
                            })
                            .show_ui(ui, |ui| {
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Udp, "UDP").changed();
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tcp, "TCP").changed();
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tls, "TLS").changed();
                            });
                        ui.end_row();
                    });

                if account.transport == TransportProtocol::Tls {
                    edited |= ui.checkbox(
                        &mut account.tls_insecure_skip_verify,
                        "Skip TLS certificate verification (self-signed/home-lab PBXes)",
                    ).changed();
                    if account.tls_insecure_skip_verify {
                        ui.label(RichText::new(
                            "Warning: certificate verification is disabled — traffic can be intercepted."
                        ).color(palette.warn));
                    }
                }

                ui.add_space(6.0);
                ui.label("Forward if unanswered (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.no_answer_forward, "sip:voicemail@example.com");
                });
                ui.horizontal(|ui| {
                    ui.label("after (seconds):");
                    edited |= ui.add(egui::DragValue::new(&mut account.no_answer_timeout_secs).range(1..=300)).changed();
                });
            });

            if !self.config.accounts.iter().any(|a| a.enabled) {
                ui.label(RichText::new(
                    "Warning: no accounts are enabled — DeeLip won't be able to register on restart."
                ).color(palette.warn));
            }
            ui.label(RichText::new(
                "Each enabled account registers independently on its own local SIP port \
                 (base port below, incrementing by one per additional account)."
            ).color(palette.muted).small());

            ui.add_space(10.0);

            // ── Audio ─────────────────────────────────────────────────────
            ui.label(RichText::new("Audio").strong());
            ui.group(|ui| {
                let (input_names, output_names) = self.audio_device_cache
                    .get_or_insert_with(|| (list_device_names(true), list_device_names(false)))
                    .clone();

                if ui.button("Refresh device list").clicked() {
                    self.audio_device_cache = Some((list_device_names(true), list_device_names(false)));
                }

                egui::Grid::new("settings_audio_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Input device:");
                        let selected = self.config.audio.input_device.clone()
                            .unwrap_or_else(|| "Default".into());
                        egui::ComboBox::from_id_source("settings_input_device")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.config.audio.input_device.is_none(), "Default").clicked() {
                                    self.config.audio.input_device = None;
                                    edited = true;
                                }
                                for name in &input_names {
                                    let is_sel = self.config.audio.input_device.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(is_sel, name).clicked() {
                                        self.config.audio.input_device = Some(name.clone());
                                        edited = true;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Output device:");
                        let selected = self.config.audio.output_device.clone()
                            .unwrap_or_else(|| "Default".into());
                        egui::ComboBox::from_id_source("settings_output_device")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.config.audio.output_device.is_none(), "Default").clicked() {
                                    self.config.audio.output_device = None;
                                    edited = true;
                                }
                                for name in &output_names {
                                    let is_sel = self.config.audio.output_device.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(is_sel, name).clicked() {
                                        self.config.audio.output_device = Some(name.clone());
                                        edited = true;
                                    }
                                }
                            });
                        ui.end_row();
                    });

                edited |= ui.checkbox(&mut self.config.audio.echo_cancellation, "Echo cancellation").changed();
                edited |= ui.checkbox(&mut self.config.recording_enabled, "Record calls").changed();
                ui.label(RichText::new(
                    "Recordings saved to ~/.config/deelip/recordings/"
                ).color(palette.muted).small());
            });

            ui.add_space(10.0);

            // ── Network ───────────────────────────────────────────────────
            ui.label(RichText::new("Network").strong());
            ui.group(|ui| {
                egui::Grid::new("settings_network_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Local SIP port:");
                        edited |= ui.add(egui::DragValue::new(&mut self.config.local_sip_port)).changed();
                        ui.end_row();

                        ui.label("STUN server:");
                        edited |= optional_text_field(ui, &mut self.config.stun_server, "e.g. stun.l.google.com:19302");
                        ui.end_row();

                        ui.label("TURN server:");
                        edited |= optional_text_field(ui, &mut self.config.turn_server, "e.g. turn.example.com:3478");
                        ui.end_row();

                        ui.label("TURN username:");
                        edited |= optional_text_field(ui, &mut self.config.turn_username, "");
                        ui.end_row();

                        ui.label("TURN password:");
                        edited |= optional_password_field(ui, &mut self.config.turn_password);
                        ui.end_row();
                    });
            });

            ui.add_space(10.0);

            if ui.button("Save").clicked() {
                match self.config.save(&self.config_path) {
                    Ok(())   => self.settings_saved_notice = true,
                    Err(err) => {
                        self.settings_saved_notice = false;
                        tracing::error!("Failed to save config: {err}");
                    }
                }
            }
            if self.settings_saved_notice {
                ui.label(RichText::new("Saved — restart DeeLip to apply changes.").color(palette.accent));
            }
        });

        if edited {
            self.settings_saved_notice = false;
        }
    }
}

/// List available cpal device names (input or output), for populating the
/// Settings device pickers.
fn list_device_names(input: bool) -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let devices = if input { host.input_devices() } else { host.output_devices() };
    match devices {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(_)      => Vec::new(),
    }
}

/// Text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_text_field(ui: &mut Ui, value: &mut Option<String>, hint: &str) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    ).changed();
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

/// Masked text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_password_field(ui: &mut Ui, value: &mut Option<String>) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui.add(
        egui::TextEdit::singleline(&mut text)
            .password(true)
            .desired_width(f32::INFINITY),
    ).changed();
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn status_bar(ui: &mut Ui, palette: &Palette, text: &str, ok: bool, held: bool) {
    let color = if held {
        palette.warn
    } else if ok {
        palette.accent
    } else {
        palette.warn
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("●").color(color));
        ui.label(text);
    });
}

/// A 3x4 phone-style dial pad (1-9,*,0,#), each digit with the classic small
/// letter caption beneath it (2:ABC .. 9:WXYZ) -- shared between the compose
/// keypad and the in-call DTMF keypad, which were previously two near-identical
/// plain-square-button loops.
fn phone_keypad(ui: &mut Ui, palette: Palette, mut on_press: impl FnMut(char)) {
    const ROWS: [[char; 3]; 4] = [['1', '2', '3'], ['4', '5', '6'], ['7', '8', '9'], ['*', '0', '#']];
    ui.vertical_centered(|ui| {
        for row in ROWS {
            ui.horizontal(|ui| {
                for digit in row {
                    let button = egui::Button::new(keypad_button_text(digit, palette))
                        .rounding(egui::Rounding::same(28.0));
                    if ui.add_sized([56.0, 56.0], button).clicked() {
                        on_press(digit);
                    }
                }
            });
        }
    });
}

fn keypad_button_text(digit: char, palette: Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob { halign: egui::Align::Center, ..Default::default() };
    job.append(
        &digit.to_string(),
        0.0,
        egui::TextFormat { font_id: egui::FontId::proportional(20.0), ..Default::default() },
    );
    let letters = digit_letters(digit);
    if !letters.is_empty() {
        job.append(
            &format!("\n{letters}"),
            0.0,
            egui::TextFormat { font_id: egui::FontId::proportional(9.0), color: palette.muted, ..Default::default() },
        );
    }
    job
}

fn digit_letters(digit: char) -> &'static str {
    match digit {
        '2' => "ABC", '3' => "DEF", '4' => "GHI", '5' => "JKL", '6' => "MNO",
        '7' => "PQRS", '8' => "TUV", '9' => "WXYZ",
        _ => "",
    }
}

/// Display label for an account picker — `display_name` if set, else `user@server`.
fn account_label(account: &SipAccount) -> String {
    match account.display_name.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("{}@{}", account.username, account.server),
    }
}

fn status_filter_label(filter: &Option<CallStatus>) -> &'static str {
    match filter {
        None                        => "All",
        Some(CallStatus::Answered) => "Answered",
        Some(CallStatus::Missed)   => "Missed",
        Some(CallStatus::Rejected) => "Rejected",
        Some(CallStatus::Failed)   => "Failed",
    }
}

/// Quote a CSV field if it contains a comma, quote, or newline.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Shorten a SIP URI for display: `sip:alice@example.com` → `alice@example.com`.
fn short_uri(uri: &str) -> String {
    uri.strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))
        .unwrap_or(uri)
        .to_string()
}

/// Normalize a dial-box entry into a full SIP URI. Bare numbers/usernames
/// (no scheme, no "@") are dialed against the account's own domain, matching
/// how MicroSIP and other softphones resolve local extensions.
fn normalize_target(raw: &str, domain: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("sip:") || lower.starts_with("sips:") {
        raw.to_string()
    } else if raw.contains('@') {
        format!("sip:{raw}")
    } else {
        format!("sip:{raw}@{domain}")
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_duration(secs: u32) -> String {
    if secs < 60 { format!("{secs}s") }
    else         { format!("{}m {:02}s", secs / 60, secs % 60) }
}

fn format_age(ts: u64) -> String {
    let age = unix_now().saturating_sub(ts);
    match age {
        0..=59              => format!("{age}s ago"),
        60..=3599           => format!("{}m ago", age / 60),
        3600..=86399        => format!("{}h ago", age / 3600),
        _                   => format!("{}d ago", age / 86400),
    }
}

fn ctx_key_enter(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

#[cfg(test)]
mod tests {
    use super::normalize_target;

    #[test]
    fn bare_number_gets_domain_appended() {
        assert_eq!(normalize_target("600", "127.0.0.1"), "sip:600@127.0.0.1");
    }

    #[test]
    fn existing_sip_uri_is_untouched() {
        assert_eq!(normalize_target("sip:600@127.0.0.1", "example.com"), "sip:600@127.0.0.1");
    }

    #[test]
    fn sips_uri_is_untouched() {
        assert_eq!(normalize_target("sips:bob@example.com", "example.com"), "sips:bob@example.com");
    }

    #[test]
    fn user_at_host_without_scheme_gets_scheme_added() {
        assert_eq!(normalize_target("bob@example.com", "example.com"), "sip:bob@example.com");
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(normalize_target("  600  ", "127.0.0.1"), "sip:600@127.0.0.1");
    }
}
