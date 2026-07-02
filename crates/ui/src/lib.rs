use std::path::PathBuf;
use std::time::Duration;

use deelip_config::{
    CallDirection, CallHistory, CallRecord, CallStatus,
    Contact, ContactBook,
};
use deelip_media::{alloc_rtp_port, MediaEngine};
use deelip_sip::{
    build_answer, build_hold_offer, build_offer, build_resume_offer,
    parse_sdp, AudioCodec, SipEvent, SipHandle, SrtpParams, SrtpSession,
};
use egui::{Color32, FontId, RichText, Ui};
use tokio::runtime::Handle;

use deelip_nat::TurnRelay;

// ── Tab navigation ────────────────────────────────────────────────────────────

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Tab { #[default] Dialer, History, Contacts }

// ── App state ─────────────────────────────────────────────────────────────────

pub struct DeelipApp {
    sip: SipHandle,
    rt:  Handle,

    tab: Tab,

    // Dialer
    call_target: String,

    // Status
    status_line: String,
    reg_ok:      bool,

    // Active call
    call_id:          Option<String>,
    media:            Option<MediaEngine>,
    local_rtp:        u16,
    is_held:          bool,
    call_codec:       AudioCodec,
    call_dtmf_type:   Option<u8>,
    call_local_srtp:  Option<SrtpParams>,
    call_relay:       Option<TurnRelay>,

    /// Configured TURN server (server, username, password) — when set, every
    /// call relays its RTP through it instead of dialing direct.
    turn_config: Option<(String, String, String)>,

    // Incoming call waiting for user action
    pending_call: Option<PendingCall>,

    // Call timing (for history duration)
    call_start_time: Option<u64>,
    call_direction:  Option<CallDirection>,
    call_remote_uri: Option<String>,

    // History
    history:      CallHistory,
    history_path: Option<PathBuf>,

    // Contacts
    contacts:       ContactBook,
    contacts_path:  Option<PathBuf>,
    contact_search: String,
    new_contact:    Contact,
}

struct PendingCall {
    call_id:    String,
    from:       String,
    remote_sdp: String,
}

impl DeelipApp {
    pub fn new(sip: SipHandle, rt: Handle, turn_config: Option<(String, String, String)>) -> Self {
        let local_rtp = alloc_rtp_port();

        let history_path = CallHistory::default_path().ok();
        let history = history_path.as_deref()
            .and_then(|p| CallHistory::load(p).ok())
            .unwrap_or_default();

        let contacts_path = ContactBook::default_path().ok();
        let contacts = contacts_path.as_deref()
            .and_then(|p| ContactBook::load(p).ok())
            .unwrap_or_default();

        Self {
            sip,
            rt,
            tab:              Tab::Dialer,
            call_target:      String::new(),
            status_line:      "Registering…".into(),
            reg_ok:           false,
            call_id:          None,
            media:            None,
            local_rtp,
            is_held:          false,
            call_codec:       AudioCodec::Pcmu,
            call_dtmf_type:   None,
            call_local_srtp:  None,
            call_relay:       None,
            turn_config,
            pending_call:     None,
            call_start_time:  None,
            call_direction:   None,
            call_remote_uri:  None,
            history,
            history_path,
            contacts,
            contacts_path,
            contact_search:   String::new(),
            new_contact:      Contact::default(),
        }
    }

    // ── SIP event processing ─────────────────────────────────────────────────

    fn process_sip_events(&mut self) {
        while let Ok(event) = self.sip.event_rx.try_recv() {
            match event {
                SipEvent::Registered { expires } => {
                    self.reg_ok      = true;
                    self.status_line = format!("Registered (expires {expires}s)");
                }
                SipEvent::RegistrationFailed { reason } => {
                    self.reg_ok      = false;
                    self.status_line = format!("Registration failed: {reason}");
                }
                SipEvent::CallRinging { .. } => {
                    self.status_line = "Ringing…".into();
                }
                SipEvent::CallConnected { call_id, remote_sdp } => {
                    self.call_id         = Some(call_id.clone());
                    self.status_line     = format!("In call — {}", short_uri(&call_id));
                    self.call_start_time = Some(unix_now());
                    self.start_media(&remote_sdp);
                }
                SipEvent::IncomingCall { call_id, from, remote_sdp } => {
                    self.status_line  = format!("Incoming from {}", short_uri(&from));
                    self.call_direction  = Some(CallDirection::Inbound);
                    self.call_remote_uri = Some(from.clone());
                    self.call_start_time = Some(unix_now());
                    self.pending_call    = Some(PendingCall { call_id, from, remote_sdp });
                }
                SipEvent::CallEnded { call_id } => {
                    self.record_history(CallStatus::Answered);
                    self.end_call();
                    tracing::debug!(call_id, "Call ended normally");
                }
                SipEvent::CallFailed { call_id, code, reason } => {
                    // If there was a pending incoming call that we never answered, it's missed.
                    let status = if self.pending_call.as_ref().is_some_and(|p| p.call_id == call_id) {
                        CallStatus::Missed
                    } else {
                        CallStatus::Failed
                    };
                    self.record_history(status);
                    self.end_call();
                    self.status_line = format!("Call failed ({code}): {reason}");
                }
                SipEvent::CallHeld { .. } => {
                    self.is_held     = true;
                    self.status_line = "Call on hold".into();
                }
                SipEvent::CallResumed { .. } => {
                    self.is_held     = false;
                    self.status_line = format!("In call — {}", self.call_remote_uri.as_deref().unwrap_or("?"));
                }
                SipEvent::RemoteHeld { .. } => {
                    self.status_line = "Remote party put you on hold".into();
                }
                SipEvent::RemoteResumed { .. } => {
                    self.status_line = "Call resumed by remote party".into();
                }
            }
        }
    }

    fn start_media(&mut self, remote_sdp: &str) {
        let Some(parsed) = parse_sdp(remote_sdp) else {
            tracing::error!("Cannot parse remote SDP");
            return;
        };
        self.call_codec      = parsed.codec;
        self.call_dtmf_type  = parsed.dtmf_type;

        let srtp_session = match (&self.call_local_srtp, &parsed.srtp) {
            (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
            _ => {
                if self.sip.secure {
                    tracing::warn!("TLS signaling active but remote SDP has no a=crypto — falling back to plaintext RTP");
                }
                None
            }
        };

        let port    = self.local_rtp;
        let relay   = self.call_relay.as_ref().map(|r| r.conn.clone());
        let rt      = self.rt.clone();
        let engine  = rt.block_on(MediaEngine::start(
            port, parsed.rtp_addr, parsed.codec, parsed.dtmf_type, srtp_session, relay,
        ));
        match engine {
            Ok(e)  => { self.media = Some(e); }
            Err(e) => { tracing::error!("MediaEngine failed: {e}"); }
        }
    }

    /// Resolve the (ip, port) to advertise in this call's SDP. Allocates a
    /// TURN relay on first use if one is configured (`turn_config`), reusing
    /// the same allocation across hold/resume within the call; falls back to
    /// the direct local address if no relay is configured or allocation fails.
    fn resolve_rtp_endpoint(&mut self) -> (String, u16) {
        if self.call_relay.is_none() {
            if let Some((server, username, password)) = self.turn_config.clone() {
                match self.rt.block_on(deelip_nat::allocate_relay(&server, &username, &password)) {
                    Ok(relay) => { self.call_relay = Some(relay); }
                    Err(e) => tracing::warn!("TURN allocation failed ({e}), falling back to direct"),
                }
            }
        }
        match &self.call_relay {
            Some(relay) => (relay.relayed_addr.ip().to_string(), relay.relayed_addr.port()),
            None => (self.sip.advertised_ip.clone(), self.local_rtp),
        }
    }

    fn end_call(&mut self) {
        if let Some(engine) = self.media.take() {
            engine.stop();
        }
        self.call_id         = None;
        self.pending_call    = None;
        self.is_held         = false;
        self.call_start_time = None;
        self.call_direction  = None;
        self.call_remote_uri = None;
        self.call_local_srtp = None;
        self.call_relay      = None;
        self.local_rtp       = alloc_rtp_port();
        self.status_line     = if self.reg_ok { "Ready".into() } else { "Not registered".into() };
    }

    fn record_history(&mut self, status: CallStatus) {
        let Some(start) = self.call_start_time else { return };
        let Some(uri)   = self.call_remote_uri.take() else { return };
        let direction   = self.call_direction.take().unwrap_or(CallDirection::Outbound);
        let duration    = if matches!(status, CallStatus::Answered) {
            (unix_now().saturating_sub(start)) as u32
        } else {
            0
        };
        let record = CallRecord { remote_uri: uri, direction, timestamp: start, duration_secs: duration, status };
        self.history.push(record);
        if let Some(path) = &self.history_path {
            let _ = self.history.save(path);
        }
    }

    // ── Call actions ─────────────────────────────────────────────────────────

    fn do_call(&mut self, target: Option<String>) {
        let raw = target.unwrap_or_else(|| self.call_target.trim().to_string());
        if raw.is_empty() { return; }
        let t = normalize_target(&raw, &self.sip.domain);
        let (rtp_ip, rtp_port) = self.resolve_rtp_endpoint();
        let srtp = if self.sip.secure { Some(SrtpParams::generate()) } else { None };
        let sdp = build_offer(&rtp_ip, rtp_port, srtp.as_ref());
        self.call_local_srtp = srtp;
        self.sip.make_call(&t, sdp);
        self.call_direction  = Some(CallDirection::Outbound);
        self.call_remote_uri = Some(t.clone());
        self.call_start_time = Some(unix_now());
        self.status_line     = format!("Calling {}…", short_uri(&t));
    }

    fn do_accept(&mut self) {
        if let Some(pending) = self.pending_call.take() {
            let codec    = parse_sdp(&pending.remote_sdp).map(|p| p.codec).unwrap_or(AudioCodec::Pcmu);
            let (rtp_ip, rtp_port) = self.resolve_rtp_endpoint();
            let srtp     = if self.sip.secure { Some(SrtpParams::generate()) } else { None };
            let sdp      = build_answer(&rtp_ip, rtp_port, codec, srtp.as_ref());
            self.call_local_srtp = srtp;
            self.sip.accept_call(&pending.call_id, sdp);
            self.call_id     = Some(pending.call_id);
            self.status_line = "Accepted — connecting…".into();
            self.start_media(&pending.remote_sdp);
        }
    }

    fn do_reject(&mut self) {
        if let Some(pending) = self.pending_call.take() {
            self.record_history(CallStatus::Rejected);
            self.sip.reject_call(&pending.call_id);
            self.call_start_time = None;
            self.call_direction  = None;
            self.call_remote_uri = None;
            self.status_line = "Ready".into();
        }
    }

    fn do_hangup(&mut self) {
        if let Some(id) = self.call_id.clone() {
            self.sip.hang_up(&id);
        }
        self.record_history(CallStatus::Answered);
        self.end_call();
    }

    fn do_hold(&mut self) {
        if let Some(id) = self.call_id.clone() {
            let (rtp_ip, rtp_port) = self.resolve_rtp_endpoint();
            let sdp = build_hold_offer(&rtp_ip, rtp_port, self.call_codec, self.call_local_srtp.as_ref());
            self.sip.hold_call(&id, sdp);
        }
    }

    fn do_resume(&mut self) {
        if let Some(id) = self.call_id.clone() {
            let (rtp_ip, rtp_port) = self.resolve_rtp_endpoint();
            let sdp = build_resume_offer(&rtp_ip, rtp_port, self.call_codec, self.call_local_srtp.as_ref());
            self.sip.resume_call(&id, sdp);
        }
    }

    fn do_dtmf(&self, digit: char) {
        if let Some(engine) = &self.media {
            engine.send_dtmf(digit);
        }
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for DeelipApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_sip_events();

        // ── Status bar ───────────────────────────────────────────────────────
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            status_bar(ui, &self.status_line, self.reg_ok, self.is_held);
        });

        // ── Tab bar ──────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Dialer,   "Dialer");
                ui.selectable_value(&mut self.tab, Tab::History,  "History");
                ui.selectable_value(&mut self.tab, Tab::Contacts, "Contacts");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                Tab::Dialer   => self.show_dialer(ui),
                Tab::History  => self.show_history(ui, ctx),
                Tab::Contacts => self.show_contacts(ui, ctx),
            }
        });

        ctx.request_repaint_after(Duration::from_millis(50));
    }

    /// Hang up any in-progress call before the process exits, so the remote
    /// side and server don't keep a dangling channel around. Sending BYE only
    /// queues it on the SipStack's command channel; block briefly so the
    /// background task actually transmits it before the runtime is torn down.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let sent = if let Some(id) = self.call_id.clone() {
            self.sip.hang_up(&id);
            true
        } else if let Some(pending) = self.pending_call.take() {
            self.sip.reject_call(&pending.call_id);
            true
        } else {
            false
        };
        if sent {
            self.rt.block_on(tokio::time::sleep(Duration::from_millis(200)));
        }
    }
}

