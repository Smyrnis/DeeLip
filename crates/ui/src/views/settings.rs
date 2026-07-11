use deelip_config::{
    DefaultListAction, DialPlanRule, DtmfMode, MediaEncryption, RecordingFormat, SipAccount,
    TransportProtocol, UpdateCheckFrequency,
};
use egui::{RichText, Ui};

use crate::app::{DeelipApp, SettingsTab, SharedApp};
use crate::helpers::{
    account_label, account_status_label, codec_label, device_picker, empty_state, field_label,
    info_hint, settings_section, styled_slider, text_edit_scope, window_icon,
};
use crate::theme::{self, Palette};

/// Shared between `show_settings_modal` (which opens the viewport under
/// this id) and the background device-scan spawns below (which need the
/// same id to wake *this* viewport specifically, not just `ROOT` -- see
/// their own doc comments for why waking only `ROOT` left this window
/// showing stale "Scanning..." text until the user happened to interact
/// with it directly).
const SETTINGS_VIEWPORT_NAME: &str = "deelip_settings_window";

impl DeelipApp {
    /// Settings as a separate, genuinely movable native OS window rather
    /// than a tab (MicroSIP-style "phone window + separate settings window"
    /// split -- see `app.rs`'s `settings_open` doc comment). No-op when
    /// closed.
    ///
    /// This used to be an `egui::Window` (a floating panel drawn *inside*
    /// the main app's own OS window canvas) plus a hand-rolled dimming
    /// backdrop faking modality -- it looked like a separate window but was
    /// mechanically trapped inside the main window's bounds, unable to be
    /// dragged out to a different part of the screen (a real user-reported
    /// bug: "the settings window is inside the initial deelip window, and i
    /// can not move it"). `Context::show_viewport_deferred` creates an
    /// actual second native window (its own OS-level title bar, move,
    /// resize, and close -- not an egui-drawn imitation of one), which is
    /// what a "separate window" needs to mean here.
    ///
    /// `Deferred`, not `Immediate` -- this used to be `Immediate`, which
    /// renders synchronously nested inside the *main* window's own per-tick
    /// callback (confirmed against `eframe`'s own source: an `Immediate`
    /// child viewport has no redraw path of its own, it only ever repaints
    /// when its parent's tick runs). That's what made Settings feel slow
    /// whenever the main window was unfocused (which it always is while
    /// Settings itself has focus): GNOME/Mutter throttles frame delivery for
    /// whichever of the two windows isn't focused down to roughly 1Hz
    /// (confirmed live, independent of whether the windows visually
    /// overlap), and since Settings was nested inside the main window's own
    /// callback, every click/keystroke inside Settings was gated by that
    /// same ~1s throttle on the *main* window's redraw. `Deferred` viewports
    /// get their own independent redraw path (`eframe` invokes their stored
    /// callback directly whenever *their* window needs a repaint, not the
    /// main window's), so Settings now responds to its own input normally
    /// regardless of the main window's focus/throttle state. This is why
    /// `DeelipApp` is wrapped in `SharedApp` (`Arc<Mutex<_>>`) -- a
    /// `Deferred` callback must be `Fn + Send + Sync + 'static`, so it can't
    /// directly borrow `&mut self` the way the old `Immediate` closure did;
    /// it locks the shared app instead. Called every frame while
    /// `settings_open` is true, same lifecycle as before (egui's viewport
    /// model is still declarative, not create-once-and-forget).
    pub(crate) fn show_settings_modal(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        if !self.settings_open {
            return;
        }

        // Some backends (or a compositor without multi-window support)
        // can't actually open a second native window -- `embed_viewports()`
        // reports that, in which case both `show_viewport_immediate` and
        // `show_viewport_deferred` fall back to running their callback
        // *synchronously*, right here, against the main window's own
        // context. Deciding this up front (rather than branching on
        // `ViewportClass::Embedded` from inside the deferred closure, as
        // this used to) matters now: if we called `show_viewport_deferred`
        // below on a backend that embeds, its closure would run inline in
        // this same call, and locking `self_arc` there would deadlock
        // against the lock this method's own caller already holds. Render
        // the fallback directly against `self` instead -- no lock needed.
        if ctx.embed_viewports() {
            let mut open = true;
            egui::Window::new("Settings")
                .id(egui::Id::new("settings_window_fallback"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([500.0, 640.0])
                .min_width(380.0)
                .show(ctx, |ui| self.show_settings(ui));
            if !open {
                self.settings_open = false;
            }
            return;
        }

        let viewport_id = egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME);
        ctx.show_viewport_deferred(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title("DeeLip Settings")
                // Sized so every tab except Account (which scrolls
                // internally -- see its own `SettingsTab::Account` match
                // arm's comment) fits without scrolling at all -- confirmed
                // live via Xvfb across all 8 tabs, not guessed. Bumped
                // taller (700 -> 740) alongside the v3.1 spacing/margin
                // loosening in `theme.rs`/`frame.rs` -- needs the same
                // live re-check as before once that's verified.
                .with_inner_size([950.0, 740.0])
                .with_min_inner_size([580.0, 520.0])
                .with_icon(window_icon()),
            move |child_ctx, _class| {
                let mut app = self_app.lock();
                if !app.settings_open {
                    return;
                }

                // TEMP diagnostic -- see `diag_settings_viewport_last_frame`'s
                // doc comment. Measures the gap between *this* viewport's own
                // successive redraws, independent of the main window's --
                // the thing the `Immediate` -> `Deferred` switch is meant to
                // fix. Remove alongside that field once confirmed live.
                let __diag_now = std::time::Instant::now();
                if let Some(last) = app.diag_settings_viewport_last_frame {
                    tracing::info!("__diag settings viewport update() gap: {:?}", __diag_now.duration_since(last));
                }
                app.diag_settings_viewport_last_frame = Some(__diag_now);

                // Explicit margin, not `CentralPanel::default()`'s bare
                // default -- confirmed live (Xvfb, at this viewport's real
                // 950px default width) that content rendered flush against
                // the literal right edge with zero breathing room. Same
                // value/reasoning as the main window's own central panel
                // (`frame.rs`).
                let central_frame = egui::Frame::central_panel(&child_ctx.style()).inner_margin(14.0);
                egui::CentralPanel::default().frame(central_frame).show(child_ctx, |ui| {
                    // No in-app Close button -- removed per explicit request
                    // (the user's real desktop always has working title-bar
                    // decorations, so it was redundant); relies solely on
                    // `close_requested()` below now. A window-manager-less
                    // environment with no decorations at all (e.g. this
                    // project's own Xvfb live-verification sandbox) has no
                    // *visible* way to close this window short of that same
                    // OS-level close-request path -- still reachable there
                    // via a synthetic request (e.g. `xdotool windowclose`),
                    // just not from a click.
                    ui.label(RichText::new("Settings").font(theme::font_heading(16.0)));
                    ui.separator();
                    app.show_settings(ui);
                });
                if child_ctx.input(|i| i.viewport().close_requested()) {
                    app.settings_open = false;
                }
            },
        );
    }

    /// Renders every Settings section in order inside the scroll area, then
    /// the trailing Save button. Each section is its own method (see below)
    /// -- this is just the scaffolding: `edited` accumulates whether any
    /// restart-required field changed (the "applies immediately" sections
    /// save themselves as they go and don't feed into it).
    fn show_settings(&mut self, ui: &mut Ui) {
        if self.config.accounts.is_empty() {
            self.config
                .accounts
                .push(deelip_config::SipAccount::default());
        }
        self.edit_account_idx = self.edit_account_idx.min(self.config.accounts.len() - 1);
        let palette = self.palette;

        // MicroSIP-style tab strip -- one section visible at a time, sized
        // to fit without scrolling, instead of the previous single long
        // `ScrollArea` stacking all 12 sections in one column.
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.settings_tab, SettingsTab::General, "General");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Account, "Account");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Audio, "Audio");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Video, "Video");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Network, "Network");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Directory, "Directory");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Hotkeys, "Hotkeys");
            ui.selectable_value(&mut self.settings_tab, SettingsTab::Advanced, "Advanced");
        });
        ui.separator();
        ui.add_space(6.0);

        // Reserved *before* the tab content below -- `ScrollArea::vertical()`
        // (used by the Account tab) greedily fills all remaining space in
        // its parent, so a naive "content, then Save button" ordering left
        // the Save button pushed below the visible window whenever a tab's
        // content scrolled (caught live, not by reading the code: Account's
        // Save button was simply gone from the screenshot). Anchoring Save
        // to the bottom *first* means whatever's left for the tab content
        // (and therefore the Account `ScrollArea` inside it) already
        // excludes this panel's own height.
        egui::TopBottomPanel::bottom("settings_save_panel").show_inside(ui, |ui| {
            ui.add_space(8.0);
            if ui.button("Save").clicked() {
                match self.config.save(&self.db) {
                    Ok(()) => self.settings_saved_notice = true,
                    Err(err) => {
                        self.settings_saved_notice = false;
                        tracing::error!("Failed to save config: {err}");
                    }
                }
            }
            if self.settings_saved_notice {
                ui.label(
                    RichText::new("Saved — restart DeeLip to apply changes.").color(palette.signal),
                );
            }
            ui.add_space(4.0);
        });

        let __diag_content_start = std::time::Instant::now();
        let edited = match self.settings_tab {
            SettingsTab::General => {
                self.show_notifications_section(ui, &palette);
                ui.add_space(14.0);
                self.show_call_handling_section(ui, &palette);
                ui.add_space(14.0);
                self.show_startup_section(ui, &palette)
            }
            // The one exception to "no scrolling" -- confirmed live (this
            // section's content still doesn't fit even at ~1400px tall,
            // an unreasonable window height) that Account is too dense to
            // ever fit a real dialog without one, even after pulling
            // several stacked label+field rows into single rows above.
            // Scrolling just this tab beats silently clipping its content,
            // which is what removing the outer `ScrollArea` entirely would
            // otherwise do.
            SettingsTab::Account => {
                let mut edited = false;
                egui::ScrollArea::vertical()
                    .id_source("account_tab_scroll")
                    .show(ui, |ui| {
                        edited = self.show_account_section(ui, &palette);
                    });
                edited
            }
            SettingsTab::Audio => self.show_audio_section(ui, &palette),
            SettingsTab::Video => self.show_video_section(ui, &palette),
            SettingsTab::Network => self.show_network_section(ui, &palette),
            SettingsTab::Directory => self.show_directory_section(ui, &palette),
            SettingsTab::Hotkeys => self.show_global_hotkeys_section(ui, &palette),
            // Same "doesn't fit, scroll just this tab" exception as Account
            // above -- confirmed live that its 4 stacked sections (Updates/
            // Blocklist/Call History/Contacts Import-Export, the latter two
            // added in a later session than the comment above was written)
            // overflow past the window's bottom, taking the Save button
            // with them.
            SettingsTab::Advanced => {
                egui::ScrollArea::vertical()
                    .id_source("advanced_tab_scroll")
                    .show(ui, |ui| {
                        self.show_updates_section(ui, &palette);
                        ui.add_space(14.0);
                        self.show_blocklist_section(ui, &palette);
                        ui.add_space(14.0);
                        self.show_history_export_section(ui, &palette);
                        ui.add_space(14.0);
                        self.show_contacts_data_section(ui, &palette);
                    });
                false
            }
        };
        tracing::info!("__diag settings tab {:?} content: {:?}", self.settings_tab, __diag_content_start.elapsed());

        if edited {
            self.settings_saved_notice = false;
        }
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    fn show_notifications_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Notifications & Ringtone", Some("Applies immediately — no restart needed."), |ui| {
            if ui.checkbox(&mut self.config.notifications_enabled, "Desktop notification on incoming calls").changed() {
                self.save_config_quietly();
            }
            if ui.checkbox(&mut self.config.ringtone_enabled, "Ringtone (incoming) / ringback (outgoing)").changed() {
                self.save_config_quietly();
            }
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.config.random_popup_position, "Random popup position").changed() {
                    self.save_config_quietly();
                }
                info_hint(ui, palette, "Show the main window at a random spot on the current \
                    monitor each time it's raised for an incoming call, instead of wherever it \
                    last was.");
            });
            ui.horizontal(|ui| {
                field_label(ui, palette, "Default list action:");
                egui::ComboBox::from_id_source("settings_default_list_action")
                    .selected_text(match self.config.default_list_action {
                        DefaultListAction::Call => "Call",
                        DefaultListAction::Message => "Message",
                        DefaultListAction::Edit => "Edit",
                    })
                    .show_ui(ui, |ui| {
                        for (val, label) in [
                            (DefaultListAction::Call, "Call"),
                            (DefaultListAction::Message, "Message"),
                            (DefaultListAction::Edit, "Edit"),
                        ] {
                            if ui.selectable_value(&mut self.config.default_list_action, val, label).changed() {
                                self.save_config_quietly();
                            }
                        }
                    });
                info_hint(ui, palette, "What double-clicking a row's name/number in History or \
                    Contacts does. \"Edit\" falls back to \"Call\" in History (nothing to edit there).");
            });
        });
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    fn show_call_handling_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Call Handling", Some("Applies immediately — no restart needed."), |ui| {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.config.single_call_mode, "Single Call Mode (disable call waiting)").changed() {
                    self.save_config_quietly();
                }
                info_hint(ui, palette, "An incoming call while another is already active is \
                    rejected with Busy instead of ringing as a 2nd call. A per-account \
                    \"Forward on busy\" (Account editor) still takes priority over this.");
            });
        });
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    fn show_blocklist_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Blocklist", Some("Applies immediately — no restart needed."), |ui| {
            ui.horizontal(|ui| {
                text_edit_scope(ui, palette, |ui| ui.add(egui::TextEdit::singleline(&mut self.blocklist_input)
                    .hint_text(RichText::new("number or sip:user@host").color(palette.ink_muted))
                    .desired_width(200.0)));
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
                empty_state(ui, palette, "No blocked numbers.");
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
    }

    /// Moved here from History's own search bar -- see the redesign plan's
    /// "Settings: History export + Contacts import/export" section.
    fn show_history_export_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Call History", None, |ui| {
            ui.horizontal(|ui| {
                field_label(ui, palette, "Export call history to CSV");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export…").clicked() {
                        self.export_history_csv();
                    }
                });
            });
        });
    }

    /// Moved here from Contacts' own search bar -- see the redesign plan's
    /// "Settings: History export + Contacts import/export" section.
    fn show_contacts_data_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Contacts Import / Export", None, |ui| {
            ui.horizontal(|ui| {
                field_label(ui, palette, "Import from CSV or vCard");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Import…").clicked() {
                        self.import_contacts();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Export as CSV");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export…").clicked() {
                        self.export_contacts_csv();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Export as vCard");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export…").clicked() {
                        self.export_contacts_vcard();
                    }
                });
            });
        });
    }

    /// Restart required -- returns whether anything changed.
    fn show_startup_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        settings_section(ui, palette, "Startup", None, |ui| {
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.start_minimized, "Start minimized (to tray)").changed();
                info_hint(ui, palette, "Restart to apply.");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.log_to_file, "Enable log file").changed();
                info_hint(ui, palette, "Also writes logs to ~/.config/deelip/deelip.log, \
                    in addition to the console. Restart to apply.");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.crash_reporting_enabled, "Save crash reports locally").changed();
                info_hint(ui, palette, "If DeeLip crashes, save a report (version, panic message, \
                    backtrace) to ~/.config/deelip/crashes/ for troubleshooting. Purely local -- \
                    never uploaded or sent anywhere. Restart to apply.");
            });
            ui.horizontal(|ui| {
                if ui.button("Open crash reports folder").clicked() {
                    if let Ok(dir) = deelip_config::crashes_dir() {
                        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.autostart_enabled, "Start DeeLip on login").changed() {
                    if let Err(e) = deelip_config::set_autostart(self.autostart_enabled) {
                        tracing::error!("Failed to update autostart: {e}");
                        self.autostart_enabled = deelip_config::is_autostart_enabled();
                    }
                }
                info_hint(ui, palette, "Applies immediately — no restart needed.");
            });
        });
        edited
    }

    /// Applies immediately -- no restart needed, saves itself on change.
    fn show_updates_section(&mut self, ui: &mut Ui, palette: &Palette) {
        settings_section(ui, palette, "Updates", Some("Applies immediately — no restart needed."), |ui| {
            ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(palette.ink_muted));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Check for updates:");
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
                info_hint(ui, palette, "Only works for a portable (tar.gz/install.sh) install -- \
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
                ui.label(RichText::new(status).color(palette.ink_muted).small());
            });
        });
    }

    /// Restart required -- returns whether anything changed.
    fn show_account_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
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
                        .font(egui::FontId::new(13.0, egui::FontFamily::Monospace))
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

    /// Kicks off cpal device enumeration on a background thread instead of
    /// blocking the render thread -- measured ~620ms on first Audio-tab
    /// visit live (PulseAudio backend), which froze the whole app (main
    /// window included -- both it and the Settings viewport are driven by
    /// this same thread) for that long right as the tab was switched. See
    /// `audio_device_rx`'s doc comment.
    ///
    /// Wakes both `ROOT` and the Settings viewport specifically: this scan
    /// only ever runs while Settings is open (see the two call sites in
    /// `show_audio_section`), and `ROOT` alone doesn't repaint a `Deferred`
    /// child viewport -- confirmed live, this left the "Scanning..." label
    /// stuck showing stale text after the scan had already finished, until
    /// the user happened to move the mouse over the Settings window.
    fn spawn_audio_device_scan(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx_slot = self.ctx_slot.clone();
        std::thread::spawn(move || {
            let _ = tx.send((list_device_names(true), list_device_names(false)));
            if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                ctx.request_repaint_of(egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME));
            }
        });
        self.audio_device_rx = Some(rx);
    }

    /// Same idiom (and same both-viewports wake reasoning) as
    /// `spawn_audio_device_scan`, for camera enumeration.
    fn spawn_camera_device_scan(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx_slot = self.ctx_slot.clone();
        std::thread::spawn(move || {
            let names = deelip_media::video_capture::list_cameras()
                .into_iter()
                .map(|(_, name)| name)
                .collect();
            let _ = tx.send(names);
            if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                ctx.request_repaint_of(egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME));
            }
        });
        self.camera_device_rx = Some(rx);
    }

    /// Restart required -- returns whether anything changed.
    fn show_audio_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new("Audio").font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            if let Some(rx) = &self.audio_device_rx {
                if let Ok(result) = rx.try_recv() {
                    self.audio_device_cache = Some(result);
                    self.audio_device_rx = None;
                }
            }
            if self.audio_device_cache.is_none() && self.audio_device_rx.is_none() {
                self.spawn_audio_device_scan();
            }
            let (input_names, output_names) = self.audio_device_cache.clone().unwrap_or_default();

            ui.horizontal(|ui| {
                if ui.button("Refresh device list").clicked() {
                    self.spawn_audio_device_scan();
                }
                if self.audio_device_rx.is_some() {
                    ui.label(RichText::new("Scanning…").color(palette.ink_muted).small());
                }
            });

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
                ui.label(RichText::new("Ringing device").color(palette.ink_muted).small());
                info_hint(ui, palette, "Independent of the Output device above -- lets the \
                    ringtone play on a different device than call audio, e.g. ring on \
                    speakers, talk on a headset.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, "Custom ringtone (WAV):");
                let name = self.config.audio.ringtone_file.as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Built-in tone".into());
                ui.label(RichText::new(name).color(palette.ink_muted));
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
                field_label(ui, palette, "Ringtone volume:");
                edited |= styled_slider(ui, palette, egui::Slider::new(&mut self.config.audio.ringtone_volume, 0.0..=2.0)
                    .fixed_decimals(2)).changed();
            });

            ui.add_space(6.0);
            edited |= ui.checkbox(&mut self.config.audio.echo_cancellation, "Echo cancellation").changed();
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.audio.agc_enabled, "Automatic microphone gain control").changed();
                info_hint(ui, palette, "Adaptively boosts a quiet mic signal and limits a loud one.");
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.recording_enabled, "Record calls").changed();
                info_hint(ui, palette, "Saves locally to ~/.config/deelip/recordings by default \
                    (see Save to below) -- never uploaded anywhere. Check your local laws on \
                    call-recording consent before enabling.");
            });
            if self.config.recording_enabled {
                ui.horizontal(|ui| {
                    field_label(ui, palette, "Format:");
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
                    field_label(ui, palette, "Save to:");
                    let shown = self.config.recordings_dir_override.as_deref()
                        .unwrap_or("~/.config/deelip/recordings (default)");
                    ui.label(RichText::new(shown).color(palette.ink_muted));
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
        edited
    }

    /// Restart required -- returns whether anything changed.
    fn show_video_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new("Video").font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            if let Some(rx) = &self.camera_device_rx {
                if let Ok(result) = rx.try_recv() {
                    self.camera_device_cache = Some(result);
                    self.camera_device_rx = None;
                }
            }
            if self.camera_device_cache.is_none() && self.camera_device_rx.is_none() {
                self.spawn_camera_device_scan();
            }
            let cameras = self.camera_device_cache.clone().unwrap_or_default();

            ui.horizontal(|ui| {
                if ui.button("Refresh camera list").clicked() {
                    self.spawn_camera_device_scan();
                }
                if self.camera_device_rx.is_some() {
                    ui.label(RichText::new("Scanning…").color(palette.ink_muted).small());
                }
            });

            ui.horizontal(|ui| {
                edited |= device_picker(ui, "settings_camera_device", "Camera:", &mut self.config.audio.camera_device, &cameras);
                info_hint(ui, palette, "Only affects outgoing video -- receiving and displaying \
                    the other party's video works with no camera at all. Has no effect unless \
                    Enable video (H.264) is also on for the account placing/answering the call.");
            });
            if cameras.is_empty() {
                empty_state(ui, palette, "No cameras detected -- video calls will still \
                    receive and display the other party's video without one.");
            }
        });
        edited
    }

    /// Restart required -- returns whether anything changed.
    fn show_network_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
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

    /// Restart required -- returns whether anything changed.
    fn show_directory_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
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

    /// Restart required -- returns whether anything changed.
    fn show_global_hotkeys_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new("Global Hotkeys").font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.global_hotkeys_enabled,
                    "Enable system-wide Answer/Hangup/Mute hotkeys (Linux: X11 only)"
                ).changed();
                info_hint(ui, palette, "Format: \"Ctrl+Alt+A\" style. Restart required to apply.");
            });
            if self.config.global_hotkeys_enabled {
                egui::Grid::new("hotkeys_grid").num_columns(2).show(ui, |ui| {
                    field_label(ui, palette, "Answer:");
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_answer).changed();
                    ui.end_row();
                    field_label(ui, palette, "Hangup:");
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_hangup).changed();
                    ui.end_row();
                    field_label(ui, palette, "Mute:");
                    edited |= ui.text_edit_singleline(&mut self.config.hotkey_mute).changed();
                    ui.end_row();
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.handle_media_buttons,
                    "Handle Media Buttons (headset Play/Pause answers/hangs up)"
                ).changed();
                info_hint(ui, palette, "Independent of the toggle above -- grabs the hardware \
                    media Play/Pause key (Linux: X11 only) to answer a ringing call or hang up \
                    the active one, like a headset's hook button. Restart required to apply.");
            });
        });
        edited
    }
}

