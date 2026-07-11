use deelip_config::UpdateCheckFrequency;
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{empty_state, field_label, info_hint, settings_section, text_edit_scope};
use crate::theme::Palette;

impl DeelipApp {
    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_updates_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Updates", Some("Applies immediately — no restart needed."), |ui| {
            ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(palette.ink_muted));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Check for updates:");
                egui::ComboBox::from_id_source("settings_update_check_frequency")
                    .selected_text(match self.config.update_check_frequency {
                        UpdateCheckFrequency::Always => "Every launch",
                        UpdateCheckFrequency::Daily => "Daily",
                        UpdateCheckFrequency::Weekly => "Weekly",
                        UpdateCheckFrequency::Never => "Never",
                    })
                    .show_ui(ui, |ui| {
                        for (val, label) in [
                            (UpdateCheckFrequency::Always, "Every launch"),
                            (UpdateCheckFrequency::Daily, "Daily"),
                            (UpdateCheckFrequency::Weekly, "Weekly"),
                            (UpdateCheckFrequency::Never, "Never"),
                        ] {
                            if ui.selectable_value(&mut self.config.update_check_frequency, val, label).changed() {
                                self.save_config_quietly();
                            }
                        }
                    });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.config.auto_update_enabled, "Automatically download and install updates").changed() {
                    self.save_config_quietly();
                }
                info_hint(ui, palette, "Only works for a portable (tar.gz/install.sh) install -- \
                    .deb/.rpm installs are always updated through your package manager instead, \
                    regardless of this toggle.");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Check for updates now").clicked() {
                    self.start_update_check();
                }
                let status = match &self.update_state {
                    crate::update::UpdateState::Idle       => "Up to date (or not checked yet).".to_string(),
                    crate::update::UpdateState::Checking    => "Checking…".to_string(),
                    crate::update::UpdateState::Available(r) => format!("Update available: {}", r.version),
                    crate::update::UpdateState::Downloading => "Downloading update…".to_string(),
                    crate::update::UpdateState::Updated(v)  => format!("Updated to {v} -- restart to finish."),
                    crate::update::UpdateState::Failed(e)   => format!("Check failed: {e}"),
                };
                ui.label(RichText::new(status).color(palette.ink_muted).small());
            });
        });
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_blocklist_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Blocklist", Some("Applies immediately — no restart needed."), |ui| {
            ui.horizontal(|ui| {
                text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut self.blocklist_input)
                    .hint_text(RichText::new("number or sip:user@host").color(palette.ink_muted))
                    .desired_width(200.0)));
                if ui.button("Block").clicked() {
                    let entry = self.blocklist_input.trim().to_string();
                    if !entry.is_empty() && !self.config.blocklist.iter().any(|e| e.eq_ignore_ascii_case(&entry)) {
                        self.config.blocklist.push(entry);
                        self.save_config_quietly();
                    }
                    self.blocklist_input.clear();
                }
            });
            if self.config.blocklist.is_empty() {
                empty_state(ui, palette, "No blocked numbers.");
            } else {
                let mut remove_idx = None;
                for (i, entry) in self.config.blocklist.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(entry);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove_idx {
                    self.config.blocklist.remove(i);
                    self.save_config_quietly();
                }
            }
        });
    }

    /// Moved here from History's own search bar -- see the redesign plan's
    /// "Settings: History export + Contacts import/export" section.
    pub(super) fn show_history_export_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Call History", None, |ui| {
            ui.horizontal(|ui| {
                field_label(ui, palette, "Export call history to CSV");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export…").clicked() {
                        self.export_history_csv();
                    }
                });
            });
        });
    }

    /// Moved here from Contacts' own search bar -- see the redesign plan's
    /// "Settings: History export + Contacts import/export" section.
    pub(super) fn show_contacts_data_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Contacts Import / Export", None, |ui| {
            ui.horizontal(|ui| {
                field_label(ui, palette, "Import from CSV or vCard");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Import…").clicked() {
                        self.import_contacts();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Export as CSV");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export…").clicked() {
                        self.export_contacts_csv();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Export as vCard");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export…").clicked() {
                        self.export_contacts_vcard();
                    }
                });
            });
        });
    }
}