// ── Tab: Dialer ───────────────────────────────────────────────────────────────

impl DeelipApp {
    fn show_dialer(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        let in_call = self.call_id.is_some() || self.pending_call.is_some();

        // ── Incoming call banner ─────────────────────────────────────────────
        if let Some(pending) = &self.pending_call {
            let from = pending.from.clone();
            ui.group(|ui| {
                ui.label(
                    RichText::new(format!("Incoming call from {}", short_uri(&from)))
                        .color(Color32::YELLOW)
                        .font(FontId::proportional(17.0)),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(" Accept ").color(Color32::GREEN)).clicked() {
                        self.do_accept();
                    }
                    if ui.button(RichText::new(" Reject ").color(Color32::RED)).clicked() {
                        self.do_reject();
                    }
                });
            });
            ui.add_space(8.0);
        }

        // ── Call target + Call/Hang Up buttons ───────────────────────────────
        ui.group(|ui| {
            ui.label("SIP address / number:");
            ui.add_space(4.0);
            let resp = ui.add_enabled(
                !in_call,
                egui::TextEdit::singleline(&mut self.call_target)
                    .hint_text("sip:bob@example.com")
                    .desired_width(f32::INFINITY),
            );
            if resp.lost_focus()
                && ctx_key_enter(ui)
                && !in_call
            {
                self.do_call(None);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if self.call_id.is_some() {
                    // Hold / Resume
                    if self.is_held {
                        if ui.button("Resume").clicked() { self.do_resume(); }
                    } else {
                        if ui.button("Hold").clicked() { self.do_hold(); }
                    }
                    ui.add_space(4.0);
                    if ui.button(RichText::new("Hang Up").color(Color32::RED)).clicked() {
                        self.do_hangup();
                    }
                } else if ui
                    .add_enabled(!in_call && self.reg_ok, egui::Button::new(" Call "))
                    .clicked()
                {
                    self.do_call(None);
                }
            });
        });

        // ── DTMF keypad (only while in active call) ──────────────────────────
        if self.call_id.is_some() {
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.label("DTMF:");
                ui.add_space(4.0);
                for row in &[['1','2','3'], ['4','5','6'], ['7','8','9'], ['*','0','#']] {
                    ui.horizontal(|ui| {
                        for &digit in row {
                            if ui.add_sized([40.0, 34.0], egui::Button::new(digit.to_string())).clicked() {
                                self.do_dtmf(digit);
                            }
                        }
                    });
                }
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

        let mut call_target: Option<String> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for record in &self.history.records {
                let (dir_icon, dir_color) = match record.direction {
                    CallDirection::Inbound  => ("←", Color32::LIGHT_BLUE),
                    CallDirection::Outbound => ("→", Color32::LIGHT_GREEN),
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
                        ui.label(RichText::new(&status_str).color(Color32::GRAY));
                        ui.label(RichText::new(format_age(record.timestamp)).color(Color32::DARK_GRAY));
                    });
                });
                ui.separator();
            }
        });

        if let Some(target) = call_target {
            self.tab         = Tab::Dialer;
            self.call_target = target.clone();
            let in_call = self.call_id.is_some() || self.pending_call.is_some();
            if !in_call && self.reg_ok {
                self.do_call(Some(target));
            }
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
                    .desired_width(f32::INFINITY)
                    .hint_text("name or sip URI"),
            );
        });
        ui.add_space(4.0);

        let mut call_target: Option<String> = None;

        // Contact list
        let results: Vec<(String, String)> = {
            let q = &self.contact_search;
            self.contacts
                .search(q)
                .iter()
                .map(|c| (c.name.clone(), c.sip_uri.clone()))
                .collect()
        };

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                if results.is_empty() {
                    ui.label(RichText::new("No contacts found.").color(Color32::GRAY));
                }
                for (name, uri) in &results {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Call").clicked() {
                                call_target = Some(uri.clone());
                            }
                            ui.label(RichText::new(uri).color(Color32::GRAY));
                        });
                    });
                    ui.separator();
                }
            });

        ui.add_space(8.0);
        ui.separator();

        // Add contact form
        ui.label(RichText::new("Add Contact").strong());
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
        let can_add = !self.new_contact.name.is_empty() && !self.new_contact.sip_uri.is_empty();
        if ui.add_enabled(can_add, egui::Button::new("Save Contact")).clicked() {
            let c = std::mem::take(&mut self.new_contact);
            self.contacts.contacts.push(c);
            if let Some(path) = &self.contacts_path {
                let _ = self.contacts.save(path);
            }
        }

        if let Some(target) = call_target {
            self.tab         = Tab::Dialer;
            self.call_target = target.clone();
            let in_call = self.call_id.is_some() || self.pending_call.is_some();
            if !in_call && self.reg_ok {
                self.do_call(Some(target));
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn status_bar(ui: &mut Ui, text: &str, ok: bool, held: bool) {
    let color = if held {
        Color32::from_rgb(255, 165, 0) // orange = on hold
    } else if ok {
        Color32::GREEN
    } else {
        Color32::YELLOW
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("●").color(color));
        ui.label(text);
    });
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
