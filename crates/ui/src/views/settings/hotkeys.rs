use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint};
use crate::strings::t;
use crate::theme::{self, Palette};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_global_hotkeys_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new(t("settings.tab_hotkeys")).font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut self.config.global_hotkeys_enabled, t("settings.hotkeys.enable_global_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.hotkeys.format_hint"));
            });
            if self.config.global_hotkeys_enabled {
                egui::Grid::new("hotkeys_grid").num_columns(2).show(ui, |ui| {
                    field_label(ui, palette, &t("settings.hotkeys.answer_label"));
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_answer).changed();
                    ui.end_row();
                    field_label(ui, palette, &t("settings.hotkeys.hangup_label"));
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_hangup).changed();
                    ui.end_row();
                    field_label(ui, palette, &t("settings.hotkeys.mute_label"));
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_mute).changed();
                    ui.end_row();
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut self.config.handle_media_buttons, t("settings.hotkeys.media_buttons_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.hotkeys.media_buttons_hint"));
            });
        });
        edited
    }
}
