use deelip_config::CallDirection;
use deelip_sip::AudioCodec;
use egui::{FontId, RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{ctx_key_enter, phone_keypad, short_uri};

impl DeelipApp {
    pub(crate) fn show_dialer(&mut self, ui: &mut Ui) {
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
                            if self.in_conference {
                                // No hold/swap semantics inside a conference --
                                // only dropping a party (Hang Up above), which
                                // falls back to an ordinary single-leg call.
                            } else if focused {
                                let hold = format!("{}  Hold", egui_phosphor::regular::PHONE_PAUSE);
                                if ui.button(hold).clicked() { hold_idx = Some(idx); }
                            } else {
                                let resume = format!("{}  Resume", egui_phosphor::regular::PLAY);
                                if ui.button(resume).clicked() { swap_idx = Some(idx); }
                            }
                            ui.add_space(4.0);
                            let state = if self.in_conference { "In conference" } else if focused { "Active" } else { "On hold" };
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

            // Orthogonal to attended transfer -- applies equally to two
            // calls that arrived via ordinary call-waiting.
            if self.calls.len() == 2 && !self.in_conference {
                ui.add_space(4.0);
                let merge = format!("{}  Merge into Conference", egui_phosphor::regular::PHONE);
                if ui.button(merge).clicked() {
                    self.start_conference();
                }
            }
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
                    ui.add_space(4.0);
                    let can_attend = self.calls.len() == 1;
                    let attended_label = format!("{}  {}", egui_phosphor::regular::ARROW_BEND_UP_RIGHT, if self.showing_attended { "Cancel attended" } else { "Attended" });
                    if ui.add_enabled(can_attend, egui::Button::new(attended_label)).clicked() {
                        self.showing_attended = !self.showing_attended;
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
                if self.showing_attended {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.attended_target)
                            .hint_text("sip:carol@example.com")
                            .desired_width(f32::INFINITY));
                        if ui.button("Call").clicked() {
                            self.do_attended_transfer_dial();
                        }
                    });
                }
                if self.attended_transfer_original.is_some() && self.calls.len() == 2 {
                    ui.add_space(4.0);
                    let complete = format!("{}  Complete Transfer", egui_phosphor::regular::CHECK_CIRCLE);
                    if ui.button(complete).clicked() {
                        self.do_complete_attended_transfer();
                    }
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

            if let Some(engine) = self.media.as_ref() {
                ui.add_space(8.0);
                let stats = engine.stats();
                let muted_color = self.palette.muted;
                egui::CollapsingHeader::new("Call statistics").show(ui, |ui| {
                    if self.in_conference && self.calls.len() == 2 {
                        show_leg_stats(ui, &short_uri(&self.calls[0].remote_uri), self.calls[0].codec, &stats.leg1, muted_color);
                        if let Some(leg2) = stats.leg2.as_ref() {
                            ui.add_space(4.0);
                            show_leg_stats(ui, &short_uri(&self.calls[1].remote_uri), self.calls[1].codec, leg2, muted_color);
                        }
                    } else if let Some(idx) = self.focused_call {
                        show_leg_stats(ui, "This call", self.calls[idx].codec, &stats.leg1, muted_color);
                    }
                });
            }
        }
    }
}

/// Render one leg's RTP stats as a small label grid inside a "Call
/// statistics" collapsing section.
fn show_leg_stats(ui: &mut Ui, label: &str, codec: AudioCodec, stats: &deelip_media::LegStats, muted: egui::Color32) {
    let codec_name = match codec {
        AudioCodec::Opus => "Opus",
        AudioCodec::G722 => "G.722",
        AudioCodec::Pcmu => "PCMU",
        AudioCodec::Pcma => "PCMA",
        AudioCodec::Gsm  => "GSM",
        AudioCodec::Ilbc => "iLBC",
    };
    ui.label(RichText::new(format!("{label} — {codec_name}")).strong());
    ui.label(RichText::new(format!(
        "Sent: {} pkts / {}    Received: {} pkts / {}",
        stats.packets_sent, format_bytes(stats.bytes_sent),
        stats.packets_received, format_bytes(stats.bytes_received),
    )).color(muted).small());
    let loss_pct = if stats.packets_received + stats.packets_lost > 0 {
        100.0 * stats.packets_lost as f64 / (stats.packets_received + stats.packets_lost) as f64
    } else {
        0.0
    };
    ui.label(RichText::new(format!(
        "Loss: {} ({:.1}%)    Jitter: {:.1} ms",
        stats.packets_lost, loss_pct, stats.jitter_ms,
    )).color(muted).small());
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} B") } else { format!("{:.1} KB", bytes as f64 / 1024.0) }
}
