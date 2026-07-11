use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint};
use crate::theme::{self, Palette};

use super::{optional_password_field, optional_text_field, optional_text_field_sized};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_directory_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Directory (LDAP)").font(theme::font_heading(13.5)));
            info_hint(ui, palette, "Corporate/LDAP directory lookup, shown in the Directory tab \
                -- read-only search, never writes back to the directory. Restart required to apply.");
        });
        theme::full_width_card(ui, *palette, |ui| {
            egui::Grid::new("settings_ldap_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    field_label(ui, palette, "Server:");
                    edited |= optional_text_field(ui, palette, &mut self.config.ldap_server, "e.g. ldap.example.com");
                    ui.end_row();

                    field_label(ui, palette, "Port:");
                    edited |= ui.add(egui::DragValue::new(&mut self.config.ldap_port)).changed();
                    ui.end_row();

                    field_label(ui, palette, "Base DN:");
                    edited |= optional_text_field(ui, palette, &mut self.config.ldap_base_dn, "e.g. dc=example,dc=com");
                    ui.end_row();

                    field_label(ui, palette, "Bind DN (optional):");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.ldap_bind_dn, "e.g. cn=readonly,dc=example,dc=com", 240.0);
                        info_hint(ui, palette, "Leave blank for an anonymous bind, if the \
                            directory allows unauthenticated search.");
                    });
                    ui.end_row();

                    field_label(ui, palette, "Bind password:");
                    edited |= optional_password_field(ui, palette, &mut self.config.ldap_bind_password);
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.ldap_use_tls, "Use TLS (ldaps://)").changed();
                info_hint(ui, palette, "Connect via implicit TLS instead of plain ldap://.");
            });
            ui.add_space(4.0);
            field_label(ui, palette, "Search filter template (optional):");
            ui.horizontal(|ui| {
                edited |= optional_text_field_sized(ui, palette, &mut self.config.ldap_search_filter, "(|(cn=*{query}*)(mail=*{query}*))", 240.0);
                info_hint(ui, palette, "\"{query}\" is replaced with the (escaped) search text. \
                    Empty: falls back to a built-in filter matching cn/displayName/mail/sn/givenName.");
            });
        });
        edited
    }
}
