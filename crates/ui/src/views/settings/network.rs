use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{field_label, info_hint};
use crate::theme::{self, Palette};

use super::{optional_password_field, optional_text_field, optional_text_field_sized};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_network_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new("Network").font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            egui::Grid::new("settings_network_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    field_label(ui, palette, "Local SIP port:");
                    ui.horizontal(|ui| {
                        edited |= ui.add(egui::DragValue::new(&mut self.config.local_sip_port)).changed();
                        info_hint(ui, palette, "Base port this app binds for signaling. Each \
                            additional enabled account (Accounts above) uses the next port up. \
                            Restart required to apply.");
                    });
                    ui.end_row();

                    field_label(ui, palette, "STUN server:");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.stun_server, "e.g. stun.l.google.com:19302", 240.0);
                        info_hint(ui, palette, "Discovers your public IP/port for NAT traversal -- \
                            used as ICE's fallback (or directly, if ICE above is off).");
                    });
                    ui.end_row();

                    field_label(ui, palette, "TURN server:");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.turn_server, "e.g. turn.example.com:3478", 240.0);
                        info_hint(ui, palette, "Relay server used when direct/STUN NAT traversal \
                            fails (e.g. symmetric NAT on both ends). Needs the Username/Password \
                            below if the server requires auth.");
                    });
                    ui.end_row();

                    field_label(ui, palette, "TURN username:");
                    edited |= optional_text_field(ui, palette, &mut self.config.turn_username, "");
                    ui.end_row();

                    field_label(ui, palette, "TURN password:");
                    edited |= optional_password_field(ui, palette, &mut self.config.turn_password);
                    ui.end_row();

                    field_label(ui, palette, "Custom nameserver:");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut self.config.custom_nameserver, "e.g. 1.1.1.1", 240.0);
                        info_hint(ui, palette, "DNS server used for SIP server / SRV lookups, \
                            instead of the OS-configured resolver.");
                    });
                    ui.end_row();
                });
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.dns_srv_enabled,
                    "Use DNS SRV records to locate the SIP server"
                ).changed();
                info_hint(ui, palette, "Looks up _sip._udp/_tcp or _sips._tcp for each \
                    account's server host before falling back to a plain A/AAAA lookup. \
                    Restart required to apply.");
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.ice_enabled,
                    "Use ICE (RFC 8445) for NAT traversal, falling back to the above if it fails"
                ).changed();
                info_hint(ui, palette, "Takes effect on the next call placed or answered, \
                    not calls already in progress.");
            });
            ui.add_space(6.0);
            let mut use_rtp_range = self.config.rtp_port_min.is_some() || self.config.rtp_port_max.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut use_rtp_range, "Restrict RTP to a port range").changed() {
                    if use_rtp_range {
                        self.config.rtp_port_min.get_or_insert(10000);
                        self.config.rtp_port_max.get_or_insert(20000);
                    } else {
                        self.config.rtp_port_min = None;
                        self.config.rtp_port_max = None;
                    }
                    edited = true;
                }
                info_hint(ui, palette, "Pin RTP media to a fixed port range for firewall/NAT \
                    port-forwarding, instead of an OS-assigned port every call. Restart required to apply.");
            });
            if use_rtp_range {
                let mut min = self.config.rtp_port_min.unwrap_or(10000);
                let mut max = self.config.rtp_port_max.unwrap_or(20000);
                ui.horizontal(|ui| {
                    field_label(ui, palette, "Min:");
                    edited |= ui.add(egui::DragValue::new(&mut min).range(1..=65534)).changed();
                    field_label(ui, palette, "Max:");
                    edited |= ui.add(egui::DragValue::new(&mut max).range(1..=65535)).changed();
                });
                self.config.rtp_port_min = Some(min);
                self.config.rtp_port_max = Some(max.max(min));
            }
        });
        edited
    }
}
