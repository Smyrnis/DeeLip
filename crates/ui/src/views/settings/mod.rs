//! Settings window: one file per tab (general/account/audio/video/network/
//! directory/hotkeys/advanced) -- see this crate's Architecture notes for why
//! the split is purely organizational. This file keeps only the shared
//! scaffolding: opening the pop-out window, the tab-strip/dispatch logic, and
//! a few small field-editing helpers used across more than one tab.

mod account;
mod advanced;
mod audio;
mod directory;
mod general;
mod hotkeys;
mod network;
mod video;

use egui::{RichText, Ui};

use crate::app::{DeelipApp, SettingsTab, SharedApp};
use crate::helpers::{show_pop_out_window, text_edit_scope};
use crate::strings::t;
use crate::theme::Palette;

/// Shared between `show_settings_modal` (which opens the viewport under
/// this id) and the background device-scan spawns in `audio.rs`/`video.rs`,
/// which need the same id to wake *this* viewport specifically -- see
/// `docs/crates/ui.md`'s Settings section for why.
const SETTINGS_VIEWPORT_NAME: &str = "deelip_settings_window";

impl DeelipApp {
    /// Settings as a separate, genuinely movable native OS window rather
    /// than a tab. No-op when closed. Full history: `docs/crates/ui.md`'s
    /// "Pop-out windows" section.
    pub(crate) fn show_settings_modal(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        // `SETTINGS_VIEWPORT_NAME` is passed as `key` so its hash matches
        // what the background device-scan spawns elsewhere in this module
        // already wake via `request_repaint_of`.
        show_pop_out_window(
            self,
            ctx,
            self_app,
            SETTINGS_VIEWPORT_NAME,
            format!("DeeLip {}", t("settings.window_title")),
            // Sized so every tab but Account/Advanced fits without scrolling
            // -- see docs/crates/ui.md's Settings section.
            [950.0, 740.0],
            [580.0, 520.0],
            true,
            |app| app.settings_ui.settings_open,
            |app| app.settings_ui.settings_open = false,
            |_app| t("settings.window_title"),
            |app, ui| {
                ui.separator();
                app.show_settings(ui);
            },
        );
    }

    /// Renders every Settings section in order inside the scroll area, then
    /// the trailing Save button. Each section is its own method (see the
    /// per-tab files in this directory) -- this is just the scaffolding:
    /// `edited` accumulates whether any restart-required field changed (the
    /// "applies immediately" sections save themselves as they go and don't
    /// feed into it).
    fn show_settings(&mut self, ui: &mut Ui) {
        if self.config.accounts.is_empty() {
            self.config.accounts.push(deelip_config::SipAccount::default());
        }
        self.settings_ui.edit_account_idx = self.settings_ui.edit_account_idx.min(self.config.accounts.len() - 1);
        let palette = self.palette;

        // A tab strip -- one section visible at a time, sized to fit
        // without scrolling, instead of the previous single long
        // `ScrollArea` stacking all 12 sections in one column.
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::General, t("settings.tab_general"));
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::Account, t("settings.tab_account"));
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::Audio, t("settings.tab_audio"));
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::Video, t("settings.tab_video"));
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::Network, t("settings.tab_network"));
            ui.selectable_value(
                &mut self.settings_ui.settings_tab,
                SettingsTab::Directory,
                t("settings.tab_directory"),
            );
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::Hotkeys, t("settings.tab_hotkeys"));
            ui.selectable_value(&mut self.settings_ui.settings_tab, SettingsTab::Advanced, t("settings.tab_advanced"));
        });
        ui.separator();
        ui.add_space(6.0);

        // Reserved *before* the tab content below -- see docs/crates/ui.md's
        // Settings section for why a scrolling tab needs this ordering.
        egui::Panel::bottom("settings_save_panel").show_inside(ui, |ui| {
            ui.add_space(8.0);
            if ui.button(t("common.save_button")).clicked() {
                match self.config.save(&self.db) {
                    Ok(()) => self.settings_ui.settings_saved_notice = true,
                    Err(err) => {
                        self.settings_ui.settings_saved_notice = false;
                        tracing::error!("Failed to save config: {err}");
                    }
                }
            }
            if self.settings_ui.settings_saved_notice {
                ui.label(RichText::new(t("settings.saved_restart_notice")).color(palette.signal));
            }
            ui.add_space(4.0);
        });

        let edited = match self.settings_ui.settings_tab {
            SettingsTab::General => {
                self.show_notifications_section(ui, &palette);
                ui.add_space(14.0);
                self.show_call_handling_section(ui, &palette);
                ui.add_space(14.0);
                self.show_startup_section(ui, &palette)
            }
            // One of two tabs that scroll internally by necessity -- see
            // docs/crates/ui.md's Settings section.
            SettingsTab::Account => {
                let mut edited = false;
                egui::ScrollArea::vertical().id_salt("account_tab_scroll").show(ui, |ui| {
                    edited = self.show_account_section(ui, &palette);
                });
                edited
            }
            SettingsTab::Audio => self.show_audio_section(ui, &palette),
            SettingsTab::Video => self.show_video_section(ui, &palette),
            SettingsTab::Network => self.show_network_section(ui, &palette),
            SettingsTab::Directory => self.show_directory_section(ui, &palette),
            SettingsTab::Hotkeys => self.show_global_hotkeys_section(ui, &palette),
            // Same exception as Account above -- its 4 stacked sections
            // overflow past the window bottom.
            SettingsTab::Advanced => {
                egui::ScrollArea::vertical().id_salt("advanced_tab_scroll").show(ui, |ui| {
                    self.show_updates_section(ui, &palette);
                    ui.add_space(14.0);
                    self.show_blocklist_section(ui, &palette);
                    ui.add_space(14.0);
                    self.show_history_export_section(ui, &palette);
                    ui.add_space(14.0);
                    self.show_contacts_data_section(ui, &palette);
                });
                false
            }
        };

        if edited {
            self.settings_ui.settings_saved_notice = false;
        }
    }
}

/// Text field bound to an `Option<String>` — an empty field maps to `None`.
/// Shared across the Account/Network/Directory tabs.
pub(super) fn optional_text_field(ui: &mut Ui, palette: &Palette, value: &mut Option<String>, hint: &str) -> bool {
    optional_text_field_sized(ui, palette, value, hint, f32::INFINITY)
}

/// Same as `optional_text_field`, but with a caller-chosen width instead of
/// always filling the rest of the row -- for a row that needs to fit
/// something else (a trailing label/control) after the field.
pub(super) fn optional_text_field_sized(
    ui: &mut Ui, palette: &Palette, value: &mut Option<String>, hint: &str, width: f32,
) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = text_edit_scope(ui, palette, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(RichText::new(hint).color(palette.ink_muted))
                .desired_width(width),
        )
        .changed()
    });
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

/// Masked text field bound to an `Option<String>` — an empty field maps to
/// `None`. Shared across the Network/Directory tabs.
pub(super) fn optional_password_field(ui: &mut Ui, palette: &Palette, value: &mut Option<String>) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = text_edit_scope(ui, palette, |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).password(true).desired_width(f32::INFINITY)).changed()
    });
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}
