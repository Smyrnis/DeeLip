use deelip_config::UpdateCheckFrequency;
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{empty_state, field_label, info_hint, settings_section, text_edit_scope};
use crate::strings::{t, tf};
use crate::theme::Palette;

impl DeelipApp {
    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_updates_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(
            ui,
            palette,
            &t("settings.advanced.updates_section_title"),
            Some(&t("settings.applies_immediately_hint")),
            |ui| {
                ui.label(
                    RichText::new(tf("settings.advanced.version_label", &[("version", env!("CARGO_PKG_VERSION"))]))
                        .color(palette.ink_muted),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.advanced.check_for_updates_label"));
                    egui::ComboBox::from_id_source("settings_update_check_frequency")
                        .selected_text(match self.config.update_check_frequency {
                            UpdateCheckFrequency::Always => t("settings.advanced.freq_every_launch"),
                            UpdateCheckFrequency::Daily => t("settings.advanced.freq_daily"),
                            UpdateCheckFrequency::Weekly => t("settings.advanced.freq_weekly"),
                            UpdateCheckFrequency::Never => t("settings.advanced.freq_never"),
                        })
                        .show_ui(ui, |ui| {
                            for (val, label) in [
                                (UpdateCheckFrequency::Always, t("settings.advanced.freq_every_launch")),
                                (UpdateCheckFrequency::Daily, t("settings.advanced.freq_daily")),
                                (UpdateCheckFrequency::Weekly, t("settings.advanced.freq_weekly")),
                                (UpdateCheckFrequency::Never, t("settings.advanced.freq_never")),
                            ] {
                                if ui.selectable_value(&mut self.config.update_check_frequency, val, label).changed() {
                                    self.save_config_quietly();
                                }
                            }
                        });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut self.config.auto_update_enabled, t("settings.advanced.auto_update_checkbox"))
                        .changed()
                    {
                        self.save_config_quietly();
                    }
                    info_hint(ui, palette, &t("settings.advanced.auto_update_hint"));
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(t("settings.advanced.check_now_button")).clicked() {
                        self.start_update_check();
                    }
                    let status = match &self.update_state {
                        crate::update::UpdateState::Idle => t("settings.advanced.update_idle"),
                        crate::update::UpdateState::Checking => t("settings.advanced.update_checking"),
                        crate::update::UpdateState::Available(r) => {
                            tf("settings.advanced.update_available", &[("version", &r.version)])
                        }
                        crate::update::UpdateState::Downloading => t("settings.advanced.update_downloading"),
                        crate::update::UpdateState::Updated(v) => {
                            tf("settings.advanced.update_updated", &[("version", v)])
                        }
                        crate::update::UpdateState::Failed(e) => {
                            tf("settings.advanced.update_failed", &[("error", &e.to_string())])
                        }
                    };
                    ui.label(RichText::new(status).color(palette.ink_muted).small());
                });
            },
        );
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_blocklist_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(
            ui,
            palette,
            &t("settings.advanced.blocklist_section_title"),
            Some(&t("settings.applies_immediately_hint")),
            |ui| {
                ui.horizontal(|ui| {
                    text_edit_scope(ui, palette, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.blocklist_input)
                                .hint_text(
                                    RichText::new(t("settings.advanced.blocklist_hint")).color(palette.ink_muted),
                                )
                                .desired_width(200.0),
                        )
                    });
                    if ui.button(t("history.block_button")).clicked() {
                        let entry = self.blocklist_input.trim().to_string();
                        if !entry.is_empty() && !self.config.blocklist.iter().any(|e| e.eq_ignore_ascii_case(&entry)) {
                            self.config.blocklist.push(entry);
                            self.save_config_quietly();
                        }
                        self.blocklist_input.clear();
                    }
                });
                if self.config.blocklist.is_empty() {
                    empty_state(ui, palette, &t("settings.advanced.no_blocked_numbers"));
                } else {
                    let mut remove_idx = None;
                    for (i, entry) in self.config.blocklist.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(entry);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(t("common.remove_button")).clicked() {
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
            },
        );
    }

    /// Moved here from History's own search bar -- see the redesign plan's
    /// "Settings: History export + Contacts import/export" section.
    pub(super) fn show_history_export_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, &t("settings.advanced.call_history_section_title"), None, |ui| {
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.advanced.export_history_label"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("settings.advanced.export_button")).clicked() {
                        self.export_history_csv();
                    }
                });
            });
        });
    }

    /// Moved here from Contacts' own search bar -- see the redesign plan's
    /// "Settings: History export + Contacts import/export" section.
    pub(super) fn show_contacts_data_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, &t("settings.advanced.contacts_data_section_title"), None, |ui| {
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.advanced.import_contacts_label"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("settings.advanced.import_button")).clicked() {
                        self.import_contacts();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.advanced.export_contacts_csv_label"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("settings.advanced.export_button")).clicked() {
                        self.export_contacts_csv();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.advanced.export_contacts_vcard_label"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("settings.advanced.export_button")).clicked() {
                        self.export_contacts_vcard();
                    }
                });
            });
        });
    }
}
