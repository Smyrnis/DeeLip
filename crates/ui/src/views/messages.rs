use deelip_config::{Message, MessageDirection};
use egui::{RichText, Ui};

use crate::helpers::{empty_state, format_timestamp, normalize_target, short_uri, unix_now};
use crate::app::DeelipApp;

impl DeelipApp {
    pub(crate) fn show_messages(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);

        if self.messages.messages.is_empty() {
            empty_state(ui, &self.palette, "No messages yet.");
            ui.add_space(8.0);
        } else {
            // Distinct peers, most-recently-active first -- `messages` is
            // already newest-first (see `MessageLog::push`), so the first
            // occurrence of each `peer_uri` while walking it in order is
            // exactly that peer's most recent activity.
            let mut peers: Vec<String> = Vec::new();
            for m in &self.messages.messages {
                if !peers.contains(&m.peer_uri) {
                    peers.push(m.peer_uri.clone());
                }
            }

            let selected_valid = self
                .selected_message_peer
                .as_ref()
                .is_some_and(|p| peers.contains(p));
            if !selected_valid {
                self.selected_message_peer = peers.first().cloned();
                if let Some(peer) = &self.selected_message_peer {
                    self.message_to = short_uri(peer);
                }
            }

            ui.horizontal(|ui| {
                ui.label("Conversation:");
                let selected_label = self
                    .selected_message_peer
                    .as_deref()
                    .map(short_uri)
                    .unwrap_or_else(|| "—".to_string());
                egui::ComboBox::from_id_source("messages_peer_picker")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for peer in &peers {
                            let is_sel = self.selected_message_peer.as_deref() == Some(peer);
                            if ui
                                .selectable_label(is_sel, short_uri(peer))
                                .clicked()
                            {
                                self.selected_message_peer = Some(peer.clone());
                                self.message_to = short_uri(peer);
                            }
                        }
                    });
            });
            ui.add_space(6.0);

            // Chronological order for the bubble thread -- the underlying
            // store is newest-first, so reverse just the selected peer's
            // slice rather than the whole log.
            let thread: Vec<&Message> = match &self.selected_message_peer {
                Some(peer) => self
                    .messages
                    .messages
                    .iter()
                    .filter(|m| &m.peer_uri == peer)
                    .rev()
                    .collect(),
                None => Vec::new(),
            };

            let palette = self.palette;
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for m in thread {
                        let outbound = m.direction == MessageDirection::Outbound;
                        let fill = if outbound {
                            palette.signal.gamma_multiply(0.28)
                        } else {
                            palette.surface
                        };
                        ui.with_layout(
                            egui::Layout::top_down(if outbound {
                                egui::Align::Max
                            } else {
                                egui::Align::Min
                            }),
                            |ui| {
                                egui::Frame::none()
                                    .fill(fill)
                                    .stroke(egui::Stroke::new(1.0, palette.border))
                                    .rounding(egui::Rounding::same(2.0))
                                    .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                                    .show(ui, |ui| {
                                        ui.set_max_width(ui.available_width() * 0.7);
                                        ui.label(RichText::new(&m.body));
                                        ui.label(
                                            RichText::new(format_timestamp(m.timestamp))
                                                .font(egui::FontId::new(
                                                    10.5,
                                                    egui::FontFamily::Monospace,
                                                ))
                                                .color(palette.ink_muted),
                                        );
                                    });
                            },
                        );
                        ui.add_space(4.0);
                    }
                });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(RichText::new("Send message").font(crate::theme::font_heading(14.0)));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("To:");
            ui.add(
                egui::TextEdit::singleline(&mut self.message_to)
                    .hint_text("sip:bob@example.com")
                    .font(egui::FontId::new(13.0, egui::FontFamily::Monospace))
                    .desired_width(f32::INFINITY),
            );
        });
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.message_body)
                .desired_rows(3)
                .hint_text("Message text")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        let can_send = !self.message_to.trim().is_empty()
            && !self.message_body.trim().is_empty()
            && self.reg_ok
            && self.selected_account_idx().is_some();
        if ui
            .add_enabled(can_send, egui::Button::new("Send"))
            .clicked()
        {
            self.do_send_message();
        }
    }

    fn do_send_message(&mut self) {
        let Some(acc) = self.selected_account_idx() else {
            return;
        };
        let domain = self.dial_domain(acc);
        let to = normalize_target(self.message_to.trim(), &domain);
        let body = self.message_body.trim().to_string();
        self.accounts[acc].handle.send_message(&to, &body);

        self.messages.push(Message {
            peer_uri: to.clone(),
            direction: MessageDirection::Outbound,
            body,
            timestamp: unix_now(),
        });
        let _ = self.messages.save(&self.db);
        self.message_body.clear();
        self.selected_message_peer = Some(to);
    }
}
