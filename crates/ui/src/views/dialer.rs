use deelip_config::CallDirection;
use deelip_media::video_codec::Yuv420Frame;
use deelip_sip::AudioCodec;
use egui::{Align2, Color32, RichText, Ui};

use crate::app::{DeelipApp, VideoViewCache};
use crate::helpers::{
    account_status_label, audio_codec_label, ctx_key_enter, empty_state, format_call_timer,
    phone_keypad, short_uri, unix_now,
};
use crate::theme::{self, Palette};

/// Width the idle dial pad and the in-call "stage" are capped to and
/// centered within -- the address field and keypad are the whole point of
/// the idle screen, so they read as one small, deliberate instrument
/// instead of stretching edge-to-edge in a resized window.
const STAGE_WIDTH: f32 = 280.0;

impl DeelipApp {
    pub(crate) fn show_dialer(&mut self, ui: &mut Ui) {
        let idle =
            self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
        if idle {
            self.show_dialer_idle(ui);
        } else {
            self.show_dialer_in_call(ui);
        }
    }

    // ── Idle: number entry + keypad ───────────────────────────────────────

    fn show_dialer_idle(&mut self, ui: &mut Ui) {
        if self.accounts.len() > 1 {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Call from").color(self.palette.ink_muted).small());
                let current = self.selected_account_idx().unwrap_or(0);
                let palette = self.palette;
                let selected_label = {
                    let acc = &self.accounts[current];
                    account_status_label(ui, &palette, acc.reg_ok, &acc.label)
                };
                egui::ComboBox::from_id_source("dialer_account_picker")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for i in 0..self.accounts.len() {
                            let acc = &self.accounts[i];
                            let label = account_status_label(ui, &palette, acc.reg_ok, &acc.label);
                            if ui
                                .add(egui::SelectableLabel::new(current == i, label))
                                .clicked()
                            {
                                self.selected_account = i;
                                self.refresh_idle_status();
                            }
                        }
                    });
            });
            ui.add_space(6.0);
        }

        if let Some(current) = self.selected_account_idx() {
            let mut auto_answer = self.accounts[current].account.auto_answer_enabled;
            if ui.checkbox(&mut auto_answer, "Auto-answer incoming calls").changed() {
                self.toggle_auto_answer(current);
            }
            ui.add_space(10.0);
        }

        // A centered fixed-width column, not `ui.vertical_centered` -- that
        // only centers single fixed-size children; a nested `ui.horizontal`
        // row (the keypad, the backspace/clear row) reports its own
        // min_rect starting flush at the container's left edge, so it
        // doesn't get centered by the parent layout at all (see
        // `phone_keypad`'s doc comment). Fixing this column's width once
        // and centering *it* means everything inside -- including
        // `phone_keypad`'s own internal row-centering -- lines up
        // consistently against the same `STAGE_WIDTH`.
        let margin = ((ui.available_width() - STAGE_WIDTH) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(margin);
            ui.vertical(|ui| {
                ui.set_width(STAGE_WIDTH);

                let resp = ui.add_sized(
                    [STAGE_WIDTH, 38.0],
                    egui::TextEdit::singleline(&mut self.call_target)
                        .hint_text("sip:address or number")
                        .font(egui::FontId::new(15.0, egui::FontFamily::Monospace))
                        .horizontal_align(egui::Align::Center),
                );
                if resp.lost_focus() && ctx_key_enter(ui) {
                    self.do_call(None);
                }
                ui.add_space(18.0);

                let palette = self.palette;
                phone_keypad(ui, palette, |digit| self.call_target.push(digit));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let row_width = 56.0 + ui.spacing().item_spacing.x + 52.0;
                    ui.add_space(((STAGE_WIDTH - row_width) / 2.0).max(0.0));
                    // Plain Unicode, not `egui_phosphor::regular::BACKSPACE` --
                    // see the crate-level note on the broken icon set in
                    // `theme.rs`.
                    if ui
                        .add_enabled(
                            !self.call_target.is_empty(),
                            egui::Button::new("⌫"),
                        )
                        .clicked()
                    {
                        self.call_target.pop();
                    }
                    if ui
                        .add_enabled(!self.call_target.is_empty(), egui::Button::new("Clear"))
                        .clicked()
                    {
                        self.call_target.clear();
                    }
                });
                ui.add_space(16.0);

                let call_text = RichText::new(format!("{}  Call", egui_phosphor::regular::PHONE))
                    .font(theme::font_medium(15.0))
                    .color(Color32::WHITE);
                if ui
                    .add_sized(
                        [STAGE_WIDTH, 42.0],
                        egui::Button::new(call_text)
                            .fill(self.palette.signal)
                            .rounding(egui::Rounding::same(10.0)),
                    )
                    .clicked()
                {
                    self.do_call(None);
                }

                let can_redial = self.reg_ok && self.last_dialed.is_some();
                if can_redial {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        let redial_text = RichText::new(format!(
                            "{}  Redial",
                            egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE
                        ))
                        .color(self.palette.ink_muted)
                        .small();
                        if ui.add(egui::Button::new(redial_text).frame(false)).clicked() {
                            self.do_redial();
                        }
                    });
                }
            });
        });
    }

    // ── In-call: focused call screen -- replaces the keypad entirely while
    // ringing/dialing/connected, instead of stacking status boxes above it ──

    fn show_dialer_in_call(&mut self, ui: &mut Ui) {
        // A fresh incoming call takes over the whole screen; a *second*
        // incoming call while one is already active is shown as a compact
        // banner above the existing in-call content instead.
        if let Some(from) = self.pending_call.as_ref().map(|p| p.from.clone()) {
            if self.calls.is_empty() {
                self.show_incoming_call_screen(ui, &from);
                return;
            }
            self.show_call_waiting_banner(ui, &from);
        }

        if self.calls.is_empty() {
            if let Some(target) = self.pending_outbound.as_ref().map(|o| o.remote_uri.clone()) {
                self.show_dialing_screen(ui, &target);
            }
            return;
        }

        self.show_active_calls(ui);
    }

    fn show_incoming_call_screen(&mut self, ui: &mut Ui, from: &str) {
        let (name, is_name) = self.caller_display(from);
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            call_avatar(ui, &self.palette, &name, RingState::Pending);
            ui.add_space(8.0);
            caller_name_label(ui, &self.palette, &name, is_name);
            ui.add_space(4.0);
            state_badge(ui, "ringing", self.palette.ringing);
        });
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            let spacing = ui.available_width() * 0.16;
            ui.add_space(spacing);
            if circular_action_button(ui, egui_phosphor::regular::PHONE, self.palette.signal) {
                self.do_accept();
            }
            ui.add_space(ui.available_width() - spacing - 44.0);
            if circular_action_button(ui, egui_phosphor::regular::PHONE_X, self.palette.danger) {
                self.do_reject();
            }
        });
    }

    fn show_call_waiting_banner(&mut self, ui: &mut Ui, from: &str) {
        let (name, _) = self.caller_display(from);
        theme::full_width_card(ui, self.palette, |ui| {
            ui.label(
                RichText::new(format!("Call waiting -- {name}"))
                    .color(self.palette.ringing)
                    .font(theme::font_medium(14.0)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let accept = format!("{}  Accept", egui_phosphor::regular::PHONE);
                if ui
                    .button(RichText::new(accept).color(self.palette.signal))
                    .clicked()
                {
                    self.do_accept();
                }
                let reject = format!("{}  Reject", egui_phosphor::regular::PHONE_X);
                if ui
                    .button(RichText::new(reject).color(self.palette.danger))
                    .clicked()
                {
                    self.do_reject();
                }
            });
        });
        ui.add_space(8.0);
    }

    fn show_dialing_screen(&mut self, ui: &mut Ui, target: &str) {
        let (name, is_name) = self.caller_display(target);
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            call_avatar(ui, &self.palette, &name, RingState::Pending);
            ui.add_space(8.0);
            caller_name_label(ui, &self.palette, &name, is_name);
            ui.add_space(4.0);
            state_badge(ui, "calling", self.palette.ringing);
        });
    }

    /// Resolve a raw SIP URI to a contact's name when one exists, matching
    /// History/Contacts' own `display_name` convention -- returns whether a
    /// real *name* was found, so the caller can render a resolved name in
    /// Inter and a bare address in JetBrains Mono (the redesign's one
    /// typographic rule: numbers/addresses are mono, names are Inter).
    fn caller_display(&self, uri: &str) -> (String, bool) {
        match self.contacts.find_by_uri(uri) {
            Some(c) => (c.name.clone(), true),
            None => (short_uri(uri), false),
        }
    }

    fn show_active_calls(&mut self, ui: &mut Ui) {
        let mut hangup_idx: Option<usize> = None;
        let mut hold_idx: Option<usize> = None;
        let mut swap_idx: Option<usize> = None;

        // If an attended-transfer consultation call is currently ringing,
        // its `pending_outbound` coexists with the held original call --
        // surface it as a small line rather than silently showing nothing.
        if let Some(out) = &self.pending_outbound {
            ui.label(
                RichText::new(format!("Calling {}…", short_uri(&out.remote_uri)))
                    .color(self.palette.ink_muted),
            );
            ui.add_space(6.0);
        }

        for idx in 0..self.calls.len() {
            let focused = self.focused_call == Some(idx);
            let (dir_icon, uri, start_time) = {
                let call = &self.calls[idx];
                let dir_icon = match call.direction {
                    CallDirection::Inbound => egui_phosphor::regular::PHONE_INCOMING,
                    CallDirection::Outbound => egui_phosphor::regular::PHONE_OUTGOING,
                };
                (dir_icon, call.remote_uri.clone(), call.start_time)
            };
            let (name, is_name) = self.caller_display(&uri);

            if focused {
                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    call_avatar(ui, &self.palette, &name, RingState::Connected);
                    ui.add_space(8.0);
                    caller_name_label(ui, &self.palette, &name, is_name);
                    ui.add_space(4.0);
                    let state = if self.in_conference { "in conference" } else { "connected" };
                    state_badge(ui, state, self.palette.signal);
                    ui.add_space(2.0);
                    let elapsed = unix_now().saturating_sub(start_time);
                    ui.label(
                        RichText::new(format_call_timer(elapsed))
                            .font(theme::font_mono_medium(24.0))
                            .color(self.palette.ink),
                    );
                    if self.calls[idx].media.video.is_some() {
                        ui.add_space(8.0);
                        self.show_video_panel(ui);
                    }
                    if self.is_recording() {
                        ui.add_space(4.0);
                        ui.label(RichText::new("● REC").color(self.palette.danger).small());
                    }
                    if let Some(sas) = self.media.as_ref().and_then(|m| m.zrtp_sas()) {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("🔒 ZRTP SAS: {sas}"))
                                .font(egui::FontId::new(12.5, egui::FontFamily::Monospace))
                                .color(self.palette.signal),
                        )
                        .on_hover_text(
                            "Read this 4-character code out loud with the other party to \
                             confirm no one is intercepting this call.",
                        );
                    }
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if circular_action_button(
                            ui,
                            egui_phosphor::regular::PHONE_X,
                            self.palette.danger,
                        ) {
                            hangup_idx = Some(idx);
                        }
                        if !self.in_conference {
                            ui.add_space(10.0);
                            let b = egui::Button::new(
                                RichText::new(egui_phosphor::regular::PHONE_PAUSE)
                                    .size(20.0)
                                    .color(self.palette.ink_muted),
                            )
                            .fill(self.palette.surface)
                            .rounding(egui::Rounding::same(32.0));
                            if ui.add_sized([56.0, 56.0], b).clicked() {
                                hold_idx = Some(idx);
                            }
                        }
                    });
                });
                ui.add_space(12.0);
            } else {
                theme::full_width_card(ui, self.palette, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(dir_icon).color(self.palette.ink_muted));
                        if is_name {
                            ui.label(RichText::new(&name).font(theme::font_medium(14.0)));
                        } else {
                            ui.label(
                                RichText::new(&name)
                                    .font(egui::FontId::new(13.0, egui::FontFamily::Monospace)),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hang_up = format!("{}  Hang Up", egui_phosphor::regular::PHONE_X);
                            if ui
                                .button(RichText::new(hang_up).color(self.palette.danger))
                                .clicked()
                            {
                                hangup_idx = Some(idx);
                            }
                            ui.add_space(4.0);
                            if !self.in_conference {
                                let resume = format!("{}  Resume", egui_phosphor::regular::PLAY);
                                if ui.button(resume).clicked() {
                                    swap_idx = Some(idx);
                                }
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("on hold")
                                    .font(egui::FontId::new(11.0, egui::FontFamily::Monospace))
                                    .color(self.palette.ringing),
                            );
                        });
                    });
                });
                ui.add_space(6.0);
            }
        }

        if let Some(idx) = hangup_idx {
            self.do_hangup(idx);
        }
        if let Some(idx) = hold_idx {
            self.do_hold_slot(idx);
        }
        if let Some(idx) = swap_idx {
            self.do_swap_to(idx);
        }

        if self.calls.len() == 2 && !self.in_conference {
            let merge = format!("{}  Merge into Conference", egui_phosphor::regular::PHONE);
            if ui.button(merge).clicked() {
                self.start_conference();
            }
            ui.add_space(6.0);
        }

        if self.focused_call.is_some() {
            self.show_focused_call_controls(ui);
        }
    }

    /// Renders the focused call's video panel (self-view + remote), if this
    /// call negotiated a video leg. Reads the latest camera/decoded frames
    /// first (a short immutable borrow of `self.video`), updates each
    /// side's cached egui texture only if the frame actually changed (a
    /// separate short mutable borrow -- can't hold both borrows in one
    /// closure), then draws them from a final immutable borrow. Avoids
    /// re-uploading an unchanged GPU texture on every repaint (egui
    /// repaints far faster than either camera or decode framerate).
    fn show_video_panel(&mut self, ui: &mut Ui) {
        if self.video.is_none() {
            return;
        }
        let remote_frame = self.video.as_ref().and_then(|v| v.engine.latest_decoded_frame());
        let local_frame = self
            .video
            .as_ref()
            .and_then(|v| v.camera.as_ref())
            .and_then(|c| c.latest_frame());

        let ctx = ui.ctx().clone();
        if let Some(v) = self.video.as_mut() {
            update_video_view(&ctx, &mut v.remote, remote_frame, "deelip_remote_video");
            update_video_view(&ctx, &mut v.local, local_frame, "deelip_local_video");
        }

        let palette = self.palette;
        if let Some(v) = self.video.as_ref() {
            ui.horizontal(|ui| {
                show_video_view(ui, &palette, &v.remote, "Remote");
                ui.add_space(8.0);
                show_video_view(ui, &palette, &v.local, "You");
            });
        }
        ui.add_space(4.0);
    }

    fn show_focused_call_controls(&mut self, ui: &mut Ui) {
        theme::full_width_card(ui, self.palette, |ui| {
            ui.horizontal(|ui| {
                let mute_icon = if self.is_muted() {
                    egui_phosphor::regular::MICROPHONE_SLASH
                } else {
                    egui_phosphor::regular::MICROPHONE
                };
                let mute_label = format!(
                    "{mute_icon}  {}",
                    if self.is_muted() { "Unmute" } else { "Mute" }
                );
                if ui.button(mute_label).clicked() {
                    self.do_mute_toggle();
                }
                let record_label = format!(
                    "{}  {}",
                    egui_phosphor::regular::RECORD,
                    if self.is_recording() { "Stop recording" } else { "Record" }
                );
                if ui.button(record_label).clicked() {
                    self.do_record_toggle();
                }
                let transfer_label = format!(
                    "{}  {}",
                    "↱", // plain Unicode -- see the broken-icon note in `theme.rs`
                    if self.showing_transfer {
                        "Cancel transfer"
                    } else {
                        "Transfer"
                    }
                );
                if ui.button(transfer_label).clicked() {
                    self.showing_transfer = !self.showing_transfer;
                }
                let can_attend = self.calls.len() == 1;
                let attended_label = format!(
                    "{}  {}",
                    "↱", // plain Unicode -- see the broken-icon note in `theme.rs`
                    if self.showing_attended {
                        "Cancel attended"
                    } else {
                        "Attended"
                    }
                );
                if ui
                    .add_enabled(can_attend, egui::Button::new(attended_label))
                    .clicked()
                {
                    self.showing_attended = !self.showing_attended;
                }
                let dtmf_label = format!("{}  Keypad", egui_phosphor::regular::NUMPAD);
                if ui.selectable_label(self.showing_dtmf, dtmf_label).clicked() {
                    self.showing_dtmf = !self.showing_dtmf;
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui_phosphor::regular::SPEAKER_HIGH);
                let mut out_gain = self.output_gain();
                if ui.add(egui::Slider::new(&mut out_gain, 0.0..=2.0).show_value(false)).changed() {
                    self.set_output_gain(out_gain);
                }
                ui.add_space(8.0);
                ui.label(egui_phosphor::regular::MICROPHONE);
                let mut in_gain = self.input_gain();
                if ui.add(egui::Slider::new(&mut in_gain, 0.0..=2.0).show_value(false)).changed() {
                    self.set_input_gain(in_gain);
                }
            });
            if self.showing_transfer {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.transfer_target)
                            .hint_text("sip:carol@example.com")
                            .font(egui::FontId::new(13.0, egui::FontFamily::Monospace))
                            .desired_width(f32::INFINITY),
                    );
                    if ui.button("Send").clicked() {
                        self.do_transfer();
                    }
                });
            }
            if self.showing_attended {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.attended_target)
                            .hint_text("sip:carol@example.com")
                            .font(egui::FontId::new(13.0, egui::FontFamily::Monospace))
                            .desired_width(f32::INFINITY),
                    );
                    if ui.button("Call").clicked() {
                        self.do_attended_transfer_dial();
                    }
                });
            }
            if self.attended_transfer_original.is_some() && self.calls.len() == 2 {
                ui.add_space(4.0);
                let complete = format!(
                    "{}  Complete Transfer",
                    egui_phosphor::regular::CHECK_CIRCLE
                );
                if ui.button(complete).clicked() {
                    self.do_complete_attended_transfer();
                }
            }
        });

        if self.showing_dtmf {
            ui.add_space(8.0);
            let palette = self.palette;
            theme::full_width_card(ui, palette, |ui| {
                phone_keypad(ui, palette, |digit| self.do_dtmf(digit));
            });
        }

        if let Some(engine) = self.media.as_ref() {
            ui.add_space(8.0);
            let stats = engine.stats();
            let muted_color = self.palette.ink_muted;
            egui::CollapsingHeader::new("Call statistics").show(ui, |ui| {
                if self.in_conference && self.calls.len() == 2 {
                    show_leg_stats(
                        ui,
                        &short_uri(&self.calls[0].remote_uri),
                        self.calls[0].media.codec,
                        &stats.leg1,
                        muted_color,
                    );
                    if let Some(leg2) = stats.leg2.as_ref() {
                        ui.add_space(4.0);
                        show_leg_stats(
                            ui,
                            &short_uri(&self.calls[1].remote_uri),
                            self.calls[1].media.codec,
                            leg2,
                            muted_color,
                        );
                    }
                } else if let Some(idx) = self.focused_call {
                    show_leg_stats(
                        ui,
                        "This call",
                        self.calls[idx].media.codec,
                        &stats.leg1,
                        muted_color,
                    );
                }
            });
        }
    }
}

