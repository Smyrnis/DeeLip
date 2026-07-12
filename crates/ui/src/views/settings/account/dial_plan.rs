//! Dialing prefix and the regex dial-plan rule table, including its
//! add-rule input row.

use deelip_config::{DialPlanRule, SipAccount};
use egui::{RichText, Ui};

use crate::helpers::{empty_state, field_label, info_hint, text_edit_scope};
use crate::strings::t;
use crate::theme::Palette;
use crate::views::settings::optional_text_field_sized;

#[allow(clippy::too_many_arguments)]
pub(super) fn show(
    ui: &mut Ui, palette: &Palette, account: &mut SipAccount, edited: &mut bool, pattern_input: &mut String,
    replacement_input: &mut String,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.dialing_prefix_label"));
        *edited |= optional_text_field_sized(
            ui,
            palette,
            &mut account.dialing_prefix,
            &t("settings.account.dialing_prefix_hint"),
            60.0,
        );
        info_hint(ui, palette, &t("settings.account.dialing_prefix_info"));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.dial_plan_label"));
        info_hint(ui, palette, &t("settings.account.dial_plan_info"));
    });
    if account.dial_plan.is_empty() {
        empty_state(ui, palette, &t("settings.account.dial_plan_empty"));
    } else {
        let mut remove_idx = None;
        for (i, rule) in account.dial_plan.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                *edited |= ui.checkbox(&mut rule.enabled, "").changed();
                *edited |= text_edit_scope(ui, palette, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.pattern)
                            .hint_text(RichText::new(t("settings.account.pattern_hint")).color(palette.ink_muted))
                            .desired_width(120.0),
                    )
                    .changed()
                });
                field_label(ui, palette, "→");
                *edited |= text_edit_scope(ui, palette, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.replacement)
                            .hint_text(RichText::new(t("settings.account.replacement_hint")).color(palette.ink_muted))
                            .desired_width(100.0),
                    )
                    .changed()
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(t("common.remove_button")).clicked() {
                        remove_idx = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove_idx {
            account.dial_plan.remove(i);
            *edited = true;
        }
    }
    ui.horizontal(|ui| {
        text_edit_scope(ui, palette, |ui| {
            ui.add(
                egui::TextEdit::singleline(pattern_input)
                    .hint_text(RichText::new(t("settings.account.pattern_example_hint")).color(palette.ink_muted))
                    .desired_width(120.0),
            )
        });
        field_label(ui, palette, "→");
        text_edit_scope(ui, palette, |ui| {
            ui.add(
                egui::TextEdit::singleline(replacement_input)
                    .hint_text(RichText::new(t("settings.account.replacement_example_hint")).color(palette.ink_muted))
                    .desired_width(100.0),
            )
        });
        if ui.button(t("settings.account.add_rule_button")).clicked() && !pattern_input.trim().is_empty() {
            account.dial_plan.push(DialPlanRule {
                pattern: pattern_input.trim().to_string(),
                replacement: replacement_input.trim().to_string(),
                enabled: true,
            });
            pattern_input.clear();
            replacement_input.clear();
            *edited = true;
        }
    });
}
