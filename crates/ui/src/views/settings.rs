use deelip_config::{
    DtmfMode, MediaEncryption, RecordingFormat, SipAccount, TransportProtocol, UpdateCheckFrequency,
};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{
    account_label, account_status_label, codec_label, device_picker, empty_state, info_hint,
    settings_section,
};
use crate::theme;

impl DeelipApp {
    pub(crate) fn show_settings(&mut self, ui: &mut Ui) {
        if self.config.accounts.is_empty() {
            self.config
                .accounts
                .push(deelip_config::SipAccount::default());
        }
        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
        let mut edited = false;
        let palette = self.palette;

        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // ── Appearance (applies immediately) ─────────────────────────────
            settings_section(ui, &palette, "Appearance", Some("Applies immediately — no restart needed."), |ui| {
                ui.horizontal(|ui| {
                    ui.label("Theme:");
                    if ui.selectable_label(!self.config.dark_mode, format!("{}  Light", egui_phosphor::regular::SUN)).clicked() {
                        self.config.dark_mode = false;
                        self.save_config_quietly();
                    }
                    if ui.selectable_label(self.config.dark_mode, format!("{}  Dark", egui_phosphor::regular::MOON)).clicked() {
                        self.config.dark_mode = true;
                        self.save_config_quietly();
                    }
                });
            });
            ui.add_space(14.0);

            // ── Notifications & Ringtone (applies immediately) ──────────────
            settings_section(ui, &palette, "Notifications & Ringtone", Some("Applies immediately — no restart needed."), |ui| {
                if ui.checkbox(&mut self.config.notifications_enabled, "Desktop notification on incoming calls").changed() {
                    self.save_config_quietly();
                }
                if ui.checkbox(&mut self.config.ringtone_enabled, "Ringtone (incoming) / ringback (outgoing)").changed() {
                    self.save_config_quietly();
                }
            });
            ui.add_space(14.0);

            // ── Blocklist ────────────────────────────────────────────────────
            settings_section(ui, &palette, "Blocklist", Some("Applies immediately — no restart needed."), |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.blocklist_input)
                        .hint_text("number or sip:user@host")
                        .desired_width(200.0));
                    if ui.button("Block").clicked() {
                        let entry = self.blocklist_input.trim().to_string();
                        if !entry.is_empty() && !self.config.blocklist.iter().any(|e| e.eq_ignore_ascii_case(&entry)) {
                            self.config.blocklist.push(entry);
                            self.save_config_quietly();
                        }
                        self.blocklist_input.clear();
                    }
                });
                if self.config.blocklist.is_empty() {
                    empty_state(ui, &palette, "No blocked numbers.");
                } else {
                    let mut remove_idx = None;
                    for (i, entry) in self.config.blocklist.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(entry);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Remove").clicked() {
                                    remove_idx = Some(i);
                                }
                            });
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.config.blocklist.remove(i);
                        self.save_config_quietly();
                    }
                }
            });
            ui.add_space(14.0);

            // ── Startup ───────────────────────────────────────────────────
            settings_section(ui, &palette, "Startup", None, |ui| {
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.start_minimized, "Start minimized (to tray)").changed();
                    info_hint(ui, &palette, "Restart to apply.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.log_to_file, "Enable log file").changed();
                    info_hint(ui, &palette, "Also writes logs to ~/.config/deelip/deelip.log, \
                        in addition to the console. Restart to apply.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.autostart_enabled, "Start DeeLip on login").changed() {
                        if let Err(e) = deelip_config::set_autostart(self.autostart_enabled) {
                            tracing::error!("Failed to update autostart: {e}");
                            self.autostart_enabled = deelip_config::is_autostart_enabled();
                        }
                    }
                    info_hint(ui, &palette, "Applies immediately — no restart needed.");
                });
            });
            ui.add_space(14.0);

            // ── Updates (applies immediately) ────────────────────────────────
            settings_section(ui, &palette, "Updates", Some("Applies immediately — no restart needed."), |ui| {
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(palette.muted));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Check for updates:");
                    egui::ComboBox::from_id_source("settings_update_check_frequency")
                        .selected_text(match self.config.update_check_frequency {
                            UpdateCheckFrequency::Always => "Every launch",
                            UpdateCheckFrequency::Daily => "Daily",
                            UpdateCheckFrequency::Weekly => "Weekly",
                            UpdateCheckFrequency::Never => "Never",
                        })
                        .show_ui(ui, |ui| {
                            for (val, label) in [
                                (UpdateCheckFrequency::Always, "Every launch"),
                                (UpdateCheckFrequency::Daily, "Daily"),
                                (UpdateCheckFrequency::Weekly, "Weekly"),
                                (UpdateCheckFrequency::Never, "Never"),
                            ] {
                                if ui.selectable_value(&mut self.config.update_check_frequency, val, label).changed() {
                                    self.save_config_quietly();
                                }
                            }
                        });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.config.auto_update_enabled, "Automatically download and install updates").changed() {
                        self.save_config_quietly();
                    }
                    info_hint(ui, &palette, "Only works for a portable (tar.gz/install.sh) install -- \
                        .deb/.rpm installs are always updated through your package manager instead, \
                        regardless of this toggle.");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Check for updates now").clicked() {
                        self.start_update_check();
                    }
                    let status = match &self.update_state {
                        crate::update::UpdateState::Idle       => "Up to date (or not checked yet).".to_string(),
                        crate::update::UpdateState::Checking    => "Checking…".to_string(),
                        crate::update::UpdateState::Available(r) => format!("Update available: {}", r.version),
                        crate::update::UpdateState::Downloading => "Downloading update…".to_string(),
                        crate::update::UpdateState::Updated(v)  => format!("Updated to {v} -- restart to finish."),
                        crate::update::UpdateState::Failed(e)   => format!("Check failed: {e}"),
                    };
                    ui.label(RichText::new(status).color(palette.muted).small());
                });
            });
            ui.add_space(14.0);

            // ── Account ───────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(RichText::new("Accounts").strong());
                info_hint(ui, &palette, "Each enabled account registers independently on its own \
                    local SIP port (base port below, incrementing by one per additional account).");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_remove = self.config.accounts.len() > 1;
                    if ui.add_enabled(can_remove, egui::Button::new("Remove")).clicked() {
                        self.config.accounts.remove(self.edit_account_idx);
                        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
                        edited = true;
                    }
                    if ui.button("+ Add account").clicked() {
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
            let is_registered = |acc: &SipAccount| self.accounts.iter().any(|a| a.account.username == acc.username && a.reg_ok);
            let selected_text = account_status_label(
                ui, &palette, is_registered(&self.config.accounts[self.edit_account_idx]),
                &format!("{}. {}", self.edit_account_idx + 1, account_label(&self.config.accounts[self.edit_account_idx])),
            );
            egui::ComboBox::from_id_source("settings_account_picker")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for i in 0..self.config.accounts.len() {
                        let label_text = format!("{}. {}", i + 1, account_label(&self.config.accounts[i]));
                        let label = account_status_label(ui, &palette, is_registered(&self.config.accounts[i]), &label_text);
                        if ui.add(egui::SelectableLabel::new(self.edit_account_idx == i, label)).clicked() {
                            self.edit_account_idx = i;
                        }
                    }
                });
            ui.add_space(6.0);

            theme::full_width_card(ui, palette, |ui| {
                let account = &mut self.config.accounts[self.edit_account_idx];

                edited |= ui.checkbox(&mut account.enabled, "Enabled (register this account on next restart)").changed();
                edited |= ui.checkbox(&mut account.dnd, "Do Not Disturb (reject all incoming calls)").changed();
                ui.add_space(4.0);

                egui::Grid::new("settings_account_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Account name:");
                        edited |= optional_text_field(ui, &mut account.account_name, "e.g. Home, Work");
                        ui.end_row();

                        ui.label("Username:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.username)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Password:");
                        ui.horizontal(|ui| {
                            edited |= ui.add(egui::TextEdit::singleline(&mut account.password)
                                .password(!self.show_account_password)
                                .desired_width(200.0)).changed();
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

                        ui.label("Login (optional):");
                        ui.horizontal(|ui| {
                            edited |= optional_text_field(ui, &mut account.auth_username, "defaults to Username");
                            info_hint(ui, &palette, "Digest-auth identity, when a provider requires \
                                a login distinct from the public SIP username above.");
                        });
                        ui.end_row();

                        ui.label("Server:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.server)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Port:");
                        edited |= ui.add(egui::DragValue::new(&mut account.port)).changed();
                        ui.end_row();

                        ui.label("Domain (optional):");
                        ui.horizontal(|ui| {
                            edited |= optional_text_field(ui, &mut account.domain, "defaults to Server");
                            info_hint(ui, &palette, "SIP domain used in From/To/Contact URIs, when it \
                                differs from the registrar host in Server above.");
                        });
                        ui.end_row();

                        ui.label("SIP proxy (optional):");
                        ui.horizontal(|ui| {
                            edited |= optional_text_field(ui, &mut account.sip_proxy, "host[:port]");
                            info_hint(ui, &palette, "Outbound proxy to actually connect through, \
                                instead of Server/Port directly.");
                        });
                        ui.end_row();

                        ui.label("Display name:");
                        edited |= optional_text_field(ui, &mut account.display_name, "");
                        ui.end_row();

                        ui.label("Transport:");
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
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Auto, "Auto").changed();
                            });
                        if account.transport == TransportProtocol::Auto {
                            info_hint(ui, &palette, "Tries UDP, then TCP, then TLS at connect time, \
                                keeping whichever one actually gets a response from the server.");
                        }
                        ui.end_row();
                    });

                if matches!(account.transport, TransportProtocol::Tls | TransportProtocol::Auto) {
                    edited |= ui.checkbox(
                        &mut account.tls_insecure_skip_verify,
                        "Skip TLS certificate verification (self-signed/home-lab PBXes)",
                    ).changed();
                    if account.tls_insecure_skip_verify {
                        ui.label(RichText::new(
                            "Warning: certificate verification is disabled — traffic can be intercepted."
                        ).color(palette.warn));
                    }
                }

                ui.add_space(6.0);
                ui.label("Codecs (order = preference):");
                let mut move_up: Option<usize> = None;
                let mut move_down: Option<usize> = None;
                let mut to_disable: Option<usize> = None;
                for (i, name) in account.codec_order.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(codec_label(name));
                        if ui.add_enabled(i > 0, egui::Button::new("↑")).clicked() {
                            move_up = Some(i);
                        }
                        if ui.add_enabled(i + 1 < account.codec_order.len(), egui::Button::new("↓")).clicked() {
                            move_down = Some(i);
                        }
                        let can_disable = account.codec_order.len() > 1;
                        if ui.add_enabled(can_disable, egui::Button::new("Disable")).clicked() {
                            to_disable = Some(i);
                        }
                    });
                }
                if let Some(i) = move_up { account.codec_order.swap(i, i - 1); edited = true; }
                if let Some(i) = move_down { account.codec_order.swap(i, i + 1); edited = true; }
                if let Some(i) = to_disable { account.codec_order.remove(i); edited = true; }
                for name in ["opus", "g722", "pcmu", "pcma", "gsm", "ilbc", "g729"] {
                    if !account.codec_order.iter().any(|c| c == name) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(codec_label(name)).color(palette.muted));
                            if ui.small_button("Enable").clicked() {
                                account.codec_order.push(name.to_string());
                                edited = true;
                            }
                        });
                    }
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Force codec for incoming:");
                    let selected_label = account.force_incoming_codec.as_deref()
                        .map(codec_label)
                        .unwrap_or("No override");
                    egui::ComboBox::from_id_source("settings_force_incoming_codec")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(account.force_incoming_codec.is_none(), "No override").clicked() {
                                account.force_incoming_codec = None;
                                edited = true;
                            }
                            for name in &account.codec_order {
                                if ui.selectable_label(account.force_incoming_codec.as_deref() == Some(name.as_str()), codec_label(name)).clicked() {
                                    account.force_incoming_codec = Some(name.clone());
                                    edited = true;
                                }
                            }
                        });
                    info_hint(ui, &palette, "Negotiates this codec on an incoming call whenever the \
                        caller offers it at all, ignoring the caller's own preference order.");
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut account.vad_enabled, "Voice activity detection (comfort noise)").changed();
                    info_hint(ui, &palette, "During silence, sends occasional comfort-noise packets \
                        instead of continuous audio, and plays synthesized background noise for the \
                        far end's silence instead of dead air. Only takes effect with a non-Opus codec.");
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("DTMF mode:");
                    egui::ComboBox::from_id_source("settings_dtmf_mode")
                        .selected_text(match account.dtmf_mode {
                            DtmfMode::Rfc2833 => "RFC 2833 (RTP telephone-event)",
                            DtmfMode::SipInfo => "SIP INFO",
                            DtmfMode::Inband  => "Inband (audio tone)",
                        })
                        .show_ui(ui, |ui| {
                            edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::Rfc2833, "RFC 2833 (RTP telephone-event)").changed();
                            edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::SipInfo, "SIP INFO").changed();
                            edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::Inband, "Inband (audio tone)").changed();
                        });
                });

                ui.add_space(6.0);
                ui.label("Forward always (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.forward_always, "sip:reception@example.com");
                });

                ui.add_space(6.0);
                ui.label("Forward when busy (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.forward_on_busy, "sip:voicemail@example.com");
                });

                ui.add_space(6.0);
                ui.label("Forward if unanswered (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.no_answer_forward, "sip:voicemail@example.com");
                });
                ui.horizontal(|ui| {
                    ui.label("after (seconds):");
                    edited |= ui.add(egui::DragValue::new(&mut account.no_answer_timeout_secs).range(1..=300)).changed();
                });

                ui.add_space(6.0);
                edited |= ui.checkbox(&mut account.auto_answer_enabled, "Auto-answer incoming calls (intercom mode)").changed();
                if account.auto_answer_enabled {
                    ui.horizontal(|ui| {
                        ui.label("after (seconds):");
                        edited |= ui.add(egui::DragValue::new(&mut account.auto_answer_secs).range(0..=60)).changed();
                    });
                }

                ui.add_space(6.0);
                ui.label("Voicemail mailbox for MWI (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.mailbox, "1000");
                });

                ui.add_space(6.0);
                ui.label("Dialing prefix (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.dialing_prefix, "e.g. 9");
                    info_hint(ui, &palette, "Auto-prepended to bare numbers dialed from this account \
                        (e.g. \"9\" for an outside line) -- not applied to a full SIP URI or an \
                        explicit user@host entry.");
                });

                ui.add_space(6.0);
                edited |= ui.checkbox(&mut account.hide_caller_id, "Hide caller ID (send Privacy: id)").changed();

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Register refresh (seconds):");
                    edited |= ui.add(egui::DragValue::new(&mut account.register_expires).range(60..=86400)).changed();
                    info_hint(ui, &palette, "Requested REGISTER Expires -- the server may return a \
                        shorter value, which re-registration timing always honors regardless of this.");
                });

                ui.add_space(6.0);
                let mut keepalive_on = account.keepalive_secs.is_some();
                if ui.checkbox(&mut keepalive_on, "NAT keepalive").changed() {
                    account.keepalive_secs = if keepalive_on { Some(15) } else { None };
                    edited = true;
                }
                if let Some(secs) = &mut account.keepalive_secs {
                    ui.horizontal(|ui| {
                        ui.label("every (seconds):");
                        edited |= ui.add(egui::DragValue::new(secs).range(5..=300)).changed();
                        info_hint(ui, &palette, "Sends a lone empty packet to the registrar on this \
                            interval, to hold a NAT/firewall's outbound binding open between registrations.");
                    });
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Media encryption:");
                    egui::ComboBox::from_id_source("settings_media_encryption")
                        .selected_text(match account.media_encryption {
                            MediaEncryption::MatchTransport => "Match transport (default)",
                            MediaEncryption::Disabled => "Disabled",
                            MediaEncryption::Enabled => "Always (SRTP)",
                        })
                        .show_ui(ui, |ui| {
                            edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::MatchTransport, "Match transport (default)").changed();
                            edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::Disabled, "Disabled").changed();
                            edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::Enabled, "Always (SRTP)").changed();
                        });
                });
                info_hint(ui, &palette, "\"Match transport\" offers SRTP exactly when the signaling \
                    transport is TLS (today's behavior); the other two are independent of transport.");

                ui.add_space(6.0);
                ui.label("Public address (optional):");
                ui.horizontal(|ui| {
                    edited |= optional_text_field(ui, &mut account.public_address, "e.g. 203.0.113.5");
                    info_hint(ui, &palette, "Overrides the address advertised in Contact/SDP for this \
                        account, instead of the globally STUN-discovered external IP.");
                });

                ui.add_space(6.0);
                let mut ice_override_on = account.ice_enabled.is_some();
                if ui.checkbox(&mut ice_override_on, "Override global ICE setting for this account").changed() {
                    account.ice_enabled = if ice_override_on { Some(self.config.ice_enabled) } else { None };
                    edited = true;
                }
                if let Some(ice_on) = &mut account.ice_enabled {
                    edited |= ui.checkbox(ice_on, "Use ICE (RFC 8445) for this account").changed();
                }
            });

            if !self.config.accounts.iter().any(|a| a.enabled) {
                ui.label(RichText::new(
                    "Warning: no accounts are enabled — DeeLip won't be able to register on restart."
                ).color(palette.warn));
            }

            ui.add_space(14.0);

            // ── Audio ─────────────────────────────────────────────────────
            ui.label(RichText::new("Audio").strong());
            theme::full_width_card(ui, palette, |ui| {
                let (input_names, output_names) = self.audio_device_cache
                    .get_or_insert_with(|| (list_device_names(true), list_device_names(false)))
                    .clone();

                if ui.button("Refresh device list").clicked() {
                    self.audio_device_cache = Some((list_device_names(true), list_device_names(false)));
                }

                egui::Grid::new("settings_audio_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        edited |= device_picker(ui, "settings_input_device", "Input device:", &mut self.config.audio.input_device, &input_names);
                        ui.end_row();
                        edited |= device_picker(ui, "settings_output_device", "Output device:", &mut self.config.audio.output_device, &output_names);
                        ui.end_row();
                        edited |= device_picker(ui, "settings_ringtone_device", "Ringing device:", &mut self.config.audio.ringtone_device, &output_names);
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Ringing device").color(palette.muted).small());
                    info_hint(ui, &palette, "Independent of the Output device above -- lets the \
                        ringtone play on a different device than call audio, e.g. ring on \
                        speakers, talk on a headset.");
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Custom ringtone (WAV):");
                    let name = self.config.audio.ringtone_file.as_deref()
                        .and_then(|p| std::path::Path::new(p).file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Built-in tone".into());
                    ui.label(RichText::new(name).color(palette.muted));
                    if ui.small_button("Choose…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                            self.config.audio.ringtone_file = Some(path.to_string_lossy().into_owned());
                            edited = true;
                        }
                    }
                    if self.config.audio.ringtone_file.is_some() && ui.small_button("Clear").clicked() {
                        self.config.audio.ringtone_file = None;
                        edited = true;
                    }
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Ringtone volume:");
                    edited |= ui.add(egui::Slider::new(&mut self.config.audio.ringtone_volume, 0.0..=2.0)
                        .fixed_decimals(2)).changed();
                });

                ui.add_space(6.0);
                edited |= ui.checkbox(&mut self.config.audio.echo_cancellation, "Echo cancellation").changed();
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.audio.agc_enabled, "Automatic microphone gain control").changed();
                    info_hint(ui, &palette, "Adaptively boosts a quiet mic signal and limits a loud one.");
                });

                ui.add_space(6.0);
                edited |= ui.checkbox(&mut self.config.recording_enabled, "Record calls").changed();
                if self.config.recording_enabled {
                    ui.horizontal(|ui| {
                        ui.label("Format:");
                        egui::ComboBox::from_id_source("settings_recording_format")
                            .selected_text(match self.config.recording_format {
                                RecordingFormat::Wav => "WAV (lossless, larger files)",
                                RecordingFormat::Mp3 => "MP3 (lossy, smaller files)",
                            })
                            .show_ui(ui, |ui| {
                                edited |= ui.selectable_value(&mut self.config.recording_format, RecordingFormat::Wav, "WAV (lossless, larger files)").changed();
                                edited |= ui.selectable_value(&mut self.config.recording_format, RecordingFormat::Mp3, "MP3 (lossy, smaller files)").changed();
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Save to:");
                        let shown = self.config.recordings_dir_override.as_deref()
                            .unwrap_or("~/.config/deelip/recordings (default)");
                        ui.label(RichText::new(shown).color(palette.muted));
                        if ui.small_button("Choose…").clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.config.recordings_dir_override = Some(dir.to_string_lossy().into_owned());
                                edited = true;
                            }
                        }
                        if self.config.recordings_dir_override.is_some() && ui.small_button("Reset").clicked() {
                            self.config.recordings_dir_override = None;
                            edited = true;
                        }
                    });
                }
            });

            ui.add_space(14.0);

            // ── Network ───────────────────────────────────────────────────
            ui.label(RichText::new("Network").strong());
            theme::full_width_card(ui, palette, |ui| {
                egui::Grid::new("settings_network_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Local SIP port:");
                        edited |= ui.add(egui::DragValue::new(&mut self.config.local_sip_port)).changed();
                        ui.end_row();

                        ui.label("STUN server:");
                        edited |= optional_text_field(ui, &mut self.config.stun_server, "e.g. stun.l.google.com:19302");
                        ui.end_row();

                        ui.label("TURN server:");
                        edited |= optional_text_field(ui, &mut self.config.turn_server, "e.g. turn.example.com:3478");
                        ui.end_row();

                        ui.label("TURN username:");
                        edited |= optional_text_field(ui, &mut self.config.turn_username, "");
                        ui.end_row();

                        ui.label("TURN password:");
                        edited |= optional_password_field(ui, &mut self.config.turn_password);
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.ice_enabled,
                        "Use ICE (RFC 8445) for NAT traversal, falling back to the above if it fails"
                    ).changed();
                    info_hint(ui, &palette, "Takes effect on the next call placed or answered, \
                        not calls already in progress.");
                });
            });

            ui.add_space(14.0);
            ui.label(RichText::new("Global Hotkeys").strong());
            theme::full_width_card(ui, palette, |ui| {
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.global_hotkeys_enabled,
                        "Enable system-wide Answer/Hangup/Mute hotkeys (Linux: X11 only)"
                    ).changed();
                    info_hint(ui, &palette, "Format: \"Ctrl+Alt+A\" style. Restart required to apply.");
                });
                if self.config.global_hotkeys_enabled {
                    egui::Grid::new("hotkeys_grid").num_columns(2).show(ui, |ui| {
                        ui.label("Answer:");
                        edited |= ui.text_edit_singleline(&mut self.config.hotkey_answer).changed();
                        ui.end_row();
                        ui.label("Hangup:");
                        edited |= ui.text_edit_singleline(&mut self.config.hotkey_hangup).changed();
                        ui.end_row();
                        ui.label("Mute:");
                        edited |= ui.text_edit_singleline(&mut self.config.hotkey_mute).changed();
                        ui.end_row();
                    });
                }
            });

            ui.add_space(14.0);

            if ui.button("Save").clicked() {
                match self.config.save(&self.db) {
                    Ok(())   => self.settings_saved_notice = true,
                    Err(err) => {
                        self.settings_saved_notice = false;
                        tracing::error!("Failed to save config: {err}");
                    }
                }
            }
            if self.settings_saved_notice {
                ui.label(RichText::new("Saved — restart DeeLip to apply changes.").color(palette.accent));
            }
        });

        if edited {
            self.settings_saved_notice = false;
        }
    }
}

/// List available cpal device names (input or output), for populating the
/// Settings device pickers.
fn list_device_names(input: bool) -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let devices = if input {
        host.input_devices()
    } else {
        host.output_devices()
    };
    match devices {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_text_field(ui: &mut Ui, value: &mut Option<String>, hint: &str) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
        .changed();
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

/// Masked text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_password_field(ui: &mut Ui, value: &mut Option<String>) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .password(true)
                .desired_width(f32::INFINITY),
        )
        .changed();
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}