/// Which state `call_avatar`/`state_badge` reflect -- `Pending`
/// (ringing/dialing/hold) gets a softly pulsing amber status dot since it
/// wants attention; `Connected` settles to a static `signal`-colored dot,
/// since a live call is a stable state, not an urgent one.
///
/// v2 note: the original pass used a large animated dual-ring pulse
/// (concentric circles expanding outward around the avatar) as the app's
/// signature element. User feedback on that first pass was that it read as
/// too playful -- a big bouncing shape, not a serious instrument. This is
/// the toned-down replacement: a small static avatar with a corner status
/// dot (the same "live status" convention Slack/Stripe/Notion use) plus a
/// separate text badge, not a hero animation. The dot still animates for
/// `Pending` (a slow opacity fade, not a bounce), reusing the app's
/// existing ~20fps repaint cadence (`frame.rs`'s `request_repaint_after`)
/// as its clock rather than requesting its own.
#[derive(Clone, Copy, PartialEq)]
enum RingState {
    Pending,
    Connected,
}

/// A caller initial on a small surface circle, with a state-colored status
/// dot at its corner -- see the `RingState` doc comment for why this
/// replaced the original pass's big pulse-ring animation.
fn call_avatar(ui: &mut Ui, palette: &Palette, display_name: &str, state: RingState) {
    let avatar_d = 52.0;
    let pad = 8.0; // room for the status dot to sit outside the avatar's own edge
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(avatar_d + pad, avatar_d + pad), egui::Sense::hover());
    let center = rect.center() - egui::vec2(pad / 2.0, pad / 2.0);
    let painter = ui.painter();
    let avatar_r = avatar_d / 2.0;

    painter.circle_filled(center, avatar_r, palette.surface);
    painter.circle_stroke(center, avatar_r, egui::Stroke::new(1.0, palette.border));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        avatar_initial(display_name).to_string(),
        theme::font_heading(17.0),
        palette.ink,
    );

    let dot_color = match state {
        RingState::Pending => palette.ringing,
        RingState::Connected => palette.signal,
    };
    let dot_alpha = match state {
        RingState::Pending => {
            // A slow opacity fade, not a bounce -- see the `RingState` doc
            // comment. No extra `request_repaint()` here -- `frame.rs`'s
            // own `request_repaint_after(50ms)` cadence already redraws
            // this often enough for a slow fade to read as smooth.
            let t = ui.input(|i| i.time) as f32;
            let phase = (t * 1.6).sin() * 0.5 + 0.5;
            (110.0 + phase * 145.0) as u8
        }
        RingState::Connected => 255,
    };
    let dot_center = center + egui::vec2(avatar_r * 0.78, avatar_r * 0.78);
    // A canvas-colored ring first, so the dot reads as sitting on top of
    // (cut out from) the avatar's own edge rather than overlapping it raw.
    painter.circle_filled(dot_center, 7.0, palette.canvas);
    painter.circle_filled(dot_center, 5.0, with_alpha(dot_color, dot_alpha));
}

