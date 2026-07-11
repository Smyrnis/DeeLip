use deelip_config::{DialPlanRule, DtmfMode, MediaEncryption, SipAccount, TransportProtocol};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{
    account_label, account_status_label, codec_label, empty_state, field_label, info_hint, text_edit_scope,
};
use crate::strings::t;
use crate::theme::{self, Palette};

use super::{optional_text_field, optional_text_field_sized};

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
                    self.config.accounts.remove(self.edit_account_idx);
                    self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
                    edited = true;
                }
                if ui.button(format!("+ {}", t("settings.account.add_account_button"))).clicked() {
                    self.config.accounts.push(SipAccount::default());
                    self.edit_account_idx = self.config.accounts.len() - 1;
                    edited = true;
                }
            });
        });
        ui.add_space(4.0);
        // A draft account only has a live registration-status dot once
        // it matches a currently-running identity by username -- a
        // freshly-added or just-edited entry has no such match yet
        // (accurately reads as "not registered" until Save + restart).
        let is_registered =
            |acc: &SipAccount| self.accounts.iter().any(|a| a.account.username == acc.username && a.reg_ok);
        let selected_text = account_status_label(
            ui,
            palette,
            is_registered(&self.config.accounts[self.edit_account_idx]),
            &format!("{}. {}", self.edit_account_idx + 1, account_label(&self.config.accounts[self.edit_account_idx])),
        );
        egui::ComboBox::from_id_source("settings_account_picker").selected_text(selected_text).show_ui(ui, |ui| {
            for i in 0..self.config.accounts.len() {
                let label_text = format!("{}. {}", i + 1, account_label(&self.config.accounts[i]));
                let label = account_status_label(ui, palette, is_registered(&self.config.accounts[i]), &label_text);
                if ui.add(egui::SelectableLabel::new(self.edit_account_idx == i, label)).clicked() {
                    self.edit_account_idx = i;
                }
            }
        });
        ui.add_space(6.0);

        theme::full_width_card(ui, *palette, |ui| {
            let account = &mut self.config.accounts[self.edit_account_idx];

            edited |= ui.checkbox(&mut account.enabled, t("settings.account.enabled_checkbox")).changed();
            edited |= ui.checkbox(&mut account.dnd, t("settings.account.dnd_checkbox")).changed();
            ui.horizontal(|ui| {
                edited |=
                    ui.checkbox(&mut account.local_account, t("settings.account.local_account_checkbox")).changed();
                info_hint(ui, palette, &t("settings.account.local_account_hint"));
            });
            if account.local_account {
                empty_state(ui, palette, &t("settings.account.local_account_empty_state"));
            }
            ui.add_space(4.0);

            egui::Grid::new("settings_account_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                field_label(ui, palette, &t("settings.account.account_name_label"));
                edited |= optional_text_field(
                    ui,
                    palette,
                    &mut account.account_name,
                    &t("settings.account.account_name_hint"),
                );
                ui.end_row();

                field_label(ui, palette, &t("settings.account.username_label"));
                edited |= text_edit_scope(ui, palette, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut account.username).desired_width(f32::INFINITY)).changed()
                });
                ui.end_row();

                field_label(ui, palette, &t("settings.account.password_label"));
                ui.horizontal(|ui| {
                    edited |= text_edit_scope(ui, palette, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut account.password)
                                .password(!self.show_account_password)
                                .desired_width(200.0),
                        )
                        .changed()
                    });
                    let icon = if self.show_account_password {
                        egui_phosphor::regular::EYE_SLASH
                    } else {
                        egui_phosphor::regular::EYE
                    };
                    if ui.small_button(icon).clicked() {
                        self.show_account_password = !self.show_account_password;
                    }
                });
                ui.end_row();

                field_label(ui, palette, &t("settings.account.login_label"));
                ui.horizontal(|ui| {
                    edited |= optional_text_field_sized(
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
                edited |= text_edit_scope(ui, palette, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut account.server)
                            .font(theme::font_address())
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                });
                ui.end_row();

                field_label(ui, palette, &t("settings.account.port_label"));
                edited |= ui.add(egui::DragValue::new(&mut account.port)).changed();
                ui.end_row();

                field_label(ui, palette, &t("settings.account.domain_label"));
                ui.horizontal(|ui| {
                    edited |= optional_text_field_sized(
                        ui,
                        palette,
                        &mut account.domain,
                        &t("settings.account.domain_hint"),
                        240.0,
                    );
                    info_hint(ui, palette, &t("settings.account.domain_info"));
                });
                ui.end_row();

                field_label(ui, palette, &t("settings.account.sip_proxy_label"));
                ui.horizontal(|ui| {
                    edited |= optional_text_field_sized(
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
                edited |= optional_text_field(ui, palette, &mut account.display_name, "");
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
                        edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Udp, "UDP").changed();
                        edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tcp, "TCP").changed();
                        edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tls, "TLS").changed();
                        edited |=
                            ui.selectable_value(&mut account.transport, TransportProtocol::Auto, "Auto").changed();
                    });
                if account.transport == TransportProtocol::Auto {
                    info_hint(ui, palette, &t("settings.account.transport_auto_info"));
                }
                ui.end_row();
            });

            if matches!(account.transport, TransportProtocol::Tls | TransportProtocol::Auto) {
                edited |= ui
                    .checkbox(&mut account.tls_insecure_skip_verify, t("settings.account.tls_skip_verify_checkbox"))
                    .changed();
                if account.tls_insecure_skip_verify {
                    ui.label(RichText::new(t("settings.account.tls_warning")).color(palette.ringing));
                }
            }

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
                    ui.label(
                        RichText::new(t("settings.account.codecs_available_label")).color(palette.ink_muted).small(),
                    );
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
                    ui.label(
                        RichText::new(t("settings.account.codecs_enabled_label")).color(palette.ink_muted).small(),
                    );
                    list_frame.show(ui, |ui| {
                        // Fixed width, same reasoning as the Available
                        // column above.
                        ui.set_width(290.0);
                        ui.set_min_height(120.0);
                        for (i, name) in account.codec_order.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let can_disable = account.codec_order.len() > 1;
                                if ui
                                    .add_enabled(
                                        can_disable,
                                        egui::Button::new(egui_phosphor::regular::ARROW_LEFT).small(),
                                    )
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
                edited = true;
            }
            if let Some(i) = move_up {
                account.codec_order.swap(i, i - 1);
                edited = true;
            }
            if let Some(i) = move_down {
                account.codec_order.swap(i, i + 1);
                edited = true;
            }
            if let Some(i) = to_disable {
                account.codec_order.remove(i);
                edited = true;
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.force_incoming_codec_label"));
                let no_override = t("settings.account.no_override_option");
                let selected_label =
                    account.force_incoming_codec.as_deref().map(codec_label).unwrap_or(no_override.as_str());
                egui::ComboBox::from_id_source("settings_force_incoming_codec").selected_text(selected_label).show_ui(
                    ui,
                    |ui| {
                        if ui
                            .selectable_label(
                                account.force_incoming_codec.is_none(),
                                t("settings.account.no_override_option"),
                            )
                            .clicked()
                        {
                            account.force_incoming_codec = None;
                            edited = true;
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
                                edited = true;
                            }
                        }
                    },
                );
                info_hint(ui, palette, &t("settings.account.force_incoming_codec_info"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.vad_enabled, t("settings.account.vad_checkbox")).changed();
                info_hint(ui, palette, &t("settings.account.vad_info"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.dtmf_mode_label"));
                egui::ComboBox::from_id_source("settings_dtmf_mode")
                    .selected_text(match account.dtmf_mode {
                        DtmfMode::Rfc2833 => t("settings.account.dtmf_rfc2833"),
                        DtmfMode::SipInfo => t("settings.account.dtmf_sipinfo"),
                        DtmfMode::Inband => t("settings.account.dtmf_inband"),
                        DtmfMode::Auto => t("settings.account.dtmf_auto"),
                    })
                    .show_ui(ui, |ui| {
                        edited |= ui
                            .selectable_value(
                                &mut account.dtmf_mode,
                                DtmfMode::Rfc2833,
                                t("settings.account.dtmf_rfc2833"),
                            )
                            .changed();
                        edited |= ui
                            .selectable_value(
                                &mut account.dtmf_mode,
                                DtmfMode::SipInfo,
                                t("settings.account.dtmf_sipinfo"),
                            )
                            .changed();
                        edited |= ui
                            .selectable_value(
                                &mut account.dtmf_mode,
                                DtmfMode::Inband,
                                t("settings.account.dtmf_inband"),
                            )
                            .changed();
                        edited |= ui
                            .selectable_value(&mut account.dtmf_mode, DtmfMode::Auto, t("settings.account.dtmf_auto"))
                            .changed();
                    });
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.forward_always_label"));
                edited |= optional_text_field(
                    ui,
                    palette,
                    &mut account.forward_always,
                    &t("settings.account.forward_always_hint"),
                );
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.forward_busy_label"));
                edited |= optional_text_field(
                    ui,
                    palette,
                    &mut account.forward_on_busy,
                    &t("settings.account.voicemail_uri_hint"),
                );
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.forward_unanswered_label"));
                edited |= optional_text_field_sized(
                    ui,
                    palette,
                    &mut account.no_answer_forward,
                    &t("settings.account.voicemail_uri_hint"),
                    180.0,
                );
                field_label(ui, palette, &t("settings.account.after_seconds_short_label"));
                edited |= ui.add(egui::DragValue::new(&mut account.no_answer_timeout_secs).range(1..=300)).changed();
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |=
                    ui.checkbox(&mut account.auto_answer_enabled, t("settings.account.auto_answer_checkbox")).changed();
                info_hint(ui, palette, &t("settings.account.auto_answer_info"));
            });
            if account.auto_answer_enabled {
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.account.after_seconds_label"));
                    edited |= ui.add(egui::DragValue::new(&mut account.auto_answer_secs).range(0..=60)).changed();
                });
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(
                        &mut account.auto_answer_control_button,
                        t("settings.account.auto_answer_control_checkbox"),
                    )
                    .changed();
                info_hint(ui, palette, &t("settings.account.auto_answer_control_info"));
            });
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut account.deny_incoming_control_button, t("settings.account.deny_incoming_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.account.deny_incoming_info"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.mailbox_label"));
                edited |= optional_text_field_sized(
                    ui,
                    palette,
                    &mut account.mailbox,
                    &t("settings.account.mailbox_hint"),
                    100.0,
                );
                info_hint(ui, palette, &t("settings.account.mailbox_info"));
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut account.publish_presence, t("settings.account.publish_presence_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.account.publish_presence_info"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.dialing_prefix_label"));
                edited |= optional_text_field_sized(
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
                        edited |= ui.checkbox(&mut rule.enabled, "").changed();
                        edited |= text_edit_scope(ui, palette, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut rule.pattern)
                                    .hint_text(
                                        RichText::new(t("settings.account.pattern_hint")).color(palette.ink_muted),
                                    )
                                    .desired_width(120.0),
                            )
                            .changed()
                        });
                        field_label(ui, palette, "→");
                        edited |= text_edit_scope(ui, palette, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut rule.replacement)
                                    .hint_text(
                                        RichText::new(t("settings.account.replacement_hint")).color(palette.ink_muted),
                                    )
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
                    edited = true;
                }
            }
            ui.horizontal(|ui| {
                text_edit_scope(ui, palette, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dialplan_pattern_input)
                            .hint_text(
                                RichText::new(t("settings.account.pattern_example_hint")).color(palette.ink_muted),
                            )
                            .desired_width(120.0),
                    )
                });
                field_label(ui, palette, "→");
                text_edit_scope(ui, palette, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dialplan_replacement_input)
                            .hint_text(
                                RichText::new(t("settings.account.replacement_example_hint")).color(palette.ink_muted),
                            )
                            .desired_width(100.0),
                    )
                });
                if ui.button(t("settings.account.add_rule_button")).clicked()
                    && !self.dialplan_pattern_input.trim().is_empty()
                {
                    account.dial_plan.push(DialPlanRule {
                        pattern: self.dialplan_pattern_input.trim().to_string(),
                        replacement: self.dialplan_replacement_input.trim().to_string(),
                        enabled: true,
                    });
                    self.dialplan_pattern_input.clear();
                    self.dialplan_replacement_input.clear();
                    edited = true;
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |=
                    ui.checkbox(&mut account.hide_caller_id, t("settings.account.hide_caller_id_checkbox")).changed();
                info_hint(ui, palette, &t("settings.account.hide_caller_id_info"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.account.register_refresh_label"));
                edited |= ui.add(egui::DragValue::new(&mut account.register_expires).range(60..=86400)).changed();
                info_hint(ui, palette, &t("settings.account.register_refresh_info"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut session_timers_on = account.session_timers_enabled;
                if ui.checkbox(&mut session_timers_on, t("settings.account.session_timers_checkbox")).changed() {
                    account.session_timers_enabled = session_timers_on;
                    edited = true;
                }
                info_hint(ui, palette, &t("settings.account.session_timers_info"));
            });

            ui.add_space(6.0);
            let mut keepalive_on = account.keepalive_secs.is_some();
            if ui.checkbox(&mut keepalive_on, t("settings.account.nat_keepalive_checkbox")).changed() {
                account.keepalive_secs = if keepalive_on { Some(15) } else { None };
                edited = true;
            }
            if let Some(secs) = &mut account.keepalive_secs {
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.account.every_seconds_label"));
                    edited |= ui.add(egui::DragValue::new(secs).range(5..=300)).changed();
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
                        edited |= ui
                            .selectable_value(
                                &mut account.media_encryption,
                                MediaEncryption::MatchTransport,
                                t("settings.account.enc_match_transport"),
                            )
                            .changed();
                        edited |= ui
                            .selectable_value(
                                &mut account.media_encryption,
                                MediaEncryption::Disabled,
                                t("settings.account.enc_disabled"),
                            )
                            .changed();
                        edited |= ui
                            .selectable_value(
                                &mut account.media_encryption,
                                MediaEncryption::Enabled,
                                t("settings.account.enc_always_srtp"),
                            )
                            .changed();
                        edited |= ui
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
                edited |=
                    ui.checkbox(&mut account.video_enabled, t("settings.account.video_enabled_checkbox")).changed();
                info_hint(ui, palette, &t("settings.account.video_enabled_info"));
            });

            ui.add_space(6.0);
            field_label(ui, palette, &t("settings.account.public_address_label"));
            ui.horizontal(|ui| {
                edited |= optional_text_field_sized(
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
                edited |= ui
                    .checkbox(&mut account.allow_ip_rewrite, t("settings.account.allow_ip_rewrite_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.account.allow_ip_rewrite_info"));
            });

            ui.add_space(6.0);
            let mut ice_override_on = account.ice_enabled.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut ice_override_on, t("settings.account.ice_override_checkbox")).changed() {
                    account.ice_enabled = if ice_override_on { Some(self.config.ice_enabled) } else { None };
                    edited = true;
                }
                info_hint(ui, palette, &t("settings.account.ice_override_info"));
            });
            if let Some(ice_on) = &mut account.ice_enabled {
                edited |= ui.checkbox(ice_on, t("settings.account.use_ice_checkbox")).changed();
            }
        });

        if !self.config.accounts.iter().any(|a| a.enabled) {
            ui.label(RichText::new(t("settings.account.no_accounts_warning")).color(palette.ringing));
        }

        edited
    }
}
