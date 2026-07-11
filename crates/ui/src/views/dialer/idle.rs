use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{account_status_label, ctx_key_enter, phone_keypad, text_edit_scope};
use crate::strings::t;
use crate::theme;

/// Width the idle dial pad and the in-call "stage" are capped to and
/// centered within -- the address field and keypad are the whole point of
/// the idle screen, so they read as one small, deliberate instrument
/// instead of stretching edge-to-edge in a resized window.
const STAGE_WIDTH: f32 = 320.0;

impl DeelipApp {
    // ── Idle: number entry + keypad ───────────────────────────────────────

    pub(super) fn show_dialer_idle(&mut self, ui: &mut Ui) {
        if self.accounts.len() > 1 {
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("dialer.call_from")).color(self.palette.ink_muted).small());
                let current = self.selected_account_idx().unwrap_or(0);
                let palette = self.palette;
                let selected_label = {
                    let acc = &self.accounts[current];
                    account_status_label(ui, &palette, acc.reg_ok, &acc.label)
                };
                egui::ComboBox::from_id_source("dialer_account_picker").selected_text(selected_label).show_ui(
                    ui,
                    |ui| {
                        for i in 0..self.accounts.len() {
                            let acc = &self.accounts[i];
                            let label = account_status_label(ui, &palette, acc.reg_ok, &acc.label);
                            if ui.add(egui::SelectableLabel::new(current == i, label)).clicked() {
                                self.selected_account = i;
                                self.refresh_idle_status();
                            }
                        }
                    },
                );
            });
            ui.add_space(6.0);
        }

        // A centered fixed-width column, not `ui.vertical_centered` -- see
        // docs/crates/ui.md's "centering nested rows" note for why that alone
        // doesn't center the keypad/backspace rows inside it.
        let margin = ((ui.available_width() - STAGE_WIDTH) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(margin);
            ui.vertical(|ui| {
                ui.set_width(STAGE_WIDTH);

                let palette = self.palette;
                let resp = text_edit_scope(ui, &palette, |ui| {
                    ui.add_sized(
                        [STAGE_WIDTH, 48.0],
                        egui::TextEdit::singleline(&mut self.call_target)
                            .hint_text(RichText::new(t("dialer.enter_a_number")).color(palette.ink_muted))
                            .font(egui::FontId::new(19.0, egui::FontFamily::Monospace))
                            .horizontal_align(egui::Align::Center),
                    )
                });
                if resp.lost_focus() && ctx_key_enter(ui) {
                    self.do_call(None);
                }
                ui.add_space(18.0);

                let palette = self.palette;
                phone_keypad(ui, palette, |digit| self.call_target.push(digit));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let row_width = 64.0 + ui.spacing().item_spacing.x + 60.0;
                    ui.add_space(((STAGE_WIDTH - row_width) / 2.0).max(0.0));
                    // Plain Unicode, not `egui_phosphor::regular::BACKSPACE` --
                    // see the crate-level note on the broken icon set in
                    // `theme.rs`.
                    if ui.add_enabled(!self.call_target.is_empty(), egui::Button::new("⌫")).clicked() {
                        self.call_target.pop();
                    }
                    if ui
                        .add_enabled(!self.call_target.is_empty(), egui::Button::new(t("common.clear_button")))
                        .clicked()
                    {
                        self.call_target.clear();
                    }
                });
                ui.add_space(16.0);

                // Grey chrome, not `signal` -- see docs/crates/ui.md's Theming section.
                let call_text =
                    RichText::new(format!("{}  {}", egui_phosphor::regular::PHONE, t("common.call_button")))
                        .font(theme::font_medium(15.0))
                        .color(self.palette.ink);
                if ui
                    .add_sized(
                        [STAGE_WIDTH, 42.0],
                        egui::Button::new(call_text)
                            .fill(self.palette.surface_hover)
                            .stroke(egui::Stroke::new(1.0, self.palette.border))
                            .rounding(egui::Rounding::same(2.0)),
                    )
                    .clicked()
                {
                    self.do_call(None);
                }

                let can_redial = self.reg_ok && self.last_dialed.is_some();
                if can_redial {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        let redial_text = RichText::new(format!(
                            "{}  {}",
                            egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE,
                            t("dialer.redial_button")
                        ))
                        .color(self.palette.ink_muted)
                        .small();
                        if ui.add(egui::Button::new(redial_text).frame(false)).clicked() {
                            self.do_redial();
                        }
                    });
                }
            });
        });
    }
}
