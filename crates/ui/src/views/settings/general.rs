use deelip_config::DefaultListAction;
use egui::Ui;

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint, settings_section};
use crate::theme::Palette;

impl DeelipApp {
    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_notifications_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Notifications & Ringtone", Some("Applies immediately — no restart needed."), |ui| {
            if ui.checkbox(&mut self.config.notifications_enabled, "Desktop notification on incoming calls").changed() {
                self.save_config_quietly();
            }
            if ui.checkbox(&mut self.config.ringtone_enabled, "Ringtone (incoming) / ringback (outgoing)").changed() {
                self.save_config_quietly();
            }
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.config.random_popup_position, "Random popup position").changed() {
                    self.save_config_quietly();
                }
                info_hint(ui, palette, "Show the main window at a random spot on the current \
                    monitor each time it's raised for an incoming call, instead of wherever it \
                    last was.");
            });
            ui.horizontal(|ui| {
                field_label(ui, palette, "Default list action:");
                egui::ComboBox::from_id_source("settings_default_list_action")
                    .selected_text(match self.config.default_list_action {
                        DefaultListAction::Call => "Call",
                        DefaultListAction::Message => "Message",
                        DefaultListAction::Edit => "Edit",
                    })
                    .show_ui(ui, |ui| {
                        for (val, label) in [
                            (DefaultListAction::Call, "Call"),
                            (DefaultListAction::Message, "Message"),
                            (DefaultListAction::Edit, "Edit"),
                        ] {
                            if ui.selectable_value(&mut self.config.default_list_action, val, label).changed() {
                                self.save_config_quietly();
                            }
                        }
                    });
                info_hint(ui, palette, "What double-clicking a row's name/number in History or \
                    Contacts does. \"Edit\" falls back to \"Call\" in History (nothing to edit there).");
            });
        });
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    pub(super) fn show_call_handling_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Call Handling", Some("Applies immediately — no restart needed."), |ui| {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.config.single_call_mode, "Single Call Mode (disable call waiting)").changed() {
                    self.save_config_quietly();
                }
                info_hint(ui, palette, "An incoming call while another is already active is \
                    rejected with Busy instead of ringing as a 2nd call. A per-account \
                    \"Forward on busy\" (Account editor) still takes priority over this.");
            });
        });
    }

    /// Restart required -- returns whether anything changed.
    pub(super) fn show_startup_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        settings_section(ui, palette, "Startup", None, |ui| {
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.start_minimized, "Start minimized (to tray)").changed();
                info_hint(ui, palette, "Restart to apply.");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.log_to_file, "Enable log file").changed();
                info_hint(ui, palette, "Also writes logs to ~/.config/deelip/deelip.log, \
                    in addition to the console. Restart to apply.");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.crash_reporting_enabled, "Save crash reports locally").changed();
                info_hint(ui, palette, "If DeeLip crashes, save a report (version, panic message, \
                    backtrace) to ~/.config/deelip/crashes/ for troubleshooting. Purely local -- \
                    never uploaded or sent anywhere. Restart to apply.");
            });
            ui.horizontal(|ui| {
                if ui.button("Open crash reports folder").clicked() {
                    if let Ok(dir) = deelip_config::crashes_dir() {
                        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.autostart_enabled, "Start DeeLip on login").changed() {
                    if let Err(e) = deelip_config::set_autostart(self.autostart_enabled) {
                        tracing::error!("Failed to update autostart: {e}");
                        self.autostart_enabled = deelip_config::is_autostart_enabled();
                    }
                }
                info_hint(ui, palette, "Applies immediately — no restart needed.");
            });
        });
        edited
    }
}
