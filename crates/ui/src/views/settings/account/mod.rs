//! The account editor: header (add/remove/pick account) stays here since it
//! runs before any per-account borrow is taken; the account-editing card's
//! sections split into `identity.rs`/`codecs.rs`/`call_handling.rs`/
//! `dial_plan.rs`/`network.rs` -- see `identity.rs`'s doc comment for why
//! those are free functions taking `&mut SipAccount` rather than this
//! crate's usual `impl DeelipApp`-per-file split (same precedent otherwise:
//! `views/dialer/`, `sip-core/src/call/lifecycle/`).

mod call_handling;
mod codecs;
mod dial_plan;
mod identity;
mod network;

use deelip_config::SipAccount;
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{account_label, account_status_label, info_hint};
use crate::strings::t;
use crate::theme::{self, Palette};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_account_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new(t("settings.account.accounts_heading")).font(theme::font_heading(13.5)));
            info_hint(ui, palette, &t("settings.account.accounts_hint"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_remove = self.config.accounts.len() > 1;
                if ui.add_enabled(can_remove, egui::Button::new(t("common.remove_button"))).clicked() {
                    self.config.accounts.remove(self.settings_ui.edit_account_idx);
                    self.settings_ui.edit_account_idx =
                        self.settings_ui.edit_account_idx.min(self.config.accounts.len() - 1);
                    edited = true;
                }
                if ui.button(format!("+ {}", t("settings.account.add_account_button"))).clicked() {
                    self.config.accounts.push(SipAccount::default());
                    self.settings_ui.edit_account_idx = self.config.accounts.len() - 1;
                    edited = true;
                }
            });
        });
        ui.add_space(4.0);
        // A draft account only has a live registration-status dot once
        // it matches a currently-running identity by username -- a
        // freshly-added or just-edited entry has no such match yet
        // (accurately reads as "not registered" until Save + restart).
        let is_registered = |acc: &SipAccount| {
            self.accounts_state.accounts.iter().any(|a| a.account.username == acc.username && a.reg_ok)
        };
        let selected_text = account_status_label(
            ui,
            palette,
            is_registered(&self.config.accounts[self.settings_ui.edit_account_idx]),
            &format!(
                "{}. {}",
                self.settings_ui.edit_account_idx + 1,
                account_label(&self.config.accounts[self.settings_ui.edit_account_idx])
            ),
        );
        egui::ComboBox::from_id_salt("settings_account_picker").selected_text(selected_text).show_ui(ui, |ui| {
            for i in 0..self.config.accounts.len() {
                let label_text = format!("{}. {}", i + 1, account_label(&self.config.accounts[i]));
                let label = account_status_label(ui, palette, is_registered(&self.config.accounts[i]), &label_text);
                if ui.add(egui::Button::selectable(self.settings_ui.edit_account_idx == i, label)).clicked() {
                    self.settings_ui.edit_account_idx = i;
                }
            }
        });
        ui.add_space(6.0);

        theme::full_width_card(ui, *palette, |ui| {
            // Read before `account` borrows `self.config.accounts` below --
            // see `network.rs`'s doc comment.
            let global_ice_enabled = self.config.ice_enabled;
            let account = &mut self.config.accounts[self.settings_ui.edit_account_idx];

            identity::show(ui, palette, account, &mut edited, &mut self.settings_ui.show_account_password);
            codecs::show(ui, palette, account, &mut edited);
            call_handling::show(ui, palette, account, &mut edited);
            dial_plan::show(
                ui,
                palette,
                account,
                &mut edited,
                &mut self.settings_ui.dialplan_pattern_input,
                &mut self.settings_ui.dialplan_replacement_input,
            );
            network::show(ui, palette, account, &mut edited, global_ice_enabled);
        });

        if !self.config.accounts.iter().any(|a| a.enabled) {
            ui.label(RichText::new(t("settings.account.no_accounts_warning")).color(palette.ringing));
        }

        edited
    }
}
