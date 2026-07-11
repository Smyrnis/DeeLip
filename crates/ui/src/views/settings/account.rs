use deelip_config::{DialPlanRule, DtmfMode, MediaEncryption, SipAccount, TransportProtocol};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{
    account_label, account_status_label, codec_label, empty_state, field_label, info_hint,
    text_edit_scope,
};
use crate::theme::{self, Palette};

use super::{optional_text_field, optional_text_field_sized};

impl DeelipApp {
    /// Restart required -- returns whether anything changed.
    pub(super) fn show_account_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new("Accounts").font(theme::font_heading(13.5)));
            info_hint(ui, palette, "Each enabled account registers independently on its own \
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
            ui, palette, is_registered(&self.config.accounts[self.edit_account_idx]),
            &format!("{}. {}", self.edit_account_idx + 1, account_label(&self.config.accounts[self.edit_account_idx])),
        );
        egui::ComboBox::from_id_source("settings_account_picker")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
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

            edited |= ui.checkbox(&mut account.enabled, "Enabled (register this account on next restart)").changed();
            edited |= ui.checkbox(&mut account.dnd, "Do Not Disturb (reject all incoming calls)").changed();
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.local_account, "Local Account (serverless, direct-IP calling)").changed();
                info_hint(ui, palette, "Place and receive calls straight to/from an IP address with \
                    no SIP server at all -- no REGISTER is ever sent. Server, Password, Login, and \
                    Transport below are ignored (always plain UDP); dial a bare IP or host[:port] \
                    (e.g. 192.168.1.50 or 192.168.1.50:5060) directly from the dialer. Username/ \
                    Display name are still used as this account's caller-ID identity. Restart required.");
            });
            if account.local_account {
                empty_state(ui, palette, "Local Account: Server/Password/Login/Transport ignored below.");
            }
            ui.add_space(4.0);

            egui::Grid::new("settings_account_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    field_label(ui, palette, "Account name:");
                    edited |= optional_text_field(ui, palette, &mut account.account_name, "e.g. Home, Work");
                    ui.end_row();

                    field_label(ui, palette, "Username:");
                    edited |= text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut account.username)
                        .desired_width(f32::INFINITY)).changed());
                    ui.end_row();

                    field_label(ui, palette, "Password:");
                    ui.horizontal(|ui| {
                        edited |= text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut account.password)
                            .password(!self.show_account_password)
                            .desired_width(200.0)).changed());
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

                    field_label(ui, palette, "Login (optional):");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut account.auth_username, "defaults to Username", 240.0);
                        info_hint(ui, palette, "Digest-auth identity, when a provider requires \
                            a login distinct from the public SIP username above.");
                    });
                    ui.end_row();

                    field_label(ui, palette, "Server:");
                    edited |= text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut account.server)
                        .font(theme::font_address())
                        .desired_width(f32::INFINITY)).changed());
                    ui.end_row();

                    field_label(ui, palette, "Port:");
                    edited |= ui.add(egui::DragValue::new(&mut account.port)).changed();
                    ui.end_row();

                    field_label(ui, palette, "Domain (optional):");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut account.domain, "defaults to Server", 240.0);
                        info_hint(ui, palette, "SIP domain used in From/To/Contact URIs, when it \
                            differs from the registrar host in Server above.");
                    });
                    ui.end_row();

                    field_label(ui, palette, "SIP proxy (optional):");
                    ui.horizontal(|ui| {
                        edited |= optional_text_field_sized(ui, palette, &mut account.sip_proxy, "host[:port]", 240.0);
                        info_hint(ui, palette, "Outbound proxy to actually connect through, \
                            instead of Server/Port directly.");
                    });
                    ui.end_row();

                    field_label(ui, palette, "Display name:");
                    edited |= optional_text_field(ui, palette, &mut account.display_name, "");
                    ui.end_row();

                    field_label(ui, palette, "Transport:");
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
                        info_hint(ui, palette, "Tries UDP, then TCP, then TLS at connect time, \
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
                    ).color(palette.ringing));
                }
            }

            ui.add_space(6.0);
            field_label(ui, palette, "Codecs:");
            let mut to_enable: Option<&str> = None;
            let mut move_up: Option<usize> = None;
            let mut move_down: Option<usize> = None;
            let mut to_disable: Option<usize> = None;
            let list_frame = egui::Frame::none()
                .stroke(egui::Stroke::new(1.0, palette.border))
                .inner_margin(egui::Margin::symmetric(8.0, 6.0));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Available").color(palette.ink_muted).small());
                    list_frame.show(ui, |ui| {
                        // `set_width`, not just `set_min_size` -- a bare
                        // minimum lets a *nested* `right_to_left` layout in
                        // the Enabled column below (see its own comment)
                        // expand to claim the rest of the whole Settings
                        // panel's width instead of staying a tidy column.
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
                    ui.label(RichText::new("Enabled (order = preference)").color(palette.ink_muted).small());
                    list_frame.show(ui, |ui| {
                        // Fixed width, not just a minimum -- see the
                        // Available column's comment above; without this,
                        // the `right_to_left` group below expands to the
                        // whole remaining Settings-panel width instead of
                        // staying right next to the codec name, pushing the
                        // ↑/↓ buttons off past the edge of this column.
                        ui.set_width(290.0);
                        ui.set_min_height(120.0);
                        for (i, name) in account.codec_order.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let can_disable = account.codec_order.len() > 1;
                                if ui.add_enabled(can_disable, egui::Button::new(egui_phosphor::regular::ARROW_LEFT).small()).clicked() {
                                    to_disable = Some(i);
                                }
                                ui.label(codec_label(name));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add_enabled(i + 1 < account.codec_order.len(), egui::Button::new("↓").small()).clicked() {
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
            if let Some(name) = to_enable { account.codec_order.push(name.to_string()); edited = true; }
            if let Some(i) = move_up { account.codec_order.swap(i, i - 1); edited = true; }
            if let Some(i) = move_down { account.codec_order.swap(i, i + 1); edited = true; }
            if let Some(i) = to_disable { account.codec_order.remove(i); edited = true; }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Force codec for incoming:");
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
                info_hint(ui, palette, "Negotiates this codec on an incoming call whenever the \
                    caller offers it at all, ignoring the caller's own preference order.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.vad_enabled, "Voice activity detection (comfort noise)").changed();
                info_hint(ui, palette, "During silence, sends occasional comfort-noise packets \
                    instead of continuous audio, and plays synthesized background noise for the \
                    far end's silence instead of dead air. Only takes effect with a non-Opus codec.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "DTMF mode:");
                egui::ComboBox::from_id_source("settings_dtmf_mode")
                    .selected_text(match account.dtmf_mode {
                        DtmfMode::Rfc2833 => "RFC 2833 (RTP telephone-event)",
                        DtmfMode::SipInfo => "SIP INFO",
                        DtmfMode::Inband  => "Inband (audio tone)",
                        DtmfMode::Auto    => "Auto (RFC 2833 if negotiated, else SIP INFO)",
                    })
                    .show_ui(ui, |ui| {
                        edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::Rfc2833, "RFC 2833 (RTP telephone-event)").changed();
                        edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::SipInfo, "SIP INFO").changed();
                        edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::Inband, "Inband (audio tone)").changed();
                        edited |= ui.selectable_value(&mut account.dtmf_mode, DtmfMode::Auto, "Auto (RFC 2833 if negotiated, else SIP INFO)").changed();
                    });
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Forward always:");
                edited |= optional_text_field(ui, palette, &mut account.forward_always, "sip:reception@example.com");
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Forward when busy:");
                edited |= optional_text_field(ui, palette, &mut account.forward_on_busy, "sip:voicemail@example.com");
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Forward if unanswered:");
                edited |= optional_text_field_sized(ui, palette, &mut account.no_answer_forward, "sip:voicemail@example.com", 180.0);
                field_label(ui, palette, "after (s):");
                edited |= ui.add(egui::DragValue::new(&mut account.no_answer_timeout_secs).range(1..=300)).changed();
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.auto_answer_enabled, "Auto-answer incoming calls (intercom mode)").changed();
                info_hint(ui, palette, "Answers any incoming call on this account after the \
                    timer below, regardless of who's calling -- distinct from Auto Answer \
                    (Control Button) below, which only fires on a specific remote paging signal.");
            });
            if account.auto_answer_enabled {
                ui.horizontal(|ui| {
                    field_label(ui, palette, "after (seconds):");
                    edited |= ui.add(egui::DragValue::new(&mut account.auto_answer_secs).range(0..=60)).changed();
                });
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.auto_answer_control_button, "Auto Answer (Control Button)").changed();
                info_hint(ui, palette, "Auto-answer only when the incoming INVITE itself carries a \
                    remote paging/intercom signal (a Call-Info: ...;answer-after=N header, as sent \
                    by door-intercom/paging hardware) -- unlike the timer above, this doesn't fire \
                    on an ordinary call and bypasses DND/forwarding when it does fire.");
            });
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.deny_incoming_control_button, "Deny Incoming (Control Button)").changed();
                info_hint(ui, palette, "Reacts to the same remote paging/intercom signal as Auto \
                    Answer (Control Button) above, but rejects the call instead. Takes priority if \
                    both are somehow enabled.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Voicemail mailbox (MWI):");
                edited |= optional_text_field_sized(ui, palette, &mut account.mailbox, "1000", 100.0);
                info_hint(ui, palette, "Extension/mailbox this account subscribes to for \
                    Message-Waiting-Indicator (MWI) NOTIFY -- new-voicemail count shown as the \
                    badge next to the status bar. Leave blank to skip MWI subscription entirely.");
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.publish_presence, "Publish presence status").changed();
                info_hint(ui, palette, "Publishes this account's own availability (open/closed, \
                    following Do Not Disturb) via PUBLISH -- needs a server with a presence agent \
                    that accepts it. Restart required to apply.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Dialing prefix:");
                edited |= optional_text_field_sized(ui, palette, &mut account.dialing_prefix, "e.g. 9", 60.0);
                info_hint(ui, palette, "Auto-prepended to bare numbers dialed from this account \
                    (e.g. \"9\" for an outside line) -- not applied to a full SIP URI or an \
                    explicit user@host entry. Only used as a fallback when no Dial Plan rule \
                    below matches.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Dial Plan:");
                info_hint(ui, palette, "Ordered regex match/replace rules applied to a bare \
                    dialed number before the Dialing prefix fallback above -- the first enabled \
                    rule whose pattern matches wins. E.g. pattern \"^0(\\d+)$\", replacement \"$1\" \
                    strips a leading trunk-access 0.");
            });
            if account.dial_plan.is_empty() {
                empty_state(ui, palette, "No dial plan rules -- falls back to the prefix above.");
            } else {
                let mut remove_idx = None;
                for (i, rule) in account.dial_plan.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        edited |= ui.checkbox(&mut rule.enabled, "").changed();
                        edited |= text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut rule.pattern)
                            .hint_text(RichText::new("pattern").color(palette.ink_muted))
                            .desired_width(120.0)).changed());
                        field_label(ui, palette, "→");
                        edited |= text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut rule.replacement)
                            .hint_text(RichText::new("replacement").color(palette.ink_muted))
                            .desired_width(100.0)).changed());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
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
                text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut self.dialplan_pattern_input)
                    .hint_text(RichText::new("pattern, e.g. ^0(\\d+)$").color(palette.ink_muted))
                    .desired_width(120.0)));
                field_label(ui, palette, "→");
                text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut self.dialplan_replacement_input)
                    .hint_text(RichText::new("replacement, e.g. $1").color(palette.ink_muted))
                    .desired_width(100.0)));
                if ui.button("Add rule").clicked() && !self.dialplan_pattern_input.trim().is_empty() {
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
                edited |= ui.checkbox(&mut account.hide_caller_id, "Hide caller ID (send Privacy: id)").changed();
                info_hint(ui, palette, "Requests the server withhold your identity from the \
                    callee -- only effective if the server/provider actually honors Privacy: id; \
                    this app can't force it on an uncooperative server.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Register refresh (seconds):");
                edited |= ui.add(egui::DragValue::new(&mut account.register_expires).range(60..=86400)).changed();
                info_hint(ui, palette, "Requested REGISTER Expires -- the server may return a \
                    shorter value, which re-registration timing always honors regardless of this.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut session_timers_on = account.session_timers_enabled;
                if ui.checkbox(&mut session_timers_on, "Session Timers (RFC 4028)").changed() {
                    account.session_timers_enabled = session_timers_on;
                    edited = true;
                }
                info_hint(ui, palette, "Periodic re-INVITE keep-alives so a dead signaling path \
                    (no BYE ever arrives) can still be detected. On by default; disabling sends \
                    no Session-Expires/Min-SE at all.");
            });

            ui.add_space(6.0);
            let mut keepalive_on = account.keepalive_secs.is_some();
            if ui.checkbox(&mut keepalive_on, "NAT keepalive").changed() {
                account.keepalive_secs = if keepalive_on { Some(15) } else { None };
                edited = true;
            }
            if let Some(secs) = &mut account.keepalive_secs {
                ui.horizontal(|ui| {
                    field_label(ui, palette, "every (seconds):");
                    edited |= ui.add(egui::DragValue::new(secs).range(5..=300)).changed();
                    info_hint(ui, palette, "Sends a lone empty packet to the registrar on this \
                        interval, to hold a NAT/firewall's outbound binding open between registrations.");
                });
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Media encryption:");
                egui::ComboBox::from_id_source("settings_media_encryption")
                    .selected_text(match account.media_encryption {
                        MediaEncryption::MatchTransport => "Match transport (default)",
                        MediaEncryption::Disabled => "Disabled",
                        MediaEncryption::Enabled => "Always (SRTP)",
                        MediaEncryption::Zrtp => "ZRTP (experimental)",
                    })
                    .show_ui(ui, |ui| {
                        edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::MatchTransport, "Match transport (default)").changed();
                        edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::Disabled, "Disabled").changed();
                        edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::Enabled, "Always (SRTP)").changed();
                        edited |= ui.selectable_value(&mut account.media_encryption, MediaEncryption::Zrtp, "ZRTP (experimental)").changed();
                    });
            });
            info_hint(ui, palette, "\"Match transport\" offers SRTP exactly when the signaling \
                transport is TLS (today's behavior); the other two are independent of transport. \
                ZRTP is a from-scratch implementation, verified only against itself (two DeeLip \
                instances) in this codebase's own test suite -- not against any other ZRTP client. \
                Not supported in conference calls (falls back to no encryption for the merged call).");

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.video_enabled, "Enable video (H.264)").changed();
                info_hint(ui, palette, "Offers/accepts a video leg (H.264, 640x480 @15fps) \
                    alongside audio for calls on this account. Needs a working camera (see the \
                    Video section below) to send video; you can still receive and view the other \
                    party's video without one. Not supported in conference calls.");
            });

            ui.add_space(6.0);
            field_label(ui, palette, "Public address (optional):");
            ui.horizontal(|ui| {
                edited |= optional_text_field_sized(ui, palette, &mut account.public_address, "e.g. 203.0.113.5", 240.0);
                info_hint(ui, palette, "Overrides the address advertised in Contact/SDP for this \
                    account, instead of the globally STUN-discovered external IP.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut account.allow_ip_rewrite, "Allow IP Rewrite").changed();
                info_hint(ui, palette, "Rewrites the advertised Contact/SDP IP from the \
                    registrar's own received= feedback on each (re-)registration -- a STUN-free \
                    way to self-discover a public address. Ignored while Public address is set.");
            });

            ui.add_space(6.0);
            let mut ice_override_on = account.ice_enabled.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut ice_override_on, "Override global ICE setting for this account").changed() {
                    account.ice_enabled = if ice_override_on { Some(self.config.ice_enabled) } else { None };
                    edited = true;
                }
                info_hint(ui, palette, "Lets this one account use a different ICE setting than \
                    the global one in Network below -- e.g. disable ICE for a local-only PBX \
                    while keeping it on for other accounts.");
            });
            if let Some(ice_on) = &mut account.ice_enabled {
                edited |= ui.checkbox(ice_on, "Use ICE (RFC 8445) for this account").changed();
            }
        });

        if !self.config.accounts.iter().any(|a| a.enabled) {
            ui.label(RichText::new(
                "Warning: no accounts are enabled — DeeLip won't be able to register on restart."
            ).color(palette.ringing));
        }

        edited
    }
}
