use std::time::Duration;

use rand::Rng;

use crate::app::{DeelipApp, SharedApp};
use crate::platform::hotkeys::HotkeyAction;
use crate::platform::notify;
use crate::platform::ringtone::{RingKind, Ringtone};
use crate::strings::{t, tf};
use crate::theme;

/// Random-position counterpart to `egui::ViewportCommand::center_on_screen`
/// -- same monitor-size/window-size math, but a uniformly random spot
/// instead of dead center. `None` under the same conditions
/// `center_on_screen` gives up (no outer-rect or monitor-size info yet,
/// e.g. the very first frame).
fn random_position_on_screen(ctx: &egui::Context) -> Option<egui::ViewportCommand> {
    ctx.input(|i| {
        let outer_rect = i.viewport().outer_rect?;
        let size = outer_rect.size();
        let monitor_size = i.viewport().monitor_size?;
        let max_x = monitor_size.x - size.x;
        let max_y = monitor_size.y - size.y;
        if max_x > 1.0 && max_y > 1.0 {
            let mut rng = rand::thread_rng();
            let x = rng.gen_range(0.0..max_x);
            let y = rng.gen_range(0.0..max_y);
            Some(egui::ViewportCommand::OuterPosition([x, y].into()))
        } else {
            None
        }
    })
}

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
            let file = self.config.audio.ringtone_file.as_deref();
            let volume = self.config.audio.ringtone_volume;
            match Ringtone::start(desired.unwrap(), device, file, volume) {
                Ok(r) => self.ringtone = Some(r),
                Err(e) => tracing::warn!("Ringtone failed to start: {e}"),
            }
        } else if !is_ringing {
            self.ringtone = None;
        }
        self.was_ringing = is_ringing;
    }

    /// Raise/focus the main window once per incoming call -- deliberately
    /// not gated on `notifications_enabled` (a user who disabled desktop
    /// notifications may still want the window to come to front), so this
    /// tracks its own rising edge rather than reusing `sync_notifications`'s.
    /// Reuses the exact `ViewportCommand` pair the tray icon's own
    /// restore-from-tray path already uses (`platform::tray::restore_window`).
    /// Called once per frame.
    pub(crate) fn sync_window_raise(&mut self, ctx: &egui::Context) {
        match &self.pending_call {
            Some(p) if self.last_raised_call.as_deref() != Some(p.call_id.as_str()) => {
                self.last_raised_call = Some(p.call_id.clone());
                if self.config.random_popup_position {
                    if let Some(cmd) = random_position_on_screen(ctx) {
                        ctx.send_viewport_cmd(cmd);
                    }
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            None => self.last_raised_call = None,
            _ => {}
        }
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
                notify::notify_incoming_call(&p.call_id, &p.from, self.ctx_slot.clone());
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
        let Some((_, quit_state, _)) = &self.tray else {
            return;
        };

        // Only rebuild (clone Senders/call-ids, take the lock) when the set
        // of live/pending calls actually changed since last frame -- this
        // ran unconditionally every single frame before, regardless of
        // whether `self.calls`/`self.pending_call` changed at all. The
        // comparison itself borrows rather than allocates, so the common
        // (unchanged) case costs nothing beyond a cheap scan.
        let calls_changed = self.calls.len() != self.tray_calls_key.len()
            || self.calls.iter().zip(&self.tray_calls_key).any(|(c, (acc, id))| c.account != *acc || c.call_id != *id);
        if calls_changed {
            self.tray_calls_key = self.calls.iter().map(|c| (c.account, c.call_id.clone())).collect();
            *quit_state.calls.lock().unwrap() = self
                .tray_calls_key
                .iter()
                .map(|(account, call_id)| (self.accounts[*account].handle.cmd_tx.clone(), call_id.clone()))
                .collect();
        }

        let pending_changed = match (&self.pending_call, &self.tray_pending_key) {
            (Some(p), Some((acc, id))) => p.account != *acc || p.call_id != *id,
            (None, None) => false,
            _ => true,
        };
        if pending_changed {
            self.tray_pending_key = self.pending_call.as_ref().map(|p| (p.account, p.call_id.clone()));
            *quit_state.pending.lock().unwrap() = self
                .tray_pending_key
                .as_ref()
                .map(|(account, call_id)| (self.accounts[*account].handle.cmd_tx.clone(), call_id.clone()));
        }

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
                    if self.pending_call.is_some() {
                        self.do_accept();
                    }
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
                    if self.media.is_some() {
                        self.do_mute_toggle();
                    }
                }
                HotkeyAction::MediaHook => {
                    if self.pending_call.is_some() {
                        self.do_accept();
                    } else if let Some(idx) = self.focused_call {
                        self.do_hangup(idx);
                    } else if !self.calls.is_empty() {
                        self.do_hangup(0);
                    }
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
            let Some(pending) = &self.pending_call else {
                continue;
            };
            if pending.call_id != call_id {
                continue;
            }
            match action {
                notify::NotificationAction::Accept => self.do_accept(),
                notify::NotificationAction::Reject => self.do_reject(),
            }
        }
    }
}