/// A small filled pill with muted-tint background -- the live-status
/// convention (a short label in a colored chip) this redesign pass adopted
/// in place of the original pulse-ring animation. `text` should be
/// lowercase, matching the rest of this screen's quiet, unshouty labels.
fn state_badge(ui: &mut Ui, text: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(with_alpha(color, 35))
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(7.0, 3.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .font(egui::FontId::new(10.5, egui::FontFamily::Monospace))
                    .color(color),
            );
        });
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// First meaningful character of a display name/address, uppercased --
/// `call_avatar`'s center glyph. Falls back to a phone glyph-friendly `#`
/// on the (practically unreachable) empty-string case.
fn avatar_initial(display_name: &str) -> char {
    display_name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('#')
}

/// The caller's name in Inter, or a bare address in JetBrains Mono when no
/// contact resolved it -- the one typographic rule (numbers/addresses are
/// mono, names are Inter) applied to the in-call screen's hero label.
fn caller_name_label(ui: &mut Ui, palette: &Palette, name: &str, is_name: bool) {
    let font = if is_name {
        theme::font_heading(19.0)
    } else {
        egui::FontId::new(16.0, egui::FontFamily::Monospace)
    };
    ui.label(RichText::new(name).font(font).color(palette.ink));
}

/// A large circular icon-only button for the focused-call screen's primary
/// actions (Accept/Reject/Hang Up) -- `phone_keypad`'s digit buttons use
/// the same rounded-square-as-circle trick at a smaller size.
fn circular_action_button(ui: &mut Ui, icon: &str, color: egui::Color32) -> bool {
    let button = egui::Button::new(RichText::new(icon).size(22.0).color(egui::Color32::WHITE))
        .fill(color)
        .rounding(egui::Rounding::same(32.0));
    ui.add_sized([64.0, 64.0], button).clicked()
}

