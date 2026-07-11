use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{device_picker, empty_state, info_hint};
use crate::strings::t;
use crate::theme::{self, Palette};

use super::SETTINGS_VIEWPORT_NAME;

impl DeelipApp {
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
    pub(super) fn show_video_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new(t("settings.tab_video")).font(theme::font_heading(13.5)));
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
                if ui.button(t("settings.video.refresh_cameras_button")).clicked() {
                    self.spawn_camera_device_scan();
                }
                if self.camera_device_rx.is_some() {
                    ui.label(RichText::new(t("settings.audio.scanning")).color(palette.ink_muted).small());
                }
            });

            ui.horizontal(|ui| {
                edited |= device_picker(ui, "settings_camera_device", &t("settings.video.camera_label"), &mut self.config.audio.camera_device, &cameras);
                info_hint(ui, palette, &t("settings.video.camera_hint"));
            });
            if cameras.is_empty() {
                empty_state(ui, palette, &t("settings.video.no_cameras"));
            }
        });
        edited
    }
}
