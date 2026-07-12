//! Account identity fields: enabled/DND/local-account toggles, account
//! name/username/password/login/server/port/domain/proxy/display name,
//! transport, and TLS verify-skip.
//!
//! A free function taking `&mut SipAccount` directly, not an `impl
//! DeelipApp` method like this crate's other multi-file splits -- `account`
//! is borrowed from `self.config.accounts[idx]` for the whole body of
//! `show_account_section`'s card closure, and a `self.method(...)` call
//! would need to re-borrow all of `self` while that borrow is still live.
//! Passing the pieces each section needs as explicit parameters sidesteps
//! that entirely.

use deelip_config::{SipAccount, TransportProtocol};
use egui::{RichText, Ui};

use crate::helpers::{empty_state, field_label, info_hint, text_edit_scope};
use crate::strings::t;
use crate::theme::{self, Palette};
use crate::views::settings::{optional_text_field, optional_text_field_sized};

pub(super) fn show(
    ui: &mut Ui, palette: &Palette, account: &mut SipAccount, edited: &mut bool, show_password: &mut bool,
) {
    *edited |= ui.checkbox(&mut account.enabled, t("settings.account.enabled_checkbox")).changed();
    *edited |= ui.checkbox(&mut account.dnd, t("settings.account.dnd_checkbox")).changed();
    ui.horizontal(|ui| {
        *edited |= ui.checkbox(&mut account.local_account, t("settings.account.local_account_checkbox")).changed();
        info_hint(ui, palette, &t("settings.account.local_account_hint"));
    });
    if account.local_account {
        empty_state(ui, palette, &t("settings.account.local_account_empty_state"));
    }
    ui.add_space(4.0);

    egui::Grid::new("settings_account_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        field_label(ui, palette, &t("settings.account.account_name_label"));
        *edited |=
            optional_text_field(ui, palette, &mut account.account_name, &t("settings.account.account_name_hint"));
        ui.end_row();

        field_label(ui, palette, &t("settings.account.username_label"));
        *edited |= text_edit_scope(ui, palette, |ui| {
            ui.add(egui::TextEdit::singleline(&mut account.username).desired_width(f32::INFINITY)).changed()
        });
        ui.end_row();

        field_label(ui, palette, &t("settings.account.password_label"));
        ui.horizontal(|ui| {
            *edited |= text_edit_scope(ui, palette, |ui| {
                ui.add(egui::TextEdit::singleline(&mut account.password).password(!*show_password).desired_width(200.0))
                    .changed()
            });
            let icon = if *show_password { egui_phosphor::regular::EYE_SLASH } else { egui_phosphor::regular::EYE };
            if ui.small_button(icon).clicked() {
                *show_password = !*show_password;
            }
        });
        ui.end_row();

        field_label(ui, palette, &t("settings.account.login_label"));
        ui.horizontal(|ui| {
            *edited |= optional_text_field_sized(
                ui,
                palette,
                &mut account.auth_username,
                &t("settings.account.login_hint"),
                240.0,
            );
            info_hint(ui, palette, &t("settings.account.login_info"));
        });
        ui.end_row();

        field_label(ui, palette, &t("settings.account.server_label"));
        *edited |= text_edit_scope(ui, palette, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut account.server)
                    .font(theme::font_address())
                    .desired_width(f32::INFINITY),
            )
            .changed()
        });
        ui.end_row();

        field_label(ui, palette, &t("settings.account.port_label"));
        *edited |= ui.add(egui::DragValue::new(&mut account.port)).changed();
        ui.end_row();

        field_label(ui, palette, &t("settings.account.domain_label"));
        ui.horizontal(|ui| {
            *edited |=
                optional_text_field_sized(ui, palette, &mut account.domain, &t("settings.account.domain_hint"), 240.0);
            info_hint(ui, palette, &t("settings.account.domain_info"));
        });
        ui.end_row();

        field_label(ui, palette, &t("settings.account.sip_proxy_label"));
        ui.horizontal(|ui| {
            *edited |= optional_text_field_sized(
                ui,
                palette,
                &mut account.sip_proxy,
                &t("settings.account.host_port_hint"),
                240.0,
            );
            info_hint(ui, palette, &t("settings.account.sip_proxy_info"));
        });
        ui.end_row();

        field_label(ui, palette, &t("settings.account.display_name_label"));
        *edited |= optional_text_field(ui, palette, &mut account.display_name, "");
        ui.end_row();

        field_label(ui, palette, &t("settings.account.transport_label"));
        egui::ComboBox::from_id_source("settings_transport")
            .selected_text(match account.transport {
                TransportProtocol::Udp => "UDP",
                TransportProtocol::Tcp => "TCP",
                TransportProtocol::Tls => "TLS",
                TransportProtocol::Auto => "Auto",
            })
            .show_ui(ui, |ui| {
                *edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Udp, "UDP").changed();
                *edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tcp, "TCP").changed();
                *edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tls, "TLS").changed();
                *edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Auto, "Auto").changed();
            });
        if account.transport == TransportProtocol::Auto {
            info_hint(ui, palette, &t("settings.account.transport_auto_info"));
        }
        ui.end_row();
    });

    if matches!(account.transport, TransportProtocol::Tls | TransportProtocol::Auto) {
        *edited |= ui
            .checkbox(&mut account.tls_insecure_skip_verify, t("settings.account.tls_skip_verify_checkbox"))
            .changed();
        if account.tls_insecure_skip_verify {
            ui.label(RichText::new(t("settings.account.tls_warning")).color(palette.ringing));
        }
    }
}
