//! Codec enable/disable/reorder lists, plus "Force Codec for Incoming".

use deelip_config::SipAccount;
use egui::{RichText, Ui};

use crate::helpers::{codec_label, field_label, info_hint};
use crate::strings::t;
use crate::theme::Palette;

pub(super) fn show(ui: &mut Ui, palette: &Palette, account: &mut SipAccount, edited: &mut bool) {
    ui.add_space(6.0);
    field_label(ui, palette, &t("settings.account.codecs_label"));
    let mut to_enable: Option<&str> = None;
    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    let mut to_disable: Option<usize> = None;
    let list_frame = egui::Frame::none()
        .stroke(egui::Stroke::new(1.0, palette.border))
        .inner_margin(egui::Margin::symmetric(8.0, 6.0));
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(t("settings.account.codecs_available_label")).color(palette.ink_muted).small());
            list_frame.show(ui, |ui| {
                // `set_width`, not just `set_min_size` -- a bare
                // minimum lets the Enabled column's nested
                // `right_to_left` layout claim the rest of the panel.
                ui.set_width(150.0);
                ui.set_min_height(120.0);
                for name in ["opus", "g722", "pcmu", "pcma", "gsm", "ilbc", "g729"] {
                    if account.codec_order.iter().any(|c| c == name) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button(egui_phosphor::regular::ARROW_RIGHT).clicked() {
                            to_enable = Some(name);
                        }
                        ui.label(codec_label(name));
                    });
                }
            });
        });
        ui.vertical(|ui| {
            ui.label(RichText::new(t("settings.account.codecs_enabled_label")).color(palette.ink_muted).small());
            list_frame.show(ui, |ui| {
                // Fixed width, same reasoning as the Available
                // column above.
                ui.set_width(290.0);
                ui.set_min_height(120.0);
                for (i, name) in account.codec_order.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let can_disable = account.codec_order.len() > 1;
                        if ui
                            .add_enabled(can_disable, egui::Button::new(egui_phosphor::regular::ARROW_LEFT).small())
                            .clicked()
                        {
                            to_disable = Some(i);
                        }
                        ui.label(codec_label(name));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(i + 1 < account.codec_order.len(), egui::Button::new("↓").small())
                                .clicked()
                            {
                                move_down = Some(i);
                            }
                            if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                                move_up = Some(i);
                            }
                        });
                    });
                }
            });
        });
    });
    if let Some(name) = to_enable {
        account.codec_order.push(name.to_string());
        *edited = true;
    }
    if let Some(i) = move_up {
        account.codec_order.swap(i, i - 1);
        *edited = true;
    }
    if let Some(i) = move_down {
        account.codec_order.swap(i, i + 1);
        *edited = true;
    }
    if let Some(i) = to_disable {
        account.codec_order.remove(i);
        *edited = true;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.force_incoming_codec_label"));
        let no_override = t("settings.account.no_override_option");
        let selected_label = account.force_incoming_codec.as_deref().map(codec_label).unwrap_or(no_override.as_str());
        egui::ComboBox::from_id_source("settings_force_incoming_codec").selected_text(selected_label).show_ui(
            ui,
            |ui| {
                if ui
                    .selectable_label(account.force_incoming_codec.is_none(), t("settings.account.no_override_option"))
                    .clicked()
                {
                    account.force_incoming_codec = None;
                    *edited = true;
                }
                for name in &account.codec_order {
                    if ui
                        .selectable_label(
                            account.force_incoming_codec.as_deref() == Some(name.as_str()),
                            codec_label(name),
                        )
                        .clicked()
                    {
                        account.force_incoming_codec = Some(name.clone());
                        *edited = true;
                    }
                }
            },
        );
        info_hint(ui, palette, &t("settings.account.force_incoming_codec_info"));
    });
}
