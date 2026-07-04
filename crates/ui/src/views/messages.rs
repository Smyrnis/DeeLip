use deelip_config::{Message, MessageDirection};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{normalize_target, short_uri, unix_now};

impl DeelipApp {
    pub(crate) fn show_messages(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);

        if self.messages.messages.is_empty() {
            ui.label(RichText::new("No messages yet.").color(self.palette.muted));
        } else {
            // Same `show_rows` virtualization + single-widget-per-row divider
            // approach as History (`views/history.rs`) -- same shape of
            // problem, a capped append-mostly list.
            let row_height = ui.spacing().interact_size.y.max(ui.text_style_height(&egui::TextStyle::Body))
                + ui.spacing().item_spacing.y;
            let count = self.messages.messages.len();
            let mut reply_target: Option<String> = None;
            egui::ScrollArea::vertical().max_height(280.0).show_rows(ui, row_height, count, |ui, row_range| {
                for idx in row_range {
                    let m = &self.messages.messages[idx];
                    let (icon, color) = match m.direction {
                        MessageDirection::Inbound  => (egui_phosphor::regular::ARROW_DOWN_LEFT, self.palette.info),
                        MessageDirection::Outbound => (egui_phosphor::regular::ARROW_UP_RIGHT, self.palette.accent),
                    };
                    let peer_uri = m.peer_uri.clone();
                    let row = ui.horizontal(|ui| {
                        ui.label(RichText::new(icon).color(color));
                        ui.label(short_uri(&m.peer_uri));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if m.direction == MessageDirection::Inbound && ui.small_button("Reply").clicked() {
                                reply_target = Some(peer_uri.clone());
                            }
                            ui.label(RichText::new(&m.body).color(self.palette.muted));
                        });
                    }).response;
                    ui.painter().hline(row.rect.x_range(), row.rect.bottom(), ui.visuals().widgets.noninteractive.bg_stroke);
                }
            });
            if let Some(peer) = reply_target {
                self.message_to = short_uri(&peer);
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new("Send message").strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("To:");
            ui.add(egui::TextEdit::singleline(&mut self.message_to)
                .hint_text("sip:bob@example.com")
                .desired_width(f32::INFINITY));
        });
        ui.add_space(4.0);
        ui.add(egui::TextEdit::multiline(&mut self.message_body)
            .desired_rows(3)
            .hint_text("Message text")
            .desired_width(f32::INFINITY));
        ui.add_space(4.0);
        let can_send = !self.message_to.trim().is_empty()
            && !self.message_body.trim().is_empty()
            && self.reg_ok
            && self.selected_account_idx().is_some();
        if ui.add_enabled(can_send, egui::Button::new("Send")).clicked() {
            self.do_send_message();
        }
    }

    fn do_send_message(&mut self) {
        let Some(acc) = self.selected_account_idx() else { return };
        let domain = self.accounts[acc].handle.domain.clone();
        let to = normalize_target(self.message_to.trim(), &domain);
        let body = self.message_body.trim().to_string();
        self.accounts[acc].handle.send_message(&to, &body);

        self.messages.push(Message {
            peer_uri: to,
            direction: MessageDirection::Outbound,
            body,
            timestamp: unix_now(),
        });
        let _ = self.messages.save(&self.db);
        self.message_body.clear();
    }
}