/// Convert `frame` (if it differs from `cache.frame`, the last one already
/// uploaded) to RGB and create/update `cache.texture` -- a no-op if `frame`
/// is `None` or unchanged, so an unchanged decoded/captured frame isn't
/// reconverted/re-uploaded every repaint.
fn update_video_view(
    ctx: &egui::Context,
    cache: &mut VideoViewCache,
    frame: Option<Yuv420Frame>,
    texture_name: &str,
) {
    let Some(frame) = frame else { return };
    if cache.frame.as_ref() == Some(&frame) {
        return;
    }
    let rgb = frame.to_rgb8();
    let size = [frame.width as usize, frame.height as usize];
    let image = egui::ColorImage::from_rgb(size, &rgb);
    match &mut cache.texture {
        Some(tex) => tex.set(image, egui::TextureOptions::default()),
        None => cache.texture = Some(ctx.load_texture(texture_name, image, egui::TextureOptions::default())),
    }
    cache.frame = Some(frame);
}

/// Render one side of the video panel: the cached texture if one exists
/// yet, else a muted placeholder ("No video yet" for the self-view, which
/// never gets one without a camera; "Waiting for video…" for the remote
/// side, which should fill in shortly after the call connects).
fn show_video_view(ui: &mut Ui, palette: &Palette, cache: &VideoViewCache, label: &str) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).color(palette.ink_muted).small());
        match &cache.texture {
            Some(tex) => {
                ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(160.0, 120.0)));
            }
            None => {
                let text = if label == "You" { "No video yet" } else { "Waiting for video…" };
                empty_state(ui, palette, text);
            }
        }
    });
}

/// Render one leg's RTP stats as a small label grid inside a "Call
/// statistics" collapsing section.
fn show_leg_stats(
    ui: &mut Ui,
    label: &str,
    codec: AudioCodec,
    stats: &deelip_media::LegStats,
    muted: egui::Color32,
) {
    let codec_name = audio_codec_label(codec);
    ui.label(RichText::new(format!("{label} — {codec_name}")).strong());
    ui.label(
        RichText::new(format!(
            "Sent: {} pkts / {}    Received: {} pkts / {}",
            stats.packets_sent,
            format_bytes(stats.bytes_sent),
            stats.packets_received,
            format_bytes(stats.bytes_received),
        ))
        .color(muted)
        .small(),
    );
    let loss_pct = if stats.packets_received + stats.packets_lost > 0 {
        100.0 * stats.packets_lost as f64 / (stats.packets_received + stats.packets_lost) as f64
    } else {
        0.0
    };
    ui.label(
        RichText::new(format!(
            "Loss: {} ({:.1}%)    Jitter: {:.1} ms",
            stats.packets_lost, loss_pct, stats.jitter_ms,
        ))
        .color(muted)
        .small(),
    );
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}
