use std::time::Duration;

use crate::app::DeelipApp;
use crate::platform::hotkeys::HotkeyAction;
use crate::platform::notify;
use crate::platform::ringtone::{RingKind, Ringtone};
use crate::theme;

impl DeelipApp {
    /// Start/stop the ringtone to match current call state — a no-op if it's
    /// already playing the right thing (or nothing). Called once per frame.
    pub(crate) fn sync_ringtone(&mut self) {
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
            let device = self.config.audio.ringtone_device.as_deref();
            let file   = self.config.audio.ringtone_file.as_deref();
            match Ringtone::start(desired.unwrap(), device, file) {
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
    pub(crate) fn sync_notifications(&mut self) {
        if !self.config.notifications_enabled {
            self.last_notified_call = None;
            return;
        }
        match &self.pending_call {
            Some(p) if self.last_notified_call.as_deref() != Some(p.call_id.as_str()) => {
                self.last_notified_call = Some(p.call_id.clone());
                notify::notify_incoming_call(&p.call_id, &p.from);
            }
            None => self.last_notified_call = None,
            _ => {}
        }
    }

    /// Persist `config` immediately, without the Settings tab's "restart to
    /// apply" notice — for the appearance/notification toggles that apply
    /// live and don't go through the explicit Save button.
    pub(crate) fn save_config_quietly(&self) {
        if let Err(e) = self.config.save(&self.db) {
            tracing::error!("Failed to save config: {e}");
        }
    }

    /// Minimize-to-tray: hide instead of quitting on window close, and
    /// restore/quit in response to tray icon clicks or menu selections.
    /// No-op (falls back to normal close-quits-the-app behavior) if the
    /// tray icon failed to start. Actual click/menu handling happens on
    /// independent background threads (see `tray` module docs) — this just
    /// (a) intercepts close-to-minimize, which can only happen from inside
    /// `update()`, and (b) keeps the background threads' shared state fresh
    /// for whenever they do run.
    pub(crate) fn process_tray_events(&mut self, ctx: &egui::Context) {
        let Some((ctx_slot, quit_state, _)) = &self.tray else { return };

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

    /// Dispatch any global-hotkey presses since the last frame. No-op if
    /// global hotkeys are disabled or failed to register (`self.hotkeys`
    /// is `None`).
    pub(crate) fn process_hotkey_events(&mut self) {
        let Some(hotkeys) = &self.hotkeys else { return };
        for action in hotkeys.poll() {
            match action {
                HotkeyAction::Answer => {
                    if self.pending_call.is_some() { self.do_accept(); }
                }
                HotkeyAction::Hangup => {
                    if let Some(idx) = self.focused_call {
                        self.do_hangup(idx);
                    } else if self.pending_call.is_some() {
                        self.do_reject();
                    } else if !self.calls.is_empty() {
                        self.do_hangup(0);
                    }
                }
                HotkeyAction::Mute => {
                    if self.media.is_some() { self.do_mute_toggle(); }
                }
            }
        }
    }

    /// Dispatch any Accept/Reject notification-button presses since the
    /// last frame. Each action is checked against the *currently* pending
    /// call, not just acted on blindly -- the notification's background
    /// thread can resolve well after its call already ended some other way
    /// (timed out, hung up remotely, answered from the app itself), and a
    /// stale action for a since-gone or already-different call must be
    /// silently ignored rather than accepting/rejecting the wrong thing.
    pub(crate) fn process_notification_actions(&mut self) {
        for (call_id, action) in notify::poll_actions() {
            let Some(pending) = &self.pending_call else { continue };
            if pending.call_id != call_id { continue; }
            match action {
                notify::NotificationAction::Accept => self.do_accept(),
                notify::NotificationAction::Reject => self.do_reject(),
            }
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
        self.process_hotkey_events();
        self.process_notification_actions();

        self.palette = theme::Palette::for_theme(self.config.dark_mode);
        let mut visuals = if self.config.dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() };
        theme::apply_style(ctx, &mut visuals, &self.palette);
        ctx.set_visuals(visuals);

        // ── Status bar ───────────────────────────────────────────────────────
        let on_hold = self.focused_call.is_none() && !self.calls.is_empty();
        let new_voicemail: u32 = self.accounts.iter()
            .filter_map(|a| a.mwi.as_ref())
            .filter(|m| m.waiting)
            .map(|m| m.new_messages)
            .sum();
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            crate::helpers::status_bar(ui, &self.palette, &self.status_line, self.reg_ok, on_hold, new_voicemail);
            if let Some(idx) = self.selected_account_idx() {
                let dnd = self.accounts[idx].account.dnd;
                let (icon, label, color) = if dnd {
                    (egui_phosphor::regular::BELL_SLASH, "DND on", self.palette.danger)
                } else {
                    (egui_phosphor::regular::BELL, "DND off", self.palette.muted)
                };
                if ui.small_button(egui::RichText::new(format!("{icon}  {label}")).color(color))
                    .on_hover_text("Toggle Do Not Disturb for the selected account")
                    .clicked()
                {
                    self.toggle_dnd(idx);
                }
            }
        });

        // ── Tab bar ──────────────────────────────────────────────────────────
        // Selected tab gets an accent-tinted background for free, via
        // `visuals.selection.bg_fill` (set to `palette.accent` in
        // `theme::apply_style` above) -- the same highlight every other
        // selectable widget in the app uses, not a one-off tab-bar special case.
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, crate::app::Tab::Dialer,   format!("{}  Dialer",   egui_phosphor::regular::PHONE));
                let history_label = if self.unseen_missed_calls > 0 {
                    format!("{}  History ({})", egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE, self.unseen_missed_calls)
                } else {
                    format!("{}  History", egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE)
                };
                ui.selectable_value(&mut self.tab, crate::app::Tab::History,  history_label);
                let messages_label = if self.unseen_messages > 0 {
                    format!("{}  Messages ({})", egui_phosphor::regular::CHAT_CIRCLE_TEXT, self.unseen_messages)
                } else {
                    format!("{}  Messages", egui_phosphor::regular::CHAT_CIRCLE_TEXT)
                };
                ui.selectable_value(&mut self.tab, crate::app::Tab::Messages, messages_label);
                ui.selectable_value(&mut self.tab, crate::app::Tab::Contacts, format!("{}  Contacts", egui_phosphor::regular::ADDRESS_BOOK));
                ui.selectable_value(&mut self.tab, crate::app::Tab::Settings, format!("{}  Settings", egui_phosphor::regular::GEAR));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icon = if self.config.dark_mode { egui_phosphor::regular::SUN } else { egui_phosphor::regular::MOON };
                    if ui.button(icon).on_hover_text("Toggle light/dark theme").clicked() {
                        self.config.dark_mode = !self.config.dark_mode;
                        self.save_config_quietly();
                    }
                });
            });
        });

        if self.tab == crate::app::Tab::History && self.unseen_missed_calls > 0 {
            self.unseen_missed_calls = 0;
            self.sync_tray_badge();
        }
        if self.tab == crate::app::Tab::Messages && self.unseen_messages > 0 {
            self.unseen_messages = 0;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                crate::app::Tab::Dialer   => self.show_dialer(ui),
                crate::app::Tab::History  => self.show_history(ui, ctx),
                crate::app::Tab::Messages => self.show_messages(ui),
                crate::app::Tab::Contacts => self.show_contacts(ui, ctx),
                crate::app::Tab::Settings => self.show_settings(ui),
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
            // `tokio::time::sleep(...)` must be constructed *inside* the
            // future block_on drives, not as a bare argument -- as a plain
            // expression it's evaluated before block_on enters the runtime
            // context, and registering a timer with no ambient runtime
            // context panics ("there is no reactor running").
            self.rt.block_on(async { tokio::time::sleep(Duration::from_millis(200)).await });
        }
    }
}
