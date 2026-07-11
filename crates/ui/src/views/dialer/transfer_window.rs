use egui::{RichText, Ui};

use crate::app::{DeelipApp, SharedApp};
use crate::helpers::{phone_keypad, show_pop_out_window, text_edit_scope};
use crate::theme::{self, Palette};

impl DeelipApp {
    /// Transfer/Attended as a real separate OS window, same `Deferred`-
    /// viewport pattern as `show_messages_window` (see its doc comment for
    /// why the `embed_viewports()` fallback branch has to run inline
    /// rather than through the deferred closure, and why `self_app.lock()`
    /// must be called as a method, not `self_app.0.lock()`, to keep the
    /// `unsafe impl Send` sound). One shared window covers both blind and
    /// attended transfer via a mode switch, rather than two near-identical
    /// windows -- they're one workflow, not two unrelated features.
    /// `do_transfer`/`do_attended_transfer_dial` already flip
    /// `showing_transfer`/`showing_attended` back to `false` on success
    /// (see their own doc comments), which is this window's open
    /// condition -- so firing either one closes this window as a side
    /// effect, no separate "close" bookkeeping needed for the happy path.
    pub(crate) fn show_transfer_window(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        show_pop_out_window(
            self,
            ctx,
            self_app,
            "deelip_transfer_window",
            "DeeLip Transfer Call",
            [320.0, 540.0],
            [280.0, 420.0],
            false,
            |app| app.showing_transfer || app.showing_attended,
            |app| {
                app.showing_transfer = false;
                app.showing_attended = false;
            },
            |_app| "Transfer Call".to_string(),
            |app, ui| app.show_transfer_window_content(ui),
        );
    }

    fn show_transfer_window_content(&mut self, ui: &mut Ui) {
        // `ScrollArea`, same reasoning as `show_dialer_in_call`'s -- the
        // mode switch + separator + field + full keypad + backspace/clear
        // + action button is taller than this window's own default size at
        // some window heights (confirmed live), so this is a no-op safety
        // net once it fits, not a design choice to always scroll.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| self.show_transfer_window_content_inner(ui));
    }

    fn show_transfer_window_content_inner(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.selectable_label(!self.showing_attended, "Blind Transfer").clicked() {
                self.showing_transfer = true;
                self.showing_attended = false;
            }
            if ui
                .add_enabled(
                    self.calls.len() == 1,
                    egui::SelectableLabel::new(self.showing_attended, "Attended Transfer"),
                )
                .clicked()
            {
                self.showing_transfer = false;
                self.showing_attended = true;
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        if self.showing_attended {
            transfer_target_editor(ui, palette, &mut self.attended_target);
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui.button(format!("{}  Call", egui_phosphor::regular::PHONE)).clicked() {
                    self.do_attended_transfer_dial();
                }
            });
        } else {
            transfer_target_editor(ui, palette, &mut self.transfer_target);
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui
                    .button(format!("{}  Send", egui_phosphor::regular::EXPORT))
                    .clicked()
                {
                    self.do_transfer();
                }
            });
        }
    }
}

/// Number entry for the Transfer/Attended sub-panels -- a text field (still
/// directly editable, for a full `sip:user@host` target) plus the same
/// on-screen dial pad the idle Dialer and in-call DTMF panel use, so picking
/// a transfer target doesn't require a physical keyboard (matches MicroSIP's
/// own transfer keypad). Shared by both panels since they're otherwise
/// identical field+keypad+backspace/clear blocks, just feeding a different
/// `String` and followed by a different action button.
fn transfer_target_editor(ui: &mut Ui, palette: Palette, target: &mut String) {
    text_edit_scope(ui, &palette, |ui| {
        ui.add(
            egui::TextEdit::singleline(target)
                .hint_text(RichText::new("e.g. 1234567").color(palette.ink_muted))
                .font(theme::font_address())
                .desired_width(f32::INFINITY),
        )
    });
    ui.add_space(6.0);
    phone_keypad(ui, palette, |digit| target.push(digit));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Plain Unicode, not `egui_phosphor::regular::BACKSPACE` -- see the
        // crate-level note on the broken icon set in `theme.rs`.
        if ui.add_enabled(!target.is_empty(), egui::Button::new("⌫")).clicked() {
            target.pop();
        }
        if ui.add_enabled(!target.is_empty(), egui::Button::new("Clear")).clicked() {
            target.clear();
        }
    });
}
