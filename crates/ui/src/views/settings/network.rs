use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint};
use crate::strings::t;
use crate::theme::{self, Palette};

use super::{optional_password_field, optional_text_field, optional_text_field_sized};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_network_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new(t("settings.tab_network")).font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            egui::Grid::new("settings_network_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    field_label(ui, palette, &t("settings.network.local_sip_port_label"));
                    ui.horizontal(|ui| {
                        edited |= ui.add(egui::DragValue::new(&mut self.config.local_sip_port)).changed();
                        info_hint(ui, palette, &t("settings.network.local_sip_port_hint"));
                    });
                    ui.end_row();

                    field_label(ui, palette, &t("settings.network.stun_server_label"));
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.stun_server, &t("settings.network.stun_server_hint"), 240.0);
                        info_hint(ui, palette, &t("settings.network.stun_server_info"));
                    });
                    ui.end_row();

                    field_label(ui, palette, &t("settings.network.turn_server_label"));
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.turn_server, &t("settings.network.turn_server_hint"), 240.0);
                        info_hint(ui, palette, &t("settings.network.turn_server_info"));
                    });
                    ui.end_row();

                    field_label(ui, palette, &t("settings.network.turn_username_label"));
                    edited |= optional_text_field(ui, palette, &mut self.config.turn_username, "");
                    ui.end_row();

                    field_label(ui, palette, &t("settings.network.turn_password_label"));
                    edited |= optional_password_field(ui, palette, &mut self.config.turn_password);
                    ui.end_row();

                    field_label(ui, palette, &t("settings.network.custom_nameserver_label"));
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.custom_nameserver, &t("settings.network.custom_nameserver_hint"), 240.0);
                        info_hint(ui, palette, &t("settings.network.custom_nameserver_info"));
                    });
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.dns_srv_enabled,
                    t("settings.network.dns_srv_checkbox")
                ).changed();
                info_hint(ui, palette, &t("settings.network.dns_srv_hint"));
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.ice_enabled,
                    t("settings.network.ice_checkbox")
                ).changed();
                info_hint(ui, palette, &t("settings.network.ice_hint"));
            });
            ui.add_space(6.0);
            let mut use_rtp_range = self.config.rtp_port_min.is_some() || self.config.rtp_port_max.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut use_rtp_range, t("settings.network.restrict_rtp_checkbox")).changed() {
                    if use_rtp_range {
                        self.config.rtp_port_min.get_or_insert(10000);
                        self.config.rtp_port_max.get_or_insert(20000);
                    } else {
                        self.config.rtp_port_min = None;
                        self.config.rtp_port_max = None;
                    }
                    edited = true;
                }
                info_hint(ui, palette, &t("settings.network.restrict_rtp_hint"));
            });
            if use_rtp_range {
                let mut min = self.config.rtp_port_min.unwrap_or(10000);
                let mut max = self.config.rtp_port_max.unwrap_or(20000);
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.network.min_label"));
                    edited |= ui.add(egui::DragValue::new(&mut min).range(1..=65534)).changed();
                    field_label(ui, palette, &t("settings.network.max_label"));
                    edited |= ui.add(egui::DragValue::new(&mut max).range(1..=65535)).changed();
                });
                self.config.rtp_port_min = Some(min);
                self.config.rtp_port_max = Some(max.max(min));
            }
        });
        edited
    }
}
