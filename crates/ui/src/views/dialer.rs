use deelip_config::CallDirection;
use deelip_sip::AudioCodec;
use egui::{FontId, RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{
    account_status_label, audio_codec_label, ctx_key_enter, phone_keypad, short_uri,
};
use crate::theme;

impl DeelipApp {
    pub(crate) fn show_dialer(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        let idle =
            self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
        if idle {
            self.show_dialer_idle(ui);
        } else {
            self.show_dialer_in_call(ui);
        }
    }

    // ── Idle: number entry + keypad, MicroSIP's Phone-tab shape ──────────────

    fn show_dialer_idle(&mut self, ui: &mut Ui) {
        if self.accounts.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Call from:");
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
            ui.add_space(8.0);
        }

        theme::full_width_card(ui, self.palette, |ui| {
            ui.label(
                RichText::new("SIP address / number")
                    .color(self.palette.muted)
                    .small(),
            );
            ui.add_space(4.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.call_target)
                    .hint_text("sip:bob@example.com")
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(18.0)),
            );
            if resp.lost_focus() && ctx_key_enter(ui) {
                self.do_call(None);
            }
        });
        ui.add_space(8.0);

        let palette = self.palette;
        theme::full_width_card(ui, palette, |ui| {
            phone_keypad(ui, palette, |digit| self.call_target.push(digit));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(egui_phosphor::regular::BACKSPACE).clicked() {
                    self.call_target.pop();
                }
                if ui.button("Clear").clicked() {
                    self.call_target.clear();
                }
            });
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let call_text = RichText::new(format!("{}  Call", egui_phosphor::regular::PHONE))
                .size(16.0)
                .color(self.palette.accent);
            if ui
                .add_sized(
                    [ui.available_width() * 0.7, 40.0],
                    egui::Button::new(call_text),
                )
                .clicked()
            {
                self.do_call(None);
            }
            let can_redial = self.reg_ok && self.last_dialed.is_some();
            let redial_text = RichText::new(format!(
                "{}  Redial",
                egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE
            ))
            .size(14.0);
            if ui
                .add_enabled(can_redial, egui::Button::new(redial_text))
                .clicked()
            {
                self.do_redial();
            }
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
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(egui_phosphor::regular::PHONE_INCOMING)
                    .size(40.0)
                    .color(self.palette.info),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(short_uri(from)).font(FontId::proportional(22.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("Incoming call").color(self.palette.muted));
        });
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            let spacing = ui.available_width() * 0.12;
            ui.add_space(spacing);
            if circular_action_button(ui, egui_phosphor::regular::PHONE, self.palette.accent) {
                self.do_accept();
            }
            ui.add_space(ui.available_width() - spacing - 64.0);
            if circular_action_button(ui, egui_phosphor::regular::PHONE_X, self.palette.danger) {
                self.do_reject();
            }
        });
    }

    fn show_call_waiting_banner(&mut self, ui: &mut Ui, from: &str) {
        theme::full_width_card(ui, self.palette, |ui| {
            ui.label(
                RichText::new(format!("Call waiting: {}", short_uri(from)))
                    .color(self.palette.warn)
                    .font(FontId::proportional(15.0)),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let accept = format!("{}  Accept", egui_phosphor::regular::PHONE);
                if ui
                    .button(RichText::new(accept).color(self.palette.accent))
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
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(egui_phosphor::regular::PHONE_OUTGOING)
                    .size(40.0)
                    .color(self.palette.accent),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(short_uri(target)).font(FontId::proportional(22.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("Calling…").color(self.palette.muted));
        });
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
                    .color(self.palette.muted),
            );
            ui.add_space(6.0);
        }

        for idx in 0..self.calls.len() {
            let focused = self.focused_call == Some(idx);
            let (icon, color, uri) = {
                let call = &self.calls[idx];
                let (icon, color) = match call.direction {
                    CallDirection::Inbound => {
                        (egui_phosphor::regular::PHONE_INCOMING, self.palette.info)
                    }
                    CallDirection::Outbound => {
                        (egui_phosphor::regular::PHONE_OUTGOING, self.palette.accent)
                    }
                };
                (icon, color, call.remote_uri.clone())
            };

            theme::full_width_card(ui, self.palette, |ui| {
                if focused {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(icon).size(28.0).color(color));
                        ui.add_space(4.0);
                        ui.label(RichText::new(short_uri(&uri)).font(FontId::proportional(20.0)));
                        let state = if self.in_conference {
                            "In conference"
                        } else {
                            "Active"
                        };
                        ui.label(RichText::new(state).color(self.palette.muted));
                        if self.config.recording_enabled {
                            ui.label(RichText::new("● REC").color(self.palette.danger));
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if circular_action_button(
                                ui,
                                egui_phosphor::regular::PHONE_X,
                                self.palette.danger,
                            ) {
                                hangup_idx = Some(idx);
                            }
                            if !self.in_conference
                                && circular_action_button(
                                    ui,
                                    egui_phosphor::regular::PHONE_PAUSE,
                                    self.palette.muted,
                                )
                            {
                                hold_idx = Some(idx);
                            }
                        });
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon).color(color));
                        ui.label(short_uri(&uri));
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
                            ui.label(RichText::new("On hold").color(self.palette.muted));
                        });
                    });
                }
            });
            ui.add_space(6.0);
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
                let transfer_label = format!(
                    "{}  {}",
                    egui_phosphor::regular::ARROW_BEND_UP_RIGHT,
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
                    egui_phosphor::regular::ARROW_BEND_UP_RIGHT,
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
            if self.showing_transfer {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.transfer_target)
                            .hint_text("sip:carol@example.com")
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
            let muted_color = self.palette.muted;
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

/// A large circular icon-only button for the focused-call screen's primary
/// actions (Accept/Reject/Hang Up/Hold) -- `phone_keypad`'s digit buttons
/// use the same rounded-square-as-circle trick at a smaller size.
fn circular_action_button(ui: &mut Ui, icon: &str, color: egui::Color32) -> bool {
    let button = egui::Button::new(RichText::new(icon).size(22.0).color(egui::Color32::WHITE))
        .fill(color)
        .rounding(egui::Rounding::same(32.0));
    ui.add_sized([64.0, 64.0], button).clicked()
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
