use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint};
use crate::theme::{self, Palette};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_global_hotkeys_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new("Global Hotkeys").font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.global_hotkeys_enabled,
                    "Enable system-wide Answer/Hangup/Mute hotkeys (Linux: X11 only)"
                ).changed();
                info_hint(ui, palette, "Format: \"Ctrl+Alt+A\" style. Restart required to apply.");
            });
            if self.config.global_hotkeys_enabled {
                egui::Grid::new("hotkeys_grid").num_columns(2).show(ui, |ui| {
                    field_label(ui, palette, "Answer:");
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_answer).changed();
                    ui.end_row();
                    field_label(ui, palette, "Hangup:");
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_hangup).changed();
                    ui.end_row();
                    field_label(ui, palette, "Mute:");
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_mute).changed();
                    ui.end_row();
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.handle_media_buttons,
                    "Handle Media Buttons (headset Play/Pause answers/hangs up)"
                ).changed();
                info_hint(ui, palette, "Independent of the toggle above -- grabs the hardware \
                    media Play/Pause key (Linux: X11 only) to answer a ringing call or hang up \
                    the active one, like a headset's hook button. Restart required to apply.");
            });
        });
        edited
    }
}
