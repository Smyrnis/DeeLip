use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint};
use crate::strings::t;
use crate::theme::{self, Palette};

use super::{optional_password_field, optional_text_field, optional_text_field_sized};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_directory_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("settings.directory.section_title")).font(theme::font_heading(13.5)));
            info_hint(ui, palette, &t("settings.directory.section_hint"));
        });
        theme::full_width_card(ui, *palette, |ui| {
            egui::Grid::new("settings_ldap_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    field_label(ui, palette, &t("settings.account.server_label"));
                    edited |= optional_text_field(ui, palette, &mut self.config.ldap_server, &t("settings.directory.server_hint"));
                    ui.end_row();

                    field_label(ui, palette, &t("settings.account.port_label"));
                    edited |= ui.add(egui::DragValue::new(&mut self.config.ldap_port)).changed();
                    ui.end_row();

                    field_label(ui, palette, &t("settings.directory.base_dn_label"));
                    edited |= optional_text_field(ui, palette, &mut self.config.ldap_base_dn, &t("settings.directory.base_dn_hint"));
                    ui.end_row();

                    field_label(ui, palette, &t("settings.directory.bind_dn_label"));
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.ldap_bind_dn, &t("settings.directory.bind_dn_hint"), 240.0);
                        info_hint(ui, palette, &t("settings.directory.bind_dn_info"));
                    });
                    ui.end_row();

                    field_label(ui, palette, &t("settings.directory.bind_password_label"));
                    edited |= optional_password_field(ui, palette, &mut self.config.ldap_bind_password);
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.ldap_use_tls, t("settings.directory.use_tls_checkbox")).changed();
                info_hint(ui, palette, &t("settings.directory.use_tls_hint"));
            });
            ui.add_space(4.0);
            field_label(ui, palette, &t("settings.directory.search_filter_label"));
            ui.horizontal(|ui| {
                edited |= optional_text_field_sized(ui, palette, &mut self.config.ldap_search_filter, "(|(cn=*{query}*)(mail=*{query}*))", 240.0);
                info_hint(ui, palette, &t("settings.directory.search_filter_info"));
            });
        });
        edited
    }
}
