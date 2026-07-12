use deelip_config::DefaultListAction;
use egui::Ui;

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint, settings_section};
use crate::strings::t;
use crate::theme::Palette;

impl DeelipApp {
    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_notifications_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(
            ui,
            palette,
            &t("settings.notifications_section_title"),
            Some(&t("settings.applies_immediately_hint")),
            |ui| {
                if ui
                    .checkbox(&mut self.config.notifications_enabled, t("settings.desktop_notification_checkbox"))
                    .changed()
                {
                    self.save_config_quietly();
                }
                if ui.checkbox(&mut self.config.ringtone_enabled, t("settings.ringtone_checkbox")).changed() {
                    self.save_config_quietly();
                }
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut self.config.random_popup_position, t("settings.random_popup_checkbox"))
                        .changed()
                    {
                        self.save_config_quietly();
                    }
                    info_hint(ui, palette, &t("settings.random_popup_hint"));
                });
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.default_list_action_label"));
                    egui::ComboBox::from_id_salt("settings_default_list_action")
                        .selected_text(match self.config.default_list_action {
                            DefaultListAction::Call => t("common.call_button"),
                            DefaultListAction::Message => t("common.message_button"),
                            DefaultListAction::Edit => t("common.edit_button"),
                        })
                        .show_ui(ui, |ui| {
                            for (val, label) in [
                                (DefaultListAction::Call, t("common.call_button")),
                                (DefaultListAction::Message, t("common.message_button")),
                                (DefaultListAction::Edit, t("common.edit_button")),
                            ] {
                                if ui.selectable_value(&mut self.config.default_list_action, val, label).changed() {
                                    self.save_config_quietly();
                                }
                            }
                        });
                    info_hint(ui, palette, &t("settings.default_list_action_hint"));
                });
            },
        );
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_call_handling_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(
            ui,
            palette,
            &t("settings.call_handling_section_title"),
            Some(&t("settings.applies_immediately_hint")),
            |ui| {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.config.single_call_mode, t("settings.single_call_mode_checkbox")).changed()
                    {
                        self.save_config_quietly();
                    }
                    info_hint(ui, palette, &t("settings.single_call_mode_hint"));
                });
            },
        );
    }

    /// Restart required -- returns whether anything changed.
    pub(super) fn show_startup_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        settings_section(ui, palette, &t("settings.startup_section_title"), None, |ui| {
            ui.horizontal(|ui| {
                edited |=
                    ui.checkbox(&mut self.config.start_minimized, t("settings.start_minimized_checkbox")).changed();
                info_hint(ui, palette, &t("settings.restart_to_apply_hint"));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.log_to_file, t("settings.log_to_file_checkbox")).changed();
                info_hint(ui, palette, &t("settings.log_to_file_hint"));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut self.config.crash_reporting_enabled, t("settings.crash_reporting_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.crash_reporting_hint"));
            });
            ui.horizontal(|ui| {
                if ui.button(t("settings.open_crash_reports_button")).clicked()
                    && let Ok(dir) = deelip_config::crashes_dir()
                {
                    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.autostart_enabled, t("settings.start_on_login_checkbox")).changed()
                    && let Err(e) = deelip_config::set_autostart(self.autostart_enabled)
                {
                    tracing::error!("Failed to update autostart: {e}");
                    self.autostart_enabled = deelip_config::is_autostart_enabled();
                }
                info_hint(ui, palette, &t("settings.applies_immediately_hint"));
            });
        });
        edited
    }
}
