use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::strings::{t, tf};

/// Where the startup update check currently stands. Drives both the popup
/// (`show_update_popup`) and, when `config.auto_update_enabled` is on, the
/// automatic download -- see `process_update_events`.
pub(crate) enum UpdateState {
    /// Nothing to show -- either still checking, already up to date, the
    /// check failed, or the user dismissed this version.
    Idle,
    Checking,
    /// A newer version exists; `deelip_updater::can_self_replace()` decides
    /// whether the popup offers "Update Now" or just a link to the release.
    Available(deelip_updater::ReleaseInfo),
    Downloading,
    /// Installed on disk; takes effect on the next launch (never applied
    /// automatically -- see `do_restart_to_update`'s doc comment).
    Updated(String),
    Failed(String),
}

pub(crate) enum UpdateMsg {
    Checked(anyhow::Result<Option<deelip_updater::ReleaseInfo>>),
    Installed(anyhow::Result<()>, String),
}

impl DeelipApp {
    /// Kicks off a one-shot background check against GitHub Releases.
    /// Called once at startup (see `DeelipApp::new`, gated on
    /// `UpdateCheckFrequency::is_due`) and from the Settings tab's manual
    /// "Check for updates now" button (never gated) -- not polled again
    /// during the session, matching "tell me when the app opens", not a
    /// recurring background poll.
    pub(crate) fn start_update_check(&mut self) {
        self.config.last_update_check = Some(crate::helpers::unix_now());
        self.save_config_quietly();
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_state = UpdateState::Checking;
        let ctx_slot = self.ctx_slot.clone();
        std::thread::spawn(move || {
            let result = deelip_updater::check_latest(env!("CARGO_PKG_VERSION"));
            let _ = tx.send(UpdateMsg::Checked(result));
            if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }

    /// Drains the update-check/download channel, called once per frame.
    pub(crate) fn process_update_events(&mut self) {
        // Collect first, same reason as `process_sip_events`: a message can
        // itself need to spawn a new channel/thread (replacing
        // `self.update_rx`), which can't happen while `rx` still borrows it.
        let Some(rx) = &self.update_rx else { return };
        let messages: Vec<UpdateMsg> = rx.try_iter().collect();
        for msg in messages {
            match msg {
                UpdateMsg::Checked(Ok(Some(release))) => {
                    if self.config.update_skip_version.as_deref() == Some(release.version.as_str())
                    {
                        self.update_state = UpdateState::Idle;
                    } else if self.config.auto_update_enabled && deelip_updater::can_self_replace()
                    {
                        self.start_update_download(release);
                    } else {
                        self.update_state = UpdateState::Available(release);
                    }
                }
                UpdateMsg::Checked(Ok(None)) => self.update_state = UpdateState::Idle,
                UpdateMsg::Checked(Err(e)) => {
                    tracing::debug!("Update check failed (ignoring): {e:#}");
                    self.update_state = UpdateState::Idle;
                }
                UpdateMsg::Installed(Ok(()), version) => {
                    self.update_state = UpdateState::Updated(version)
                }
                UpdateMsg::Installed(Err(e), _) => {
                    tracing::warn!("Auto-update failed: {e:#}");
                    self.update_state = UpdateState::Failed(e.to_string());
                }
            }
        }
    }

    /// Downloads and installs `release` in the background, updating
    /// `update_state` to `Downloading` immediately and `Updated`/`Failed`
    /// once it's done. Only meaningful when `can_self_replace()` is true --
    /// callers must check that first (both call sites below already do).
    pub(crate) fn start_update_download(&mut self, release: deelip_updater::ReleaseInfo) {
        self.update_state = UpdateState::Downloading;
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        let ctx_slot = self.ctx_slot.clone();
        std::thread::spawn(move || {
            let version = release.version.clone();
            let result = deelip_updater::download_and_replace(&release);
            let _ = tx.send(UpdateMsg::Installed(result, version));
            if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }

    /// Relaunches the just-updated binary and exits this process. Refuses
    /// while any call is active/ringing/dialing -- exiting would drop it --
    /// the button that calls this is disabled in that case (see
    /// `show_update_popup`) so this is a last-resort guard, not the primary one.
    fn do_restart_to_update(&mut self) {
        if !self.calls.is_empty() || self.pending_call.is_some() || self.pending_outbound.is_some()
        {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if std::process::Command::new(exe).spawn().is_ok() {
            std::process::exit(0);
        }
    }

    /// Small popup shown on top of whatever tab is active -- called once
    /// per frame from `update()`. No-op while `update_state` is `Idle`/`Checking`.
    pub(crate) fn show_update_popup(&mut self, ctx: &egui::Context) {
        let call_active = !self.calls.is_empty()
            || self.pending_call.is_some()
            || self.pending_outbound.is_some();

        match &self.update_state {
            UpdateState::Idle | UpdateState::Checking => {}
            UpdateState::Available(release) => {
                let version = release.version.clone();
                let html_url = release.html_url.clone();
                let can_self_replace = deelip_updater::can_self_replace();
                let mut update_clicked = false;
                let mut skip_clicked = false;
                let mut later_clicked = false;
                egui::Window::new(t("update.window_available"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
                    .show(ctx, |ui: &mut Ui| {
                        ui.label(tf(
                            "update.available_message",
                            &[("version", &version), ("current", env!("CARGO_PKG_VERSION"))],
                        ));
                        if !can_self_replace {
                            ui.label(RichText::new(
                                t("update.cant_auto_update")
                            ).small());
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if can_self_replace && ui.button(t("update.update_now_button")).clicked() {
                                update_clicked = true;
                            }
                            if ui.button(t("update.view_release_button")).clicked() {
                                let _ = std::process::Command::new("xdg-open")
                                    .arg(&html_url)
                                    .spawn();
                            }
                            if ui.button(t("update.skip_version_button")).clicked() {
                                skip_clicked = true;
                            }
                            if ui.button(t("update.later_button")).clicked() {
                                later_clicked = true;
                            }
                        });
                    });
                if update_clicked {
                    self.start_update_download(release.clone());
                } else if skip_clicked {
                    self.config.update_skip_version = Some(version);
                    self.save_config_quietly();
                    self.update_state = UpdateState::Idle;
                } else if later_clicked {
                    self.update_state = UpdateState::Idle;
                }
            }
            UpdateState::Downloading => {
                egui::Window::new(t("update.window_available"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t("update.downloading_update"));
                        });
                    });
            }
            UpdateState::Updated(version) => {
                let version = version.clone();
                let mut restart_clicked = false;
                let mut later_clicked = false;
                egui::Window::new(t("update.window_installed"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
                    .show(ctx, |ui| {
                        ui.label(tf("update.updated_message", &[("version", &version)]));
                        if call_active {
                            ui.label(RichText::new(t("update.finish_call_first")).small());
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!call_active, egui::Button::new(t("update.restart_now_button")))
                                .clicked()
                            {
                                restart_clicked = true;
                            }
                            if ui.button(t("update.later_button")).clicked() {
                                later_clicked = true;
                            }
                        });
                    });
                if restart_clicked {
                    self.do_restart_to_update();
                } else if later_clicked {
                    self.update_state = UpdateState::Idle;
                }
            }
            UpdateState::Failed(reason) => {
                let reason = reason.clone();
                let mut dismiss_clicked = false;
                egui::Window::new(t("update.window_failed"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
                    .show(ctx, |ui| {
                        ui.label(RichText::new(&reason).small());
                        ui.add_space(6.0);
                        if ui.button(t("update.dismiss_button")).clicked() {
                            dismiss_clicked = true;
                        }
                    });
                if dismiss_clicked {
                    self.update_state = UpdateState::Idle;
                }
            }
        }
    }
}
