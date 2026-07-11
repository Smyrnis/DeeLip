//! Settings window: one file per tab (general/account/audio/video/network/
//! directory/hotkeys/advanced), all still just `impl DeelipApp` blocks --
//! the split is purely organizational (mirrors
//! `sip-core/src/call/lifecycle/`'s same multi-file-single-impl pattern;
//! cross-file inherent-method calls like `self.show_account_section(...)`
//! work regardless of which file defines the method). This file keeps only
//! the shared scaffolding: opening the pop-out window, the tab-strip/
//! dispatch logic, and a few small field-editing helpers used across more
//! than one tab.

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
use crate::theme::Palette;

/// Shared between `show_settings_modal` (which opens the viewport under
/// this id) and the background device-scan spawns in `audio.rs`/`video.rs`
/// (which need the same id to wake *this* viewport specifically, not just
/// `ROOT` -- see their own doc comments for why waking only `ROOT` left this
/// window showing stale "Scanning..." text until the user happened to
/// interact with it directly).
const SETTINGS_VIEWPORT_NAME: &str = "deelip_settings_window";

impl DeelipApp {
    /// Settings as a separate, genuinely movable native OS window rather
    /// than a tab (MicroSIP-style "phone window + separate settings window"
    /// split -- see `app.rs`'s `settings_open` doc comment). No-op when
    /// closed.
    ///
    /// This used to be an `egui::Window` (a floating panel drawn *inside*
    /// the main app's own OS window canvas) plus a hand-rolled dimming
    /// backdrop faking modality -- it looked like a separate window but was
    /// mechanically trapped inside the main window's bounds, unable to be
    /// dragged out to a different part of the screen (a real user-reported
    /// bug: "the settings window is inside the initial deelip window, and i
    /// can not move it"). `Context::show_viewport_deferred` creates an
    /// actual second native window (its own OS-level title bar, move,
    /// resize, and close -- not an egui-drawn imitation of one), which is
    /// what a "separate window" needs to mean here.
    ///
    /// `Deferred`, not `Immediate` -- this used to be `Immediate`, which
    /// renders synchronously nested inside the *main* window's own per-tick
    /// callback (confirmed against `eframe`'s own source: an `Immediate`
    /// child viewport has no redraw path of its own, it only ever repaints
    /// when its parent's tick runs). That's what made Settings feel slow
    /// whenever the main window was unfocused (which it always is while
    /// Settings itself has focus): GNOME/Mutter throttles frame delivery for
    /// whichever of the two windows isn't focused down to roughly 1Hz
    /// (confirmed live, independent of whether the windows visually
    /// overlap), and since Settings was nested inside the main window's own
    /// callback, every click/keystroke inside Settings was gated by that
    /// same ~1s throttle on the *main* window's redraw. `Deferred` viewports
    /// get their own independent redraw path (`eframe` invokes their stored
    /// callback directly whenever *their* window needs a repaint, not the
    /// main window's), so Settings now responds to its own input normally
    /// regardless of the main window's focus/throttle state. This is why
    /// `DeelipApp` is wrapped in `SharedApp` (`Arc<Mutex<_>>`) -- a
    /// `Deferred` callback must be `Fn + Send + Sync + 'static`, so it can't
    /// directly borrow `&mut self` the way the old `Immediate` closure did;
    /// it locks the shared app instead. Called every frame while
    /// `settings_open` is true, same lifecycle as before (egui's viewport
    /// model is still declarative, not create-once-and-forget).
    pub(crate) fn show_settings_modal(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        // Shared "real separate OS window" scaffolding -- see
        // `show_pop_out_window`'s own doc comment for the full rationale
        // (the `embed_viewports()` deadlock hazard, why the titlebar/
        // content/close-handling shape is common to every pop-out window
        // in this app). `SETTINGS_VIEWPORT_NAME` is passed as the `key` so
        // its hash matches what the background device-scan spawns
        // elsewhere in this module already wake via `request_repaint_of`.
        show_pop_out_window(
            self,
            ctx,
            self_app,
            SETTINGS_VIEWPORT_NAME,
            "DeeLip Settings",
            // Sized so every tab except Account (which scrolls internally
            // -- see its own `SettingsTab::Account` match arm's comment)
            // fits without scrolling at all -- confirmed live via Xvfb
            // across all 8 tabs, not guessed.
            [950.0, 740.0],
            [580.0, 520.0],
            true,
            |app| app.settings_open,
            |app| app.settings_open = false,
            |_app| "Settings".to_string(),
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
            self.config
                .accounts
                .push(deelip_config::SipAccount::default());
        }
        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
        let palette = self.palette;

        // MicroSIP-style tab strip -- one section visible at a time, sized
        // to fit without scrolling, instead of the previous single long
        // `ScrollArea` stacking all 12 sections in one column.
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.settings_tab, SettingsTab::General, "General");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Account, "Account");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Audio, "Audio");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Video, "Video");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Network, "Network");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Directory, "Directory");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Hotkeys, "Hotkeys");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Advanced, "Advanced");
        });
        ui.separator();
        ui.add_space(6.0);

        // Reserved *before* the tab content below -- `ScrollArea::vertical()`
        // (used by the Account tab) greedily fills all remaining space in
        // its parent, so a naive "content, then Save button" ordering left
        // the Save button pushed below the visible window whenever a tab's
        // content scrolled (caught live, not by reading the code: Account's
        // Save button was simply gone from the screenshot). Anchoring Save
        // to the bottom *first* means whatever's left for the tab content
        // (and therefore the Account `ScrollArea` inside it) already
        // excludes this panel's own height.
        egui::TopBottomPanel::bottom("settings_save_panel").show_inside(ui, |ui| {
            ui.add_space(8.0);
            if ui.button("Save").clicked() {
                match self.config.save(&self.db) {
                    Ok(()) => self.settings_saved_notice = true,
                    Err(err) => {
                        self.settings_saved_notice = false;
                        tracing::error!("Failed to save config: {err}");
                    }
                }
            }
            if self.settings_saved_notice {
                ui.label(
                    RichText::new("Saved — restart DeeLip to apply changes.").color(palette.signal),
                );
            }
            ui.add_space(4.0);
        });

        let edited = match self.settings_tab {
            SettingsTab::General => {
                self.show_notifications_section(ui, &palette);
                ui.add_space(14.0);
                self.show_call_handling_section(ui, &palette);
                ui.add_space(14.0);
                self.show_startup_section(ui, &palette)
            }
            // The one exception to "no scrolling" -- confirmed live (this
            // section's content still doesn't fit even at ~1400px tall,
            // an unreasonable window height) that Account is too dense to
            // ever fit a real dialog without one, even after pulling
            // several stacked label+field rows into single rows above.
            // Scrolling just this tab beats silently clipping its content,
            // which is what removing the outer `ScrollArea` entirely would
            // otherwise do.
            SettingsTab::Account => {
                let mut edited = false;
                egui::ScrollArea::vertical()
                    .id_source("account_tab_scroll")
                    .show(ui, |ui| {
                        edited = self.show_account_section(ui, &palette);
                    });
                edited
            }
            SettingsTab::Audio => self.show_audio_section(ui, &palette),
            SettingsTab::Video => self.show_video_section(ui, &palette),
            SettingsTab::Network => self.show_network_section(ui, &palette),
            SettingsTab::Directory => self.show_directory_section(ui, &palette),
            SettingsTab::Hotkeys => self.show_global_hotkeys_section(ui, &palette),
            // Same "doesn't fit, scroll just this tab" exception as Account
            // above -- confirmed live that its 4 stacked sections (Updates/
            // Blocklist/Call History/Contacts Import-Export, the latter two
            // added in a later session than the comment above was written)
            // overflow past the window's bottom, taking the Save button
            // with them.
            SettingsTab::Advanced => {
                egui::ScrollArea::vertical()
                    .id_source("advanced_tab_scroll")
                    .show(ui, |ui| {
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
            self.settings_saved_notice = false;
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
    ui: &mut Ui,
    palette: &Palette,
    value: &mut Option<String>,
    hint: &str,
    width: f32,
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
        ui.add(
            egui::TextEdit::singleline(&mut text)
                .password(true)
                .desired_width(f32::INFINITY),
        )
        .changed()
    });
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}
