use deelip_config::CallDirection;
use deelip_media::video_codec::Yuv420Frame;
use deelip_sip::AudioCodec;
use egui::{Align2, Color32, RichText, Ui};

use crate::app::{DeelipApp, VideoViewCache};
use crate::helpers::{
    audio_codec_label, empty_state, format_call_timer, resolve_caller, short_uri, styled_slider, unix_now,
};
use crate::strings::{t, tf};
use crate::theme::{self, Palette};

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
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            call_avatar(ui, &self.palette, &name, RingState::Pending);
            ui.add_space(8.0);
            caller_name_label(ui, &self.palette, &name, is_name);
            ui.add_space(4.0);
            state_badge(ui, &t("dialer.status_ringing"), self.palette.ringing);
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
    fn caller_display(&self, uri: &str) -> (String, bool) {
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
                    let row_width = if self.in_conference { 64.0 } else { 64.0 + 10.0 + 56.0 };
                    ui.horizontal(|ui| {
                        ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
                        if circular_action_button(ui, egui_phosphor::regular::PHONE_X, self.palette.danger) {
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
                            .rounding(egui::Rounding::same(14.0));
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
        let local_frame = self.video.as_ref().and_then(|v| v.camera.as_ref()).and_then(|c| c.latest_frame());

        let ctx = ui.ctx().clone();
        if let Some(v) = self.video.as_mut() {
            update_video_view(&ctx, &mut v.remote, remote_frame, "deelip_remote_video");
            update_video_view(&ctx, &mut v.local, local_frame, "deelip_local_video");
        }

        let palette = self.palette;
        if let Some(v) = self.video.as_ref() {
            ui.horizontal(|ui| {
                show_video_view(ui, &palette, &v.remote, &t("dialer.video_remote_label"), false);
                ui.add_space(8.0);
                show_video_view(ui, &palette, &v.local, &t("dialer.video_you_label"), true);
            });
        }
        ui.add_space(4.0);
    }

    fn show_focused_call_controls(&mut self, ui: &mut Ui) {
        theme::full_width_card(ui, self.palette, |ui| {
            let palette = self.palette;
            let row_width = 4.0 * ICON_TOGGLE_COL_WIDTH + 3.0 * ui.spacing().item_spacing.x;
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

/// Which state `call_avatar`/`state_badge` reflect -- design history (this
/// replaced an earlier animated dual-ring pulse): `docs/crates/ui.md`'s "status-dot
/// redesign" note.
#[derive(Clone, Copy, PartialEq)]
enum RingState {
    Pending,
    Connected,
}

/// A caller initial on a small surface circle, with a state-colored status
/// dot at its corner.
fn call_avatar(ui: &mut Ui, palette: &Palette, display_name: &str, state: RingState) {
    let avatar_d = 68.0;
    let pad = 8.0; // room for the status dot to sit outside the avatar's own edge
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(avatar_d + pad, avatar_d + pad), egui::Sense::hover());
    let center = rect.center() - egui::vec2(pad / 2.0, pad / 2.0);
    let painter = ui.painter();
    let avatar_r = avatar_d / 2.0;

    painter.circle_filled(center, avatar_r, palette.surface);
    painter.circle_stroke(center, avatar_r, egui::Stroke::new(1.0, palette.border));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        avatar_initial(display_name).to_string(),
        theme::font_heading(22.0),
        palette.ink,
    );

    let dot_color = match state {
        RingState::Pending => palette.ringing,
        RingState::Connected => palette.signal,
    };
    let dot_alpha = match state {
        RingState::Pending => {
            // A slow opacity fade, not a bounce. No extra `request_repaint()`
            // -- `frame.rs`'s own 50ms cadence already redraws this often
            // enough to read as smooth.
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
            ui.label(RichText::new(text).font(egui::FontId::new(10.5, egui::FontFamily::Monospace)).color(color));
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
    display_name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).unwrap_or('#')
}

/// The caller's name in Inter, or a bare address in JetBrains Mono when no
/// contact resolved it -- the one typographic rule (numbers/addresses are
/// mono, names are Inter) applied to the in-call screen's hero label.
fn caller_name_label(ui: &mut Ui, palette: &Palette, name: &str, is_name: bool) {
    let font = if is_name { theme::font_heading(24.0) } else { egui::FontId::new(20.0, egui::FontFamily::Monospace) };
    ui.label(RichText::new(name).font(font).color(palette.ink));
}

/// A large rounded-square icon-only button for the focused-call screen's
/// primary actions (Accept/Reject/Hang Up) -- same rounded-square language
/// as `phone_keypad`'s digit buttons, not a full circle.
fn circular_action_button(ui: &mut Ui, icon: &str, color: egui::Color32) -> bool {
    let button = egui::Button::new(RichText::new(icon).size(22.0).color(egui::Color32::WHITE))
        .fill(color)
        .rounding(egui::Rounding::same(14.0));
    ui.add_sized([64.0, 64.0], button).clicked()
}

/// Column width reserved per button in the Mute/Record/Xfer/Keypad row --
/// wider than the 48px button itself so "Record"/"Keypad" have room not to
/// wrap (see `icon_toggle_button`'s doc comment for why a column that wraps
/// its caption while its neighbors don't caused a real bug). Also used by
/// `show_focused_call_controls` to compute that row's own centering width.
const ICON_TOGGLE_COL_WIDTH: f32 = 60.0;

/// A smaller icon-only rounded-square button with a small caption
/// underneath -- the secondary in-call actions (Mute, Record, Transfer,
/// Keypad), same icon+caption idiom `phone_keypad` already uses for its
/// digit+letters. `active` (the surface_hover fill, matching this theme's
/// existing "toggled on" convention e.g. the tab bar's selected state)
/// reflects the button's own on/off state (muted, currently recording,
/// panel open); `danger` additionally recolors the icon+caption to
/// `palette.danger` for a state that's not just "on" but actively
/// consequential (recording right now).
///
/// Deliberately built from raw `ui.painter()` calls on one
/// `ui.allocate_exact_size` rect, not `egui::Button` + a layout container --
/// two layout-based approaches were tried first and both had a real,
/// live-desktop-only box-position bug. Full writeup: `docs/crates/ui.md`.
fn icon_toggle_button(ui: &mut Ui, palette: &Palette, icon: &str, caption: &str, active: bool, danger: bool) -> bool {
    const BTN: f32 = 48.0;
    let icon_color = if danger { palette.danger } else { palette.ink };
    let fill = if active { palette.surface_hover } else { palette.surface };

    let (col_rect, response) = ui.allocate_exact_size(egui::vec2(ICON_TOGGLE_COL_WIDTH, 64.0), egui::Sense::click());
    let btn_rect =
        egui::Rect::from_min_size(egui::pos2(col_rect.center().x - BTN / 2.0, col_rect.min.y), egui::vec2(BTN, BTN));

    let painter = ui.painter();
    painter.rect(btn_rect, egui::Rounding::same(12.0), fill, egui::Stroke::new(1.0, palette.border));
    // Per-glyph vertical nudge -- the Phosphor `MICROPHONE`/
    // `MICROPHONE_SLASH` glyph's ink sits visibly higher within its own
    // font-metrics line box than `RECORD`/`EXPORT`/`NUMPAD` do (confirmed
    // via a zoomed side-by-side screenshot), unrelated to the box-position
    // bug above -- this only recenters that one glyph's ink within an
    // already-correctly-positioned button.
    let nudge_y = if icon == egui_phosphor::regular::MICROPHONE || icon == egui_phosphor::regular::MICROPHONE_SLASH {
        3.0
    } else {
        0.0
    };
    painter.text(
        btn_rect.center() + egui::vec2(0.0, nudge_y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(18.0),
        icon_color,
    );
    painter.text(
        egui::pos2(col_rect.center().x, btn_rect.max.y + 2.0),
        egui::Align2::CENTER_TOP,
        caption,
        egui::FontId::new(11.0, egui::FontFamily::Proportional), // matches `RichText::small()`
        icon_color,
    );
    response.clicked()
}

/// Convert `frame` (if it differs from `cache.frame`, the last one already
/// uploaded) to RGB and create/update `cache.texture` -- a no-op if `frame`
/// is `None` or unchanged, so an unchanged decoded/captured frame isn't
/// reconverted/re-uploaded every repaint.
fn update_video_view(ctx: &egui::Context, cache: &mut VideoViewCache, frame: Option<Yuv420Frame>, texture_name: &str) {
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
/// side, which should fill in shortly after the call connects). `is_local`
/// picks which placeholder applies -- previously inferred by comparing
/// `label == "You"`, which broke once `label` became a localized string
/// that isn't literally "You" in every language.
fn show_video_view(ui: &mut Ui, palette: &Palette, cache: &VideoViewCache, label: &str, is_local: bool) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).color(palette.ink_muted).small());
        match &cache.texture {
            Some(tex) => {
                ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(160.0, 120.0)));
            }
            None => {
                let text = if is_local { t("dialer.no_video_yet") } else { t("dialer.waiting_for_video") };
                empty_state(ui, palette, &text);
            }
        }
    });
}

/// Render one leg's RTP stats as a small label grid inside a "Call
/// statistics" collapsing section.
fn show_leg_stats(ui: &mut Ui, label: &str, codec: AudioCodec, stats: &deelip_media::LegStats, muted: egui::Color32) {
    let codec_name = audio_codec_label(codec);
    ui.label(RichText::new(format!("{label} — {codec_name}")).strong());
    ui.label(
        RichText::new(tf(
            "dialer.stats_sent_received",
            &[
                ("sent_pkts", &stats.packets_sent.to_string()),
                ("sent_bytes", &format_bytes(stats.bytes_sent)),
                ("recv_pkts", &stats.packets_received.to_string()),
                ("recv_bytes", &format_bytes(stats.bytes_received)),
            ],
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
        RichText::new(tf(
            "dialer.stats_loss_jitter",
            &[
                ("lost", &stats.packets_lost.to_string()),
                ("pct", &format!("{loss_pct:.1}")),
                ("jitter", &format!("{:.1}", stats.jitter_ms)),
            ],
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

#[cfg(test)]
#[path = "../../../tests/unit/in_call.rs"]
mod tests;
