//! The focused in-call screen -- replaces the keypad entirely while
//! ringing/dialing/connected, instead of stacking status boxes above it.
//! Split from a single `in_call.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior change: the screen-orchestration `impl DeelipApp` methods stay
//! here, the stateless drawing primitives moved to `widgets.rs`, the video
//! texture cache to `video.rs`, and the stats-panel formatter to `stats.rs`.

mod stats;
mod video;
mod widgets;

use deelip_config::CallDirection;
use egui::{RichText, Ui};

use stats::show_leg_stats;
use video::{show_video_view, update_video_view};
use widgets::{
    call_avatar, caller_name_label, circular_action_button, icon_toggle_button, state_badge, RingState,
    CIRCULAR_ACTION_COL_WIDTH,
};

use crate::app::DeelipApp;
use crate::helpers::{format_call_timer, resolve_caller, short_uri, styled_slider, unix_now};
use crate::strings::{t, tf};
use crate::theme;

impl DeelipApp {
    // ── In-call: focused call screen -- replaces the keypad entirely while
    // ringing/dialing/connected, instead of stacking status boxes above it ──

    pub(super) fn show_dialer_in_call(&mut self, ui: &mut Ui) {
        // Wrapped in a `ScrollArea` since this content is open-ended (any
        // combination of sub-panels can be showing at once) -- a no-op for
        // the common case, only kicks in once something doesn't fit.
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.show_dialer_in_call_content(ui));
    }

    fn show_dialer_in_call_content(&mut self, ui: &mut Ui) {
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
        // More breathing room than the connected screen's own leading space
        // -- this screen has far less content (no timer/controls card yet),
        // so a bit more top space keeps it from reading as pinned to the
        // very top of a mostly-empty canvas.
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            call_avatar(ui, &self.palette, &name, RingState::Pending);
            ui.add_space(8.0);
            caller_name_label(ui, &self.palette, &name, is_name);
            ui.add_space(4.0);
            state_badge(ui, &t("dialer.status_ringing"), self.palette.ringing);
        });
        ui.add_space(28.0);
        let row_width = 2.0 * CIRCULAR_ACTION_COL_WIDTH + ui.spacing().item_spacing.x;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
            if circular_action_button(ui, egui_phosphor::regular::PHONE, self.palette.signal, &t("common.accept_button"))
            {
                self.do_accept();
            }
            if circular_action_button(
                ui,
                egui_phosphor::regular::PHONE_X,
                self.palette.danger,
                &t("common.reject_button"),
            ) {
                self.do_reject();
            }
        });
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if icon_toggle_button(
                ui,
                &self.palette,
                egui_phosphor::regular::EXPORT,
                &t("dialer.redirect_caption"),
                self.showing_redirect,
                false,
            ) {
                self.showing_redirect = !self.showing_redirect;
            }
        });
    }

    fn show_call_waiting_banner(&mut self, ui: &mut Ui, from: &str) {
        let (name, _) = self.caller_display(from);
        theme::full_width_card(ui, self.palette, |ui| {
            ui.label(
                RichText::new(tf("dialer.call_waiting", &[("name", &name)]))
                    .color(self.palette.ringing)
                    .font(theme::font_medium(14.0)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let accept = format!("{}  {}", egui_phosphor::regular::PHONE, t("common.accept_button"));
                if ui.button(RichText::new(accept).color(self.palette.signal)).clicked() {
                    self.do_accept();
                }
                let reject = format!("{}  {}", egui_phosphor::regular::PHONE_X, t("common.reject_button"));
                if ui.button(RichText::new(reject).color(self.palette.danger)).clicked() {
                    self.do_reject();
                }
                let redirect = format!("{}  {}", egui_phosphor::regular::EXPORT, t("dialer.redirect_caption"));
                if ui.button(redirect).clicked() {
                    self.showing_redirect = !self.showing_redirect;
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
            state_badge(ui, &t("dialer.status_calling"), self.palette.ringing);
        });
    }

    /// Resolve a raw SIP URI to a contact's name when one exists, matching
    /// History/Contacts' own `display_name` convention -- returns whether a
    /// real *name* was found, so the caller can render a resolved name in
    /// Inter and a bare address in JetBrains Mono (the redesign's one
    /// typographic rule: numbers/addresses are mono, names are Inter).
    pub(crate) fn caller_display(&self, uri: &str) -> (String, bool) {
        resolve_caller(&self.contacts, uri)
    }

    fn show_active_calls(&mut self, ui: &mut Ui) {
        let mut hangup_idx: Option<usize> = None;
        let mut hold_idx: Option<usize> = None;
        let mut swap_idx: Option<usize> = None;

        // If an attended-transfer consultation call is currently ringing,
        // its `pending_outbound` coexists with the held original call --
        // surface it as a small line rather than silently showing nothing.
        if let Some(out) = &self.pending_outbound {
            let (name, _) = self.caller_display(&out.remote_uri);
            ui.label(RichText::new(tf("dialer.calling_name", &[("name", &name)])).color(self.palette.ink_muted));
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
                    let state = if self.in_conference {
                        t("dialer.status_in_conference")
                    } else {
                        t("dialer.status_connected")
                    };
                    state_badge(ui, &state, self.palette.signal);
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
                        ui.label(
                            RichText::new(format!("● {}", t("dialer.rec_indicator")))
                                .color(self.palette.danger)
                                .small(),
                        );
                    }
                    if let Some(sas) = self.media.as_ref().and_then(|m| m.zrtp_sas()) {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("🔒 {}", tf("dialer.zrtp_sas", &[("sas", &sas)])))
                                .font(egui::FontId::new(12.5, egui::FontFamily::Monospace))
                                .color(self.palette.signal),
                        )
                        .on_hover_text(t("dialer.zrtp_sas_hover"));
                    }
                    ui.add_space(16.0);
                    // `vertical_centered` only centers a single fixed-size
                    // child -- see docs/crates/ui.md's "centering nested rows" note.
                    let row_width =
                        if self.in_conference { CIRCULAR_ACTION_COL_WIDTH } else { CIRCULAR_ACTION_COL_WIDTH + 10.0 + 56.0 };
                    ui.horizontal(|ui| {
                        ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
                        if circular_action_button(
                            ui,
                            egui_phosphor::regular::PHONE_X,
                            self.palette.danger,
                            &t("dialer.hangup_button"),
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
                            .rounding(egui::Rounding::same(14));
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
                            ui.label(RichText::new(&name).font(theme::font_address()));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hang_up = format!("{}  {}", egui_phosphor::regular::PHONE_X, t("dialer.hangup_button"));
                            if ui.button(RichText::new(hang_up).color(self.palette.danger)).clicked() {
                                hangup_idx = Some(idx);
                            }
                            ui.add_space(4.0);
                            if !self.in_conference {
                                let resume = format!("{}  {}", egui_phosphor::regular::PLAY, t("dialer.resume_button"));
                                if ui.button(resume).clicked() {
                                    swap_idx = Some(idx);
                                }
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(t("dialer.on_hold_label"))
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
            let merge = format!("{}  {}", egui_phosphor::regular::PHONE, t("dialer.merge_conference_button"));
            if ui.button(merge).clicked() {
                self.start_conference();
            }
            ui.add_space(6.0);
        }

        if self.focused_call.is_some() {
            self.show_focused_call_controls(ui);
        }
    }

    /// Renders the focused call's video panel, if this call negotiated a
    /// video leg -- see docs/crates/ui.md's "Video panel" note for the
    /// borrow-splitting shape this needs.
    fn show_video_panel(&mut self, ui: &mut Ui) {
        if self.video.is_none() {
            return;
        }
        let remote_frame = self.video.as_ref().and_then(|v| v.engine.latest_decoded_frame());
        let remote2_frame = self.video.as_ref().and_then(|v| v.engine.latest_decoded_frame_leg2());
        let local_frame = self.video.as_ref().and_then(|v| v.camera.as_ref()).and_then(|c| c.latest_frame());

        let ctx = ui.ctx().clone();
        if let Some(v) = self.video.as_mut() {
            update_video_view(&ctx, &mut v.remote, remote_frame, "deelip_remote_video");
            update_video_view(&ctx, &mut v.remote2, remote2_frame, "deelip_remote2_video");
            update_video_view(&ctx, &mut v.local, local_frame, "deelip_local_video");
        }

        let palette = self.palette;
        // A conference bridges a second remote party's video onto the same
        // panel (`remote2` only ever has a texture once that leg exists --
        // see `media.rs::start_conference`) -- everyone else just sees the
        // ordinary 2-box remote/self layout.
        let has_second_leg = self.video.as_ref().is_some_and(|v| v.remote2.texture.is_some());
        if let Some(v) = self.video.as_ref() {
            ui.horizontal(|ui| {
                let remote_label =
                    if has_second_leg { t("dialer.video_remote1_label") } else { t("dialer.video_remote_label") };
                show_video_view(ui, &palette, &v.remote, &remote_label, false);
                ui.add_space(8.0);
                if has_second_leg {
                    show_video_view(ui, &palette, &v.remote2, &t("dialer.video_remote2_label"), false);
                    ui.add_space(8.0);
                }
                show_video_view(ui, &palette, &v.local, &t("dialer.video_you_label"), true);
            });
        }
        ui.add_space(4.0);
    }

    fn show_focused_call_controls(&mut self, ui: &mut Ui) {
        theme::full_width_card(ui, self.palette, |ui| {
            let palette = self.palette;
            let has_video = self.video.is_some();
            let button_count = if has_video { 5.0 } else { 4.0 };
            let row_width =
                button_count * widgets::ICON_TOGGLE_COL_WIDTH + (button_count - 1.0) * ui.spacing().item_spacing.x;
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
                let muted = self.is_muted();
                let mute_caption = if muted { t("common.unmute_button") } else { t("common.mute_button") };
                if icon_toggle_button(
                    ui,
                    &palette,
                    if muted { egui_phosphor::regular::MICROPHONE_SLASH } else { egui_phosphor::regular::MICROPHONE },
                    &mute_caption,
                    muted,
                    false,
                ) {
                    self.do_mute_toggle();
                }
                if has_video {
                    let video_muted = self.is_video_muted();
                    let video_caption =
                        if video_muted { t("common.camera_on_button") } else { t("common.camera_off_button") };
                    if icon_toggle_button(
                        ui,
                        &palette,
                        if video_muted {
                            egui_phosphor::regular::VIDEO_CAMERA_SLASH
                        } else {
                            egui_phosphor::regular::VIDEO_CAMERA
                        },
                        &video_caption,
                        video_muted,
                        false,
                    ) {
                        self.do_video_toggle();
                    }
                }
                let recording = self.is_recording();
                let record_caption = if recording { t("common.stop_button") } else { t("common.record_button") };
                if icon_toggle_button(
                    ui,
                    &palette,
                    egui_phosphor::regular::RECORD,
                    &record_caption,
                    recording,
                    recording,
                ) {
                    self.do_record_toggle();
                }
                let transfer_open = self.showing_transfer || self.showing_attended;
                // Short caption -- "Transfer" (the untruncated translation
                // of `dialer.xfer_caption`) wraps to 2 lines in the 48px-wide
                // slot; see `dialer.xfer_caption`'s own locale-key comment.
                let xfer_caption = t("dialer.xfer_caption");
                if icon_toggle_button(
                    ui,
                    &palette,
                    // `EXPORT`, not "↱" -- confirmed broken in this exact
                    // spot despite working elsewhere; see docs/crates/ui.md's
                    // Theming section's "verify, don't assume" rule.
                    egui_phosphor::regular::EXPORT,
                    &xfer_caption,
                    transfer_open,
                    false,
                ) {
                    self.showing_transfer = !transfer_open;
                    self.showing_attended = false;
                }
                let keypad_caption = t("dialer.keypad_window_title");
                if icon_toggle_button(
                    ui,
                    &palette,
                    egui_phosphor::regular::NUMPAD,
                    &keypad_caption,
                    self.showing_dtmf,
                    false,
                ) {
                    self.showing_dtmf = !self.showing_dtmf;
                }
            });
            ui.add_space(10.0);
            // Centered as a group -- same leading-margin technique as the
            // icon-button row above (a plain `ui.horizontal` here would
            // otherwise sit flush against the card's left edge).
            let slider_row_width = 2.0 * (20.0 + ui.spacing().slider_width) + 8.0 + 2.0 * ui.spacing().item_spacing.x;
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - slider_row_width) / 2.0).max(0.0));
                ui.label(egui_phosphor::regular::SPEAKER_HIGH);
                let mut out_gain = self.output_gain();
                if styled_slider(ui, &self.palette, egui::Slider::new(&mut out_gain, 0.0..=2.0).show_value(false))
                    .changed()
                {
                    self.set_output_gain(out_gain);
                }
                ui.add_space(8.0);
                ui.label(egui_phosphor::regular::MICROPHONE);
                let mut in_gain = self.input_gain();
                if styled_slider(ui, &self.palette, egui::Slider::new(&mut in_gain, 0.0..=2.0).show_value(false))
                    .changed()
                {
                    self.set_input_gain(in_gain);
                }
            });
            if self.attended_transfer_original.is_some() && self.calls.len() == 2 {
                ui.add_space(8.0);
                let complete =
                    format!("{}  {}", egui_phosphor::regular::CHECK_CIRCLE, t("dialer.complete_transfer_button"),);
                ui.vertical_centered(|ui| {
                    if ui.add(egui::Button::new(RichText::new(complete).color(palette.signal))).clicked() {
                        self.do_complete_attended_transfer();
                    }
                });
            }
        });

        if let Some(engine) = self.media.as_ref() {
            ui.add_space(8.0);
            let stats = engine.stats();
            let muted_color = self.palette.ink_muted;
            egui::CollapsingHeader::new(t("dialer.call_statistics_header")).show(ui, |ui| {
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
                        &t("dialer.this_call_label"),
                        self.calls[idx].media.codec,
                        &stats.leg1,
                        muted_color,
                    );
                }
            });
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/in_call.rs"]
mod tests;
