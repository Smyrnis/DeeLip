use deelip_config::{CallStatus, Contact, Direction};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{
    call_status_csv_label, call_status_icon_color, call_status_label, csv_escape, double_clickable_label, empty_state,
    extract_user_part, format_duration, format_timestamp, list_row_menu, resolve_caller, search_field,
    status_filter_label,
};
use crate::strings::t;

impl DeelipApp {
    pub(crate) fn show_history(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        ui.add_space(8.0);
        if self.history_state.history.records.is_empty() {
            empty_state(ui, &self.palette, &t("history.no_history"));
            return;
        }

        // ── Search / filter / export bar ─────────────────────────────────────
        let palette = self.palette;
        ui.horizontal(|ui| {
            search_field(
                ui,
                &palette,
                &mut self.history_state.history_search,
                &t("common.search_hint_name_or_number"),
                140.0,
            );
            ui.label(t("history.status_label"));
            egui::ComboBox::from_id_salt("history_status_filter")
                .selected_text(status_filter_label(&self.history_state.history_status_filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.history_state.history_status_filter, None, t("history.status_all"));
                    ui.selectable_value(
                        &mut self.history_state.history_status_filter,
                        Some(CallStatus::Answered),
                        call_status_label(&CallStatus::Answered),
                    );
                    ui.selectable_value(
                        &mut self.history_state.history_status_filter,
                        Some(CallStatus::Missed),
                        call_status_label(&CallStatus::Missed),
                    );
                    ui.selectable_value(
                        &mut self.history_state.history_status_filter,
                        Some(CallStatus::Rejected),
                        call_status_label(&CallStatus::Rejected),
                    );
                    ui.selectable_value(
                        &mut self.history_state.history_status_filter,
                        Some(CallStatus::Failed),
                        call_status_label(&CallStatus::Failed),
                    );
                });
        });
        ui.add_space(4.0);

        // Recompute the filtered index list only when the search text,
        // status filter, or the record count itself has actually changed --
        // this used to re-lowercase every record's URI and rebuild the list
        // on every single frame regardless (egui repaints continuously, and
        // much faster than that during a scroll drag), which was the actual
        // cause of this tab dropping frames while scrolling. Mirrors the
        // existing `audio_device_cache` idiom.
        let key = (
            self.history_state.history_search.to_lowercase(),
            self.history_state.history_status_filter.clone(),
            self.history_state.history.records.len(),
        );
        if self.history_state.history_filter_key.as_ref() != Some(&key) {
            let query = &key.0;
            self.history_state.history_filtered = self
                .history_state
                .history
                .records
                .iter()
                .enumerate()
                .filter(|(_, r)| self.history_state.history_status_filter.as_ref().is_none_or(|s| *s == r.status))
                .filter(|(_, r)| query.is_empty() || r.remote_uri.to_lowercase().contains(query))
                .map(|(i, _)| i)
                .collect();
            self.history_state.history_filter_key = Some(key);
        }

        let mut call_target: Option<String> = None;
        let mut block_target: Option<String> = None;
        let mut message_target: Option<String> = None;
        let mut copy_target: Option<String> = None;
        let mut delete_idx: Option<usize> = None;
        let mut default_action_target: Option<String> = None;
        let mut add_contact_target: Option<(String, String)> = None;

