use deelip_config::{CallDirection, CallRecord, CallStatus};
use egui::{RichText, Ui};

use crate::app::{DeelipApp, Tab};
use crate::helpers::{csv_escape, extract_user_part, format_age, format_duration, short_uri, status_filter_label};

impl DeelipApp {
    pub(crate) fn show_history(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        ui.add_space(8.0);
        if self.history.records.is_empty() {
            ui.label("No call history yet.");
            return;
        }

        // ── Search / filter / export bar ─────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.history_search)
                    .desired_width(140.0)
                    .hint_text("name or URI"),
            );
            ui.label("Status:");
            egui::ComboBox::from_id_source("history_status_filter")
                .selected_text(status_filter_label(&self.history_status_filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.history_status_filter, None, "All");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Answered), "Answered");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Missed), "Missed");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Rejected), "Rejected");
                    ui.selectable_value(&mut self.history_status_filter, Some(CallStatus::Failed), "Failed");
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let export = format!("{}  Export CSV…", egui_phosphor::regular::DOWNLOAD_SIMPLE);
                if ui.button(export).clicked() {
                    self.export_history_csv();
                }
            });
        });
        ui.add_space(4.0);

        let query = self.history_search.to_lowercase();
        let filtered: Vec<&CallRecord> = self.history.records.iter()
            .filter(|r| self.history_status_filter.as_ref().is_none_or(|s| *s == r.status))
            .filter(|r| query.is_empty() || r.remote_uri.to_lowercase().contains(&query))
            .collect();

        let mut call_target: Option<String> = None;
        let mut block_target: Option<String> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            if filtered.is_empty() {
                ui.label(RichText::new("No matching calls.").color(self.palette.muted));
            }
            for record in filtered {
                let (dir_icon, dir_color) = match record.direction {
                    CallDirection::Inbound  => (egui_phosphor::regular::PHONE_INCOMING, self.palette.info),
                    CallDirection::Outbound => (egui_phosphor::regular::PHONE_OUTGOING, self.palette.accent),
                };
                let status_str = match record.status {
                    CallStatus::Answered => format_duration(record.duration_secs),
                    CallStatus::Missed   => "Missed".into(),
                    CallStatus::Rejected => "Rejected".into(),
                    CallStatus::Failed   => "Failed".into(),
                };

                ui.horizontal(|ui| {
                    ui.label(RichText::new(dir_icon).color(dir_color));
                    ui.label(short_uri(&record.remote_uri));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Call").clicked() {
                            call_target = Some(record.remote_uri.clone());
                        }
                        if ui.small_button("Block").clicked() {
                            block_target = Some(record.remote_uri.clone());
                        }
                        ui.label(RichText::new(&status_str).color(self.palette.muted));
                        ui.label(RichText::new(format_age(record.timestamp)).color(self.palette.muted));
                    });
                });
                ui.separator();
            }
        });

        if let Some(target) = call_target {
            self.tab         = Tab::Dialer;
            self.call_target = target.clone();
            let can_dial = self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
            if can_dial && self.reg_ok {
                self.do_call(Some(target));
            }
        }
        if let Some(target) = block_target {
            let entry = extract_user_part(&target);
            if !self.config.blocklist.iter().any(|e| extract_user_part(e) == entry) {
                self.config.blocklist.push(target);
                self.save_config_quietly();
            }
        }
    }

    /// Export the currently filtered history view (respecting the search box
    /// and status dropdown) to a CSV file via a native save dialog.
    pub(crate) fn export_history_csv(&self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("deelip_history.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
        else {
            return;
        };

        let query = self.history_search.to_lowercase();
        let filtered = self.history.records.iter()
            .filter(|r| self.history_status_filter.as_ref().is_none_or(|s| *s == r.status))
            .filter(|r| query.is_empty() || r.remote_uri.to_lowercase().contains(&query));

        let mut csv = String::from("timestamp,direction,remote_uri,status,duration_secs\n");
        for r in filtered {
            let direction = match r.direction {
                CallDirection::Inbound  => "inbound",
                CallDirection::Outbound => "outbound",
            };
            let status = match r.status {
                CallStatus::Answered => "answered",
                CallStatus::Missed   => "missed",
                CallStatus::Rejected => "rejected",
                CallStatus::Failed   => "failed",
            };
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                r.timestamp, direction, csv_escape(&r.remote_uri), status, r.duration_secs,
            ));
        }

        if let Err(e) = std::fs::write(&path, csv) {
            tracing::error!("Failed to export history to {}: {e}", path.display());
        }
    }
}
