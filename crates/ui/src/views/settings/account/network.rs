//! Caller-ID hiding, REGISTER refresh, Session Timers, NAT keepalive, media
//! encryption, video, public-address override, IP rewrite, and per-account
//! ICE override.

use deelip_config::{MediaEncryption, SipAccount};
use egui::Ui;

use crate::helpers::{field_label, info_hint};
use crate::strings::t;
use crate::theme::Palette;
use crate::views::settings::optional_text_field_sized;

/// `global_ice_enabled` is `self.config.ice_enabled` read by the caller
/// before `account` was borrowed -- see this module's parent doc comment
/// for why these sections take plain parameters instead of `&mut self`.
pub(super) fn show(
    ui: &mut Ui, palette: &Palette, account: &mut SipAccount, edited: &mut bool, global_ice_enabled: bool,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        *edited |= ui.checkbox(&mut account.hide_caller_id, t("settings.account.hide_caller_id_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.hide_caller_id_info"));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.register_refresh_label"));
        *edited |= ui.add(egui::DragValue::new(&mut account.register_expires).range(60..=86400)).changed();
        info_hint(ui, palette, &t("settings.account.register_refresh_info"));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let mut session_timers_on = account.session_timers_enabled;
        if ui.checkbox(&mut session_timers_on, t("settings.account.session_timers_checkbox")).changed() {
            account.session_timers_enabled = session_timers_on;
            *edited = true;
        }
        info_hint(ui, palette, &t("settings.account.session_timers_info"));
    });

    ui.add_space(6.0);
    let mut keepalive_on = account.keepalive_secs.is_some();
    if ui.checkbox(&mut keepalive_on, t("settings.account.nat_keepalive_checkbox")).changed() {
        account.keepalive_secs = if keepalive_on { Some(15) } else { None };
        *edited = true;
    }
    if let Some(secs) = &mut account.keepalive_secs {
        ui.horizontal(|ui| {
            field_label(ui, palette, &t("settings.account.every_seconds_label"));
            *edited |= ui.add(egui::DragValue::new(secs).range(5..=300)).changed();
            info_hint(ui, palette, &t("settings.account.nat_keepalive_info"));
        });
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, palette, &t("settings.account.media_encryption_label"));
        egui::ComboBox::from_id_source("settings_media_encryption")
            .selected_text(match account.media_encryption {
                MediaEncryption::MatchTransport => t("settings.account.enc_match_transport"),
                MediaEncryption::Disabled => t("settings.account.enc_disabled"),
                MediaEncryption::Enabled => t("settings.account.enc_always_srtp"),
                MediaEncryption::Zrtp => t("settings.account.enc_zrtp"),
            })
            .show_ui(ui, |ui| {
                *edited |= ui
                    .selectable_value(
                        &mut account.media_encryption,
                        MediaEncryption::MatchTransport,
                        t("settings.account.enc_match_transport"),
                    )
                    .changed();
                *edited |= ui
                    .selectable_value(
                        &mut account.media_encryption,
                        MediaEncryption::Disabled,
                        t("settings.account.enc_disabled"),
                    )
                    .changed();
                *edited |= ui
                    .selectable_value(
                        &mut account.media_encryption,
                        MediaEncryption::Enabled,
                        t("settings.account.enc_always_srtp"),
                    )
                    .changed();
                *edited |= ui
                    .selectable_value(
                        &mut account.media_encryption,
                        MediaEncryption::Zrtp,
                        t("settings.account.enc_zrtp"),
                    )
                    .changed();
            });
    });
    info_hint(ui, palette, &t("settings.account.media_encryption_info"));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        *edited |= ui.checkbox(&mut account.video_enabled, t("settings.account.video_enabled_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.video_enabled_info"));
    });

    ui.add_space(6.0);
    field_label(ui, palette, &t("settings.account.public_address_label"));
    ui.horizontal(|ui| {
        *edited |= optional_text_field_sized(
            ui,
            palette,
            &mut account.public_address,
            &t("settings.account.public_address_hint"),
            240.0,
        );
        info_hint(ui, palette, &t("settings.account.public_address_info"));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        *edited |=
            ui.checkbox(&mut account.allow_ip_rewrite, t("settings.account.allow_ip_rewrite_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.allow_ip_rewrite_info"));
    });

    ui.add_space(6.0);
    let mut ice_override_on = account.ice_enabled.is_some();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut ice_override_on, t("settings.account.ice_override_checkbox")).changed() {
            account.ice_enabled = if ice_override_on { Some(global_ice_enabled) } else { None };
            *edited = true;
        }
        info_hint(ui, palette, &t("settings.account.ice_override_info"));
    });
    if let Some(ice_on) = &mut account.ice_enabled {
        *edited |= ui.checkbox(ice_on, t("settings.account.use_ice_checkbox")).changed();
    }
}