/// List available cpal device names (input or output), for populating the
/// Settings device pickers. Excludes ALSA pseudo-devices that are never a
/// sensible choice for a phone call -- see `is_irrelevant_alsa_device`.
fn list_device_names(input: bool) -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let devices = if input {
        host.input_devices()
    } else {
        host.output_devices()
    };
    match devices {
        Ok(devices) => devices
            .filter_map(|d| d.name().ok())
            .filter(|name| !is_irrelevant_alsa_device(name))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Excludes ALSA's multi-channel surround (`surround21`/`surround40`/...)
/// and digital-passthrough (`iec958`/`spdif`) pseudo-devices from the
/// Settings device pickers -- real, valid ALSA PCM configurations that cpal
/// correctly enumerates, but never a sensible choice for a phone call's
/// mono/stereo mic or speaker, and their jargon-heavy names (e.g.
/// `"surround50:CARD=Generic,DEV=0"`) are meaningless to a non-technical
/// user picking a device. `Default` and real hardware/plugin devices
/// (`hw:...`, `front`, `pulse`, etc.) are left untouched.
fn is_irrelevant_alsa_device(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("surround") || lower.starts_with("iec958") || lower.starts_with("spdif")
}

/// Text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_text_field(ui: &mut Ui, palette: &Palette, value: &mut Option<String>, hint: &str) -> bool {
    optional_text_field_sized(ui, palette, value, hint, f32::INFINITY)
}

/// Same as `optional_text_field`, but with a caller-chosen width instead of
/// always filling the rest of the row -- for a row that needs to fit
/// something else (a trailing label/control) after the field.
fn optional_text_field_sized(
    ui: &mut Ui,
    palette: &Palette,
    value: &mut Option<String>,
    hint: &str,
    width: f32,
) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = text_edit_scope(ui, palette, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(RichText::new(hint).color(palette.ink_muted))
                .desired_width(width),
        )
        .changed()
    });
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

/// Masked text field bound to an `Option<String>` — an empty field maps to `None`.
fn optional_password_field(ui: &mut Ui, palette: &Palette, value: &mut Option<String>) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = text_edit_scope(ui, palette, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut text)
                .password(true)
                .desired_width(f32::INFINITY),
        )
        .changed()
    });
    if changed {
        *value = if text.is_empty() { None } else { Some(text) };
    }
    changed
}

#[cfg(test)]
#[path = "../../tests/unit/settings.rs"]
mod tests;