impl eframe::App for SharedApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let self_arc = self.clone();
        self.0.lock().unwrap().update_inner(ctx, frame, &self_arc);
    }

    /// Hang up any in-progress call before the process exits, so the remote
    /// side and server don't keep a dangling channel around.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.0.lock().unwrap().hangup_before_exit();
    }
}

impl DeelipApp {
    fn update_inner(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame, self_arc: &SharedApp) {
        // Refreshed unconditionally, every frame, regardless of tray/call
        // state -- see `ctx_slot`'s doc comment on `DeelipApp`. Background
        // producers (SIP, hotkeys, notifications, update-check, directory
        // search, device scans) read this to call `request_repaint()` the
        // instant they have something, rather than the UI having to poll.
        *self.ctx_slot.lock().unwrap() = Some(ctx.clone());

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
        self.sync_window_raise(ctx);
        self.sync_notifications();
        self.process_tray_events(ctx);
        self.process_hotkey_events();
        self.process_notification_actions();
        self.process_update_events();
        self.process_directory_events();

        let mut visuals = egui::Visuals::light();
        theme::apply_style(ctx, &mut visuals, &self.palette);
        ctx.set_visuals(visuals);

        // ── Tab bar ──────────────────────────────────────────────────────────
        // Selected tab gets an accent-tinted background for free, via
        // `visuals.selection.bg_fill` (set to `palette.signal` in
        // `theme::apply_style` above) -- the same highlight every other
        // selectable widget in the app uses, not a one-off tab-bar special case.
        //
        // The History label is recomputed only when its unseen count
        // actually changed since last frame, instead of `format!()`ing a
        // fresh String every single frame regardless (this runs at ~20fps
        // continuously) -- same cache-and-compare idiom as
        // `history_filter_key`/`audio_device_cache`.
        if self.history_tab_label_cache.0 != self.unseen_missed_calls {
            self.history_tab_label_cache = (
                self.unseen_missed_calls,
                if self.unseen_missed_calls > 0 {
                    format!(
                        "{}  {}",
                        egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE,
                        tf("nav.history_tab_with_count", &[("count", &self.unseen_missed_calls.to_string())])
                    )
                } else {
                    format!("{}  {}", egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE, t("nav.history_tab"))
                },
            );
        }
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.tab,
                    crate::app::Tab::Dialer,
                    format!("{}  {}", egui_phosphor::regular::PHONE, t("nav.dialer_tab")),
                );
                ui.selectable_value(&mut self.tab, crate::app::Tab::History, self.history_tab_label_cache.1.as_str());
                ui.selectable_value(
                    &mut self.tab,
                    crate::app::Tab::Contacts,
                    format!("{}  {}", egui_phosphor::regular::ADDRESS_BOOK, t("nav.contacts_tab")),
                );
                ui.selectable_value(
                    &mut self.tab,
                    crate::app::Tab::Directory,
                    format!("{}  {}", egui_phosphor::regular::BUILDINGS, t("nav.directory_tab")),
                );
                // Settings lives in its own modal dialog (MicroSIP-style),
                // not a tab -- opened via this gear button, right-aligned
                // like MicroSIP's own tab-row "more" affordance. Messages
                // has no tab-bar entry point at all -- see
                // `messages_window_open`'s doc comment.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(egui_phosphor::regular::GEAR).on_hover_text(t("settings.window_title")).clicked() {
                        self.settings_open = true;
                    }
                });
            });
        });

        if self.tab == crate::app::Tab::History && self.unseen_missed_calls > 0 {
            self.unseen_missed_calls = 0;
            self.sync_tray_badge();
        }

        // ── Status bar (bottom, MicroSIP-style) ───────────────────────────────
        // One row: connection dot + status text on the left; voicemail badge,
        // DND toggle, and the selected account's label on the right, in that
        // left-to-right order (added right-to-left so the account label lands
        // pinned to the far right edge, mirroring MicroSIP's "● Online ...
        // extension" bar).
        let on_hold = self.focused_call.is_none() && !self.calls.is_empty();
        let new_voicemail: u32 =
            self.accounts.iter().filter_map(|a| a.mwi.as_ref()).filter(|m| m.waiting).map(|m| m.new_messages).sum();
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                crate::helpers::status_bar(ui, &self.palette, &self.status_line, self.reg_ok, on_hold);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(idx) = self.selected_account_idx() {
                        ui.label(egui::RichText::new(&self.accounts[idx].label).color(self.palette.ink_muted).small());
                        ui.add_space(8.0);
                        let dnd = self.accounts[idx].account.dnd;
                        let (icon, color) = if dnd {
                            (egui_phosphor::regular::BELL_SLASH, self.palette.danger)
                        } else {
                            (egui_phosphor::regular::BELL, self.palette.ink_muted)
                        };
                        if ui
                            .small_button(egui::RichText::new(icon).color(color))
                            .on_hover_text(if dnd { t("nav.dnd_on_hover") } else { t("nav.dnd_off_hover") })
                            .clicked()
                        {
                            self.toggle_dnd(idx);
                        }
                    }
                    if new_voicemail > 0 {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("{} {new_voicemail}", egui_phosphor::regular::VOICEMAIL))
                                .color(self.palette.signal),
                        );
                    }
                });
            });
        });

        // Explicit margin, not egui's own default (`Style::window_margin`,
        // 6px) -- too tight once lived with, part of the same "add
        // breathing room everywhere" feedback `theme::apply_style`'s v3.1
        // spacing bump addresses.
        let central_frame = egui::Frame::central_panel(&ctx.style()).inner_margin(14.0);
        egui::CentralPanel::default().frame(central_frame).show(ctx, |ui| match self.tab {
            crate::app::Tab::Dialer => self.show_dialer(ui),
            crate::app::Tab::History => self.show_history(ui, ctx),
            crate::app::Tab::Contacts => self.show_contacts(ui, ctx),
            crate::app::Tab::Directory => self.show_directory(ui, ctx),
        });

        self.show_settings_modal(ctx, self_arc.clone());
        self.show_messages_window(ctx, self_arc.clone());
        self.show_transfer_window(ctx, self_arc.clone());
        self.show_dtmf_window(ctx, self_arc.clone());
        self.show_update_popup(ctx);
        self.show_contact_dialog(ctx, self_arc.clone());

        // The ~20fps cadence only actually matters while there's a call to
        // animate/tick (the ringing dot's pulse, the connected-call duration
        // counter) -- see `RingState`'s doc comment in `dialer.rs`. Those are
        // the only things left with no waker of their own: SIP/hotkey/tray/
        // notification/update-check/directory-search/device-scan events all
        // now push their own `request_repaint()` via `ctx_slot` the instant
        // they have something (see that field's doc comment), instead of
        // relying on this tick to notice them. So the idle branch below is
        // now just a rare safety net, not the primary way anything gets
        // noticed -- kept long on purpose: every tick also fully rebuilds
        // whatever's in the Settings viewport (`show_settings_modal` above),
        // and forcing that (plus the main window's own redraw) on a short
        // fixed schedule while idle is what caused real lag: GNOME/Mutter
        // throttles frame delivery for whichever of DeeLip's two windows
        // *isn't* focused (confirmed live -- unrelated to whether it's
        // visually occluded), and since both windows share one render
        // thread, a throttled main-window redraw directly delayed Settings'
        // own next frame whenever Settings had focus and the idle tick fired.
        let has_live_call = self.pending_call.is_some() || self.pending_outbound.is_some() || !self.calls.is_empty();
        let repaint_interval = if has_live_call { Duration::from_millis(50) } else { Duration::from_secs(2) };
        ctx.request_repaint_after(repaint_interval);
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