        if self.history_state.history_filtered.is_empty() {
            empty_state(ui, &self.palette, &t("history.no_matching_calls"));
        } else {
            // `show_rows` only lays out the rows actually scrolled into view
            // instead of all of them every frame -- with up to 200 records
            // and this app's continuous ~20fps repaint, the plain `show`
            // form was doing thousands of unnecessary widget layouts/sec.
            // `show_rows` needs one precisely-known height per iteration --
            // each row is deliberately kept to a *single* widget (the
            // `ui.horizontal` group, divider painted directly onto its own
            // rect below) rather than two siblings (group + a separate
            // `ui.separator()`), since two widgets means two auto-inserted
            // `item_spacing.y` gaps that a single flat height estimate can't
            // represent -- that mismatch was the actual cause of this tab's
            // scroll jitter, not raw row count.
            let row_height = ui.spacing().interact_size.y.max(ui.text_style_height(&egui::TextStyle::Body))
                + ui.spacing().item_spacing.y;
            let filtered = &self.history_state.history_filtered;
            let records = &self.history_state.history.records;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
                ui,
                row_height,
                filtered.len(),
                |ui, row_range| {
                    for idx in row_range {
                        let real_idx = filtered[idx];
                        let record = &records[real_idx];
                        let (dir_icon, dir_color) = match record.direction {
                            Direction::Inbound => (egui_phosphor::regular::PHONE_INCOMING, self.palette.ink_muted),
                            Direction::Outbound => (egui_phosphor::regular::PHONE_OUTGOING, self.palette.signal),
                        };
                        let (status_icon, status_color) = call_status_icon_color(&record.status, &self.palette);
                        // `Answered` shows the call duration instead of the
                        // word "Answered" -- see `call_status_label`'s own
                        // doc comment for why that one case is special-cased
                        // here rather than folded into the shared helper.
                        let status_str = if record.status == CallStatus::Answered {
                            format_duration(record.duration_secs)
                        } else {
                            call_status_label(&record.status)
                        };
                        let (display_name, is_name) = resolve_caller(&self.contacts_state.contacts, &record.remote_uri);

                        let palette = self.palette;
                        let remote_uri = record.remote_uri.clone();
                        let display_name_for_menu = display_name.clone();
                        list_row_menu(
                            ui,
                            &palette,
                            idx,
                            |ui| {
                                ui.label(RichText::new(dir_icon).color(dir_color));
                                let name_font = if is_name {
                                    crate::theme::font_medium(13.0)
                                } else {
                                    egui::FontId::new(12.0, egui::FontFamily::Monospace)
                                };
                                if double_clickable_label(ui, RichText::new(display_name).font(name_font)) {
                                    default_action_target = Some(record.remote_uri.clone());
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let mono = egui::FontId::new(11.5, egui::FontFamily::Monospace);
                                    ui.label(
                                        RichText::new(format_timestamp(record.timestamp))
                                            .font(mono.clone())
                                            .color(palette.ink_muted),
                                    );
                                    ui.label(RichText::new(&status_str).font(mono).color(palette.ink_muted));
                                    ui.label(RichText::new(status_icon).color(status_color));
                                });
                            },
                            |ui| {
                                if ui.button(t("common.call_button")).clicked() {
                                    call_target = Some(remote_uri.clone());
                                    ui.close();
                                }
                                if ui.button(t("common.message_button")).clicked() {
                                    message_target = Some(remote_uri.clone());
                                    ui.close();
                                }
                                if ui.button(t("common.copy_button")).clicked() {
                                    copy_target = Some(remote_uri.clone());
                                    ui.close();
                                }
                                if ui.button(t("history.block_button")).clicked() {
                                    block_target = Some(remote_uri.clone());
                                    ui.close();
                                }
                                ui.separator();
                                let already_contact = is_name;
                                if ui
                                    .add_enabled(!already_contact, egui::Button::new(t("directory.add_to_contacts")))
                                    .on_disabled_hover_text(t("history.already_in_contacts"))
                                    .clicked()
                                {
                                    add_contact_target = Some((remote_uri.clone(), display_name_for_menu.clone()));
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button(RichText::new(t("common.delete_button")).color(palette.danger)).clicked() {
                                    delete_idx = Some(real_idx);
                                    ui.close();
                                }
                            },
                        );
                    }
                },
            );
        }

        if let Some(target) = call_target {
            self.dial_from_list(target);
        }
        if let Some(target) = block_target {
            let entry = extract_user_part(&target);
            if !self.config.blocklist.iter().any(|e| extract_user_part(e) == entry) {
                self.config.blocklist.push(target);
                self.save_config_quietly();
            }
        }
        if let Some(target) = message_target {
            self.message_from_list(target);
        }
        if let Some(target) = copy_target {
            ui.ctx().copy_text(target);
        }
        if let Some(idx) = delete_idx {
            self.history_state.history.records.remove(idx);
            self.history_state.history_filter_key = None; // force the filtered list to recompute against the new indices
            if let Err(e) = self.history_state.history.save(&self.db) {
                tracing::error!("Failed to save call history: {e}");
            }
        }
        if let Some(target) = default_action_target {
            // `Edit` isn't meaningful for a History entry -- falls back to
            // `Call`, same as `DefaultListAction::Edit`'s own doc comment.
            match self.config.default_list_action {
                deelip_config::DefaultListAction::Message => self.message_from_list(target),
                deelip_config::DefaultListAction::Call | deelip_config::DefaultListAction::Edit => {
                    self.dial_from_list(target);
                }
            }
        }
        if let Some((remote_uri, display_name)) = add_contact_target {
            self.contacts_state.editing_contact_idx = None;
            self.contacts_state.new_contact = Contact { name: display_name, sip_uri: remote_uri, ..Default::default() };
            self.contacts_state.contact_dialog_open = true;
        }
    }

    /// Export the currently filtered history view (respecting the search box
    /// and status dropdown) to a CSV file via a native save dialog.
    pub(crate) fn export_history_csv(&self) {
        let query = self.history_state.history_search.to_lowercase();
        let filtered = self
            .history_state
            .history
            .records
            .iter()
            .filter(|r| self.history_state.history_status_filter.as_ref().is_none_or(|s| *s == r.status))
            .filter(|r| query.is_empty() || r.remote_uri.to_lowercase().contains(&query));

        let mut csv = String::from("timestamp,direction,remote_uri,status,duration_secs\n");
        for r in filtered {
            let direction = match r.direction {
                Direction::Inbound => "inbound",
                Direction::Outbound => "outbound",
            };
            let status = call_status_csv_label(&r.status);
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                r.timestamp,
                direction,
                csv_escape(&r.remote_uri),
                status,
                r.duration_secs,
            ));
        }

        crate::helpers::save_text_file("deelip_history.csv", "CSV", "csv", csv);
    }
}
