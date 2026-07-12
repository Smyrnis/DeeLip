//! Comfort noise, DTMF mode, call forwarding, no-answer handling, auto
//! answer, control-button auto-answer/deny, mailbox, and presence publish.

use deelip_config::{DtmfMode, SipAccount};
use egui::Ui;

use crate::helpers::{field_label, info_hint};
use crate::strings::t;
use crate::theme::Palette;
use crate::views::settings::{optional_text_field, optional_text_field_sized};

pub(super) fn show(ui: &mut Ui, palette: &Palette, account: &mut SipAccount, edited: &mut bool) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        *edited |= ui.checkbox(&mut account.vad_enabled, t("settings.account.vad_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.vad_info"));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.dtmf_mode_label"));
        egui::ComboBox::from_id_salt("settings_dtmf_mode")
            .selected_text(match account.dtmf_mode {
                DtmfMode::Rfc2833 => t("settings.account.dtmf_rfc2833"),
                DtmfMode::SipInfo => t("settings.account.dtmf_sipinfo"),
                DtmfMode::Inband => t("settings.account.dtmf_inband"),
                DtmfMode::Auto => t("settings.account.dtmf_auto"),
            })
            .show_ui(ui, |ui| {
                *edited |= ui
                    .selectable_value(&mut account.dtmf_mode, DtmfMode::Rfc2833, t("settings.account.dtmf_rfc2833"))
                    .changed();
                *edited |= ui
                    .selectable_value(&mut account.dtmf_mode, DtmfMode::SipInfo, t("settings.account.dtmf_sipinfo"))
                    .changed();
                *edited |= ui
                    .selectable_value(&mut account.dtmf_mode, DtmfMode::Inband, t("settings.account.dtmf_inband"))
                    .changed();
                *edited |= ui
                    .selectable_value(&mut account.dtmf_mode, DtmfMode::Auto, t("settings.account.dtmf_auto"))
                    .changed();
            });
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.forward_always_label"));
        *edited |=
            optional_text_field(ui, palette, &mut account.forward_always, &t("settings.account.forward_always_hint"));
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.forward_busy_label"));
        *edited |=
            optional_text_field(ui, palette, &mut account.forward_on_busy, &t("settings.account.voicemail_uri_hint"));
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.forward_unanswered_label"));
        *edited |= optional_text_field_sized(
            ui,
            palette,
            &mut account.no_answer_forward,
            &t("settings.account.voicemail_uri_hint"),
            180.0,
        );
        field_label(ui, palette, &t("settings.account.after_seconds_short_label"));
        *edited |= ui.add(egui::DragValue::new(&mut account.no_answer_timeout_secs).range(1..=300)).changed();
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        *edited |= ui.checkbox(&mut account.auto_answer_enabled, t("settings.account.auto_answer_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.auto_answer_info"));
    });
    if account.auto_answer_enabled {
        ui.horizontal(|ui| {
            field_label(ui, palette, &t("settings.account.after_seconds_label"));
            *edited |= ui.add(egui::DragValue::new(&mut account.auto_answer_secs).range(0..=60)).changed();
        });
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        *edited |= ui
            .checkbox(&mut account.auto_answer_control_button, t("settings.account.auto_answer_control_checkbox"))
            .changed();
        info_hint(ui, palette, &t("settings.account.auto_answer_control_info"));
    });
    ui.horizontal(|ui| {
        *edited |= ui
            .checkbox(&mut account.deny_incoming_control_button, t("settings.account.deny_incoming_checkbox"))
            .changed();
        info_hint(ui, palette, &t("settings.account.deny_incoming_info"));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.mailbox_label"));
        *edited |=
            optional_text_field_sized(ui, palette, &mut account.mailbox, &t("settings.account.mailbox_hint"), 100.0);
        info_hint(ui, palette, &t("settings.account.mailbox_info"));
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        *edited |=
            ui.checkbox(&mut account.publish_presence, t("settings.account.publish_presence_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.publish_presence_info"));
    });
}
