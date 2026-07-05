use deelip_config::{DtmfMode, SipAccount, TransportProtocol};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{account_label, codec_label, info_hint};
use crate::theme;

impl DeelipApp {
    pub(crate) fn show_settings(&mut self, ui: &mut Ui) {
        if self.config.accounts.is_empty() {
            self.config.accounts.push(deelip_config::SipAccount::default());
        }
        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
        let mut edited = false;
        let palette = self.palette;

        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // ── Appearance (applies immediately) ─────────────────────────────
            ui.horizontal(|ui| {
                ui.label(RichText::new("Appearance").strong());
                info_hint(ui, &palette, "Applies immediately — no restart needed.");
            });
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
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
            ui.horizontal(|ui| {
                ui.label(RichText::new("Notifications & Ringtone").strong());
                info_hint(ui, &palette, "Applies immediately — no restart needed.");
            });
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if ui.checkbox(&mut self.config.notifications_enabled, "Desktop notification on incoming calls").changed() {
                    self.save_config_quietly();
                }
                if ui.checkbox(&mut self.config.ringtone_enabled, "Ringtone (incoming) / ringback (outgoing)").changed() {
                    self.save_config_quietly();
                }
            });
            ui.add_space(14.0);

            // ── Blocklist ────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(RichText::new("Blocklist").strong());
                info_hint(ui, &palette, "Applies immediately — no restart needed.");
            });
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
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
                    ui.label(RichText::new("No blocked numbers.").color(palette.muted).small());
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
            ui.label(RichText::new("Startup").strong());
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.start_minimized, "Start minimized (to tray)").changed();
                    info_hint(ui, &palette, "Restart to apply.");
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
            ui.horizontal(|ui| {
                ui.label(RichText::new("Updates").strong());
                info_hint(ui, &palette, "Applies immediately — no restart needed.");
            });
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(palette.muted));
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
            egui::ComboBox::from_id_source("settings_account_picker")
                .selected_text(format!(
                    "{}. {}",
                    self.edit_account_idx + 1,
                    account_label(&self.config.accounts[self.edit_account_idx]),
                ))
                .show_ui(ui, |ui| {
                    for i in 0..self.config.accounts.len() {
                        let label = format!("{}. {}", i + 1, account_label(&self.config.accounts[i]));
                        ui.selectable_value(&mut self.edit_account_idx, i, label);
                    }
                });
            ui.add_space(6.0);

            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let account = &mut self.config.accounts[self.edit_account_idx];

                edited |= ui.checkbox(&mut account.enabled, "Enabled (register this account on next restart)").changed();
                edited |= ui.checkbox(&mut account.dnd, "Do Not Disturb (reject all incoming calls)").changed();
                ui.add_space(4.0);

                egui::Grid::new("settings_account_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Username:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.username)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Password:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.password)
                            .password(true)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Server:");
                        edited |= ui.add(egui::TextEdit::singleline(&mut account.server)
                            .desired_width(f32::INFINITY)).changed();
                        ui.end_row();

                        ui.label("Port:");
                        edited |= ui.add(egui::DragValue::new(&mut account.port)).changed();
                        ui.end_row();

                        ui.label("Display name:");
                        let mut display_name = account.display_name.clone().unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut display_name)
                            .desired_width(f32::INFINITY)).changed()
                        {
                            account.display_name = if display_name.is_empty() { None } else { Some(display_name) };
                            edited = true;
                        }
                        ui.end_row();

                        ui.label("Transport:");
                        egui::ComboBox::from_id_source("settings_transport")
                            .selected_text(match account.transport {
                                TransportProtocol::Udp => "UDP",
                                TransportProtocol::Tcp => "TCP",
                                TransportProtocol::Tls => "TLS",
                            })
                            .show_ui(ui, |ui| {
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Udp, "UDP").changed();
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tcp, "TCP").changed();
                                edited |= ui.selectable_value(&mut account.transport, TransportProtocol::Tls, "TLS").changed();
                            });
                        ui.end_row();
                    });

                if account.transport == TransportProtocol::Tls {
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
                for name in ["opus", "g722", "pcmu", "pcma", "gsm", "ilbc"] {
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
            });

            if !self.config.accounts.iter().any(|a| a.enabled) {
                ui.label(RichText::new(
                    "Warning: no accounts are enabled — DeeLip won't be able to register on restart."
                ).color(palette.warn));
            }

            ui.add_space(14.0);

            // ── Audio ─────────────────────────────────────────────────────
            ui.label(RichText::new("Audio").strong());
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
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
                        ui.label("Input device:");
                        let selected = self.config.audio.input_device.clone()
                            .unwrap_or_else(|| "Default".into());
                        egui::ComboBox::from_id_source("settings_input_device")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.config.audio.input_device.is_none(), "Default").clicked() {
                                    self.config.audio.input_device = None;
                                    edited = true;
                                }
                                for name in &input_names {
                                    let is_sel = self.config.audio.input_device.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(is_sel, name).clicked() {
                                        self.config.audio.input_device = Some(name.clone());
                                        edited = true;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Output device:");
                        let selected = self.config.audio.output_device.clone()
                            .unwrap_or_else(|| "Default".into());
                        egui::ComboBox::from_id_source("settings_output_device")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.config.audio.output_device.is_none(), "Default").clicked() {
                                    self.config.audio.output_device = None;
                                    edited = true;
                                }
                                for name in &output_names {
                                    let is_sel = self.config.audio.output_device.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(is_sel, name).clicked() {
                                        self.config.audio.output_device = Some(name.clone());
                                        edited = true;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Ringing device:");
                        let selected = self.config.audio.ringtone_device.clone()
                            .unwrap_or_else(|| "Default".into());
                        egui::ComboBox::from_id_source("settings_ringtone_device")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.config.audio.ringtone_device.is_none(), "Default").clicked() {
                                    self.config.audio.ringtone_device = None;
                                    edited = true;
                                }
                                for name in &output_names {
                                    let is_sel = self.config.audio.ringtone_device.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(is_sel, name).clicked() {
                                        self.config.audio.ringtone_device = Some(name.clone());
                                        edited = true;
                                    }
                                }
                            });
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
                edited |= ui.checkbox(&mut self.config.audio.echo_cancellation, "Echo cancellation").changed();
                ui.horizontal(|ui| {
                    edited |= ui.checkbox(&mut self.config.recording_enabled, "Record calls").changed();
                    info_hint(ui, &palette, "Recordings saved to ~/.config/deelip/recordings/");
                });
            });

            ui.add_space(14.0);

            // ── Network ───────────────────────────────────────────────────
            ui.label(RichText::new("Network").strong());
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
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
            theme::card_frame(&palette).show(ui, |ui| {
                ui.set_width(ui.available_width());
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
    let devices = if input { host.input_devices() } else { host.output_devices() };
    match devices {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(_)      => Vec::new(),
    }
}

/// Text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_text_field(ui: &mut Ui, value: &mut Option<String>, hint: &str) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    ).changed();
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

/// Masked text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_password_field(ui: &mut Ui, value: &mut Option<String>) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui.add(
        egui::TextEdit::singleline(&mut text)
            .password(true)
            .desired_width(f32::INFINITY),
    ).changed();
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}
