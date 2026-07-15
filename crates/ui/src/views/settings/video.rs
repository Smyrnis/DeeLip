use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{device_picker, empty_state, field_label, info_hint};
use crate::strings::t;
use crate::theme::{self, Palette};

use super::SETTINGS_VIEWPORT_NAME;

/// Resolution presets offered in Settings -- I420 requires even width/height
/// in both dimensions, so picking from a fixed list sidesteps ever landing
/// on an invalid custom value.
const RESOLUTION_PRESETS: [(u32, u32, &str); 3] =
    [(320, 240, "320 × 240"), (640, 480, "640 × 480"), (1280, 720, "1280 × 720")];
const FPS_PRESETS: [u32; 4] = [10, 15, 24, 30];
const BITRATE_PRESETS: [(u32, &str); 4] =
    [(250_000, "250 kbps"), (500_000, "500 kbps"), (1_000_000, "1 Mbps"), (2_000_000, "2 Mbps")];

impl DeelipApp {
    /// Same idiom (and same both-viewports wake reasoning) as
    /// `spawn_audio_device_scan`, for camera enumeration.
    fn spawn_camera_device_scan(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx_slot = self.ctx_slot.clone();
        std::thread::spawn(move || {
            let names = deelip_media::video_capture::list_cameras().into_iter().map(|(_, name)| name).collect();
            let _ = tx.send(names);
            if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                ctx.request_repaint_of(egui::ViewportId::from_hash_of(SETTINGS_VIEWPORT_NAME));
            }
        });
        self.settings_ui.camera_device_rx = Some(rx);
    }

    /// Restart required -- returns whether anything changed.
    pub(super) fn show_video_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new(t("settings.tab_video")).font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            if let Some(rx) = &self.settings_ui.camera_device_rx
                && let Ok(result) = rx.try_recv()
            {
                self.settings_ui.camera_device_cache = Some(result);
                self.settings_ui.camera_device_rx = None;
            }
            if self.settings_ui.camera_device_cache.is_none() && self.settings_ui.camera_device_rx.is_none() {
                self.spawn_camera_device_scan();
            }
            let cameras = self.settings_ui.camera_device_cache.clone().unwrap_or_default();

            ui.horizontal(|ui| {
                if ui.button(t("settings.video.refresh_cameras_button")).clicked() {
                    self.spawn_camera_device_scan();
                }
                if self.settings_ui.camera_device_rx.is_some() {
                    ui.label(RichText::new(t("settings.audio.scanning")).color(palette.ink_muted).small());
                }
            });

            ui.horizontal(|ui| {
                edited |= device_picker(
                    ui,
                    "settings_camera_device",
                    &t("settings.video.camera_label"),
                    &mut self.config.audio.camera_device,
                    &cameras,
                );
                info_hint(ui, palette, &t("settings.video.camera_hint"));
            });
            if cameras.is_empty() {
                empty_state(ui, palette, &t("settings.video.no_cameras"));
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.video.resolution_label"));
                let (w, h) = (self.config.audio.video_capture_width, self.config.audio.video_capture_height);
                let selected =
                    RESOLUTION_PRESETS.iter().find(|(pw, ph, _)| (*pw, *ph) == (w, h)).map_or("", |(_, _, l)| l);
                egui::ComboBox::from_id_salt("settings_video_resolution").selected_text(selected).show_ui(ui, |ui| {
                    for (pw, ph, label) in RESOLUTION_PRESETS {
                        if ui.selectable_label(w == pw && h == ph, label).clicked() {
                            self.config.audio.video_capture_width = pw;
                            self.config.audio.video_capture_height = ph;
                            edited = true;
                        }
                    }
                });
                info_hint(ui, palette, &t("settings.restart_to_apply_hint"));
            });

            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.video.fps_label"));
                let fps = self.config.audio.video_fps;
                egui::ComboBox::from_id_salt("settings_video_fps").selected_text(fps.to_string()).show_ui(ui, |ui| {
                    for preset in FPS_PRESETS {
                        if ui.selectable_label(fps == preset, preset.to_string()).clicked() {
                            self.config.audio.video_fps = preset;
                            edited = true;
                        }
                    }
                });
            });

            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.video.bitrate_label"));
                let bitrate = self.config.audio.video_bitrate_bps;
                let selected = BITRATE_PRESETS.iter().find(|(v, _)| *v == bitrate).map_or("", |(_, l)| l);
                egui::ComboBox::from_id_salt("settings_video_bitrate").selected_text(selected).show_ui(ui, |ui| {
                    for (value, label) in BITRATE_PRESETS {
                        if ui.selectable_label(bitrate == value, label).clicked() {
                            self.config.audio.video_bitrate_bps = value;
                            edited = true;
                        }
                    }
                });
            });
        });
        edited
    }
}
