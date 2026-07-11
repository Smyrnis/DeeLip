use deelip_config::{Message, MessageDirection};
use egui::{RichText, Ui};

use crate::app::{DeelipApp, SharedApp};
use crate::helpers::{
    avatar, empty_state, format_timestamp, list_row_divider, resolve_caller, text_edit_scope,
    unix_now, window_icon,
};

impl DeelipApp {
    /// Messages as a separate native OS window, same `Deferred`-viewport
    /// pattern as `show_settings_modal` (see its own doc comment for why a
    /// real, independently-redrawing second window rather than an in-canvas
    /// one, and why that requires locking a shared `Arc<Mutex<DeelipApp>>`
    /// instead of directly capturing `&mut self`) -- except there's no
    /// tab-bar entry point at all: the only way `messages_window_open`
    /// becomes `true` is `message_from_list` (a right-click "Message"
    /// action on a History/Contacts/Directory row). No-op when closed.
    /// Called every frame, same lifecycle as Settings.
    pub(crate) fn show_messages_window(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        if !self.messages_window_open {
            return;
        }

        // See `show_settings_modal`'s doc comment for why this has to be
        // checked up front rather than branched on from inside the deferred
        // closure: on a backend that embeds, the closure runs synchronously
        // right here, and locking `self_arc` there would deadlock against
        // the lock this method's own caller already holds.
        if ctx.embed_viewports() {
            let peers = self.message_peers();
            let mut open = true;
            egui::Window::new("Messages")
                .id(egui::Id::new("messages_window_fallback"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([640.0, 520.0])
                .min_width(480.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.allocate_ui(egui::vec2(200.0, ui.available_height()), |ui| {
                            self.show_messages_peer_list(ui, &peers)
                        });
                        ui.separator();
                        ui.vertical(|ui| self.show_messages_thread_and_compose(ui));
                    });
                });
            if !open {
                self.messages_window_open = false;
            }
            return;
        }

        let viewport_id = egui::ViewportId::from_hash_of("deelip_messages_window");
        ctx.show_viewport_deferred(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title("DeeLip Messages")
                .with_inner_size([640.0, 520.0])
                .with_min_inner_size([480.0, 360.0])
                .with_icon(window_icon()),
            move |child_ctx, _class| {
                let mut app = self_app.lock();
                if !app.messages_window_open {
                    return;
                }
                // Recomputed here (not passed in from the outer call) since
                // this closure can run well after that call returns, on its
                // own independent redraw schedule -- a value captured
                // upfront would go stale.
                let peers = app.message_peers();

                egui::TopBottomPanel::top("messages_window_titlebar").show(child_ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Messages").font(crate::theme::font_heading(16.0)));
                    ui.add_space(4.0);
                });

                egui::SidePanel::left("messages_peer_list")
                    .resizable(true)
                    .default_width(200.0)
                    .width_range(160.0..=320.0)
                    .show(child_ctx, |ui| app.show_messages_peer_list(ui, &peers));

                egui::CentralPanel::default()
                    .show(child_ctx, |ui| app.show_messages_thread_and_compose(ui));

                if child_ctx.input(|i| i.viewport().close_requested()) {
                    app.messages_window_open = false;
                }
            },
        );
    }

    /// Distinct peers, most-recently-active first -- `messages.messages` is
    /// already newest-first, so the first occurrence of each `peer_uri`
    /// while walking it in order is exactly that peer's latest activity.
    fn message_peers(&self) -> Vec<String> {
        let mut peers: Vec<String> = Vec::new();
        for m in &self.messages.messages {
            if !peers.iter().any(|p| p == &m.peer_uri) {
                peers.push(m.peer_uri.clone());
            }
        }
        peers
    }

    /// Left-pane conversation list -- avatar + resolved name only, "modern
    /// chat app" style. Clicking a row re-scopes the window to that peer;
    /// this is the *only* way to switch conversations once the window is
    /// open (no picker/dropdown -- that redundancy is what this whole
    /// redesign was for).
    fn show_messages_peer_list(&mut self, ui: &mut Ui, peers: &[String]) {
        let palette = self.palette;
        if peers.is_empty() {
            empty_state(ui, &palette, "No conversations yet.");
            return;
        }
        egui::ScrollArea::vertical()
            .id_source("messages_peer_list_scroll")
            .show(ui, |ui| self.show_messages_peer_rows(ui, peers));
    }

    fn show_messages_peer_rows(&mut self, ui: &mut Ui, peers: &[String]) {
        let palette = self.palette;
        for peer in peers {
            let selected = self.messages_window_peer.as_deref() == Some(peer.as_str());
            let (name, _) = resolve_caller(&self.contacts, peer);
            let bg_idx = ui.painter().add(egui::Shape::Noop);
            let row = ui
                .push_id(peer.as_str(), |ui| {
                    ui.horizontal(|ui| {
                        avatar(ui, &name, peer);
                        ui.add_space(6.0);
                        ui.label(RichText::new(&name).font(crate::theme::font_medium(13.0)));
                    })
                })
                .inner
                .response;
            let bg = if selected || row.hovered() {
                palette.surface_hover
            } else {
                palette.canvas
            };
            ui.painter().set(bg_idx, egui::Shape::rect_filled(row.rect, 0.0, bg));
            list_row_divider(ui, &palette, row.rect);
            if row.interact(egui::Sense::click()).clicked() {
                self.messages_window_peer = Some(peer.clone());
            }
        }
    }

    /// Right-pane thread + compose box for `messages_window_peer`. The
    /// compose bar is reserved *before* the thread (same fix as this
    /// window's prior tab-body incarnation had -- see its own history):
    /// anchoring fixed-size chrome (header, compose bar) via
    /// `TopBottomPanel`s first means the thread's `ScrollArea` in between
    /// only ever fills whatever's actually left, regardless of window
    /// height or thread length.
    fn show_messages_thread_and_compose(&mut self, ui: &mut Ui) {
        let Some(peer) = self.messages_window_peer.clone() else {
            empty_state(ui, &self.palette, "Select a conversation");
            return;
        };

        egui::TopBottomPanel::top("messages_thread_header").show_inside(ui, |ui| {
            let (name, _) = resolve_caller(&self.contacts, &peer);
            ui.add_space(4.0);
            ui.label(RichText::new(name).font(crate::theme::font_heading(14.0)));
            ui.add_space(2.0);
            ui.separator();
        });

        egui::TopBottomPanel::bottom("messages_compose_panel").show_inside(ui, |ui| {
            ui.add_space(4.0);
            let palette = self.palette;
            text_edit_scope(ui, &palette, |ui| ui.add(
                egui::TextEdit::multiline(&mut self.message_body)
                    .desired_rows(3)
                    .hint_text(RichText::new("Message text").color(palette.ink_muted))
                    .desired_width(f32::INFINITY),
            ));
            ui.add_space(4.0);
            let can_send = !self.message_body.trim().is_empty()
                && self.reg_ok
                && self.selected_account_idx().is_some();
            if ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .clicked()
            {
                self.do_send_message(peer.clone());
            }
            ui.add_space(4.0);
        });

        let thread: Vec<&Message> = self
            .messages
            .messages
            .iter()
            .filter(|m| m.peer_uri == peer)
            .rev()
            .collect();

        let palette = self.palette;
        egui::ScrollArea::vertical()
            .id_source("messages_thread_scroll")
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

    fn do_send_message(&mut self, to: String) {
        let Some(acc) = self.selected_account_idx() else {
            return;
        };
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
        self.messages_window_peer = Some(to);
    }
}
