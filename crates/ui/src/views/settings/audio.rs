use deelip_config::RecordingFormat;
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{device_picker, field_label, info_hint, styled_slider};
use crate::strings::t;
use crate::theme::{self, Palette};

use super::SETTINGS_VIEWPORT_NAME;

impl DeelipApp {
    /// Kicks off cpal device enumeration on a background thread -- see
    /// `docs/crates/ui.md`'s Settings section for why (blocks the render thread
    /// for hundreds of ms) and why it wakes both `ROOT` and the Settings
    /// viewport by name.
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
        self.settings_ui.audio_device_rx = Some(rx);
    }

    /// Restart required -- returns whether anything changed.
    pub(super) fn show_audio_section(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let mut edited = false;
        ui.label(RichText::new(t("settings.tab_audio")).font(theme::font_heading(13.5)));
        theme::full_width_card(ui, *palette, |ui| {
            if let Some(rx) = &self.settings_ui.audio_device_rx
                && let Ok(result) = rx.try_recv()
            {
                self.settings_ui.audio_device_cache = Some(result);
                self.settings_ui.audio_device_rx = None;
            }
            if self.settings_ui.audio_device_cache.is_none() && self.settings_ui.audio_device_rx.is_none() {
                self.spawn_audio_device_scan();
            }
            let (input_names, output_names) = self.settings_ui.audio_device_cache.clone().unwrap_or_default();

            ui.horizontal(|ui| {
                if ui.button(t("settings.audio.refresh_devices_button")).clicked() {
                    self.spawn_audio_device_scan();
                }
                if self.settings_ui.audio_device_rx.is_some() {
                    ui.label(RichText::new(t("settings.audio.scanning")).color(palette.ink_muted).small());
                }
            });

            egui::Grid::new("settings_audio_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                edited |= device_picker(
                    ui,
                    "settings_input_device",
                    &t("settings.audio.input_device_label"),
                    &mut self.config.audio.input_device,
                    &input_names,
                );
                ui.end_row();
                edited |= device_picker(
                    ui,
                    "settings_output_device",
                    &t("settings.audio.output_device_label"),
                    &mut self.config.audio.output_device,
                    &output_names,
                );
                ui.end_row();
                edited |= device_picker(
                    ui,
                    "settings_ringtone_device",
                    &t("settings.audio.ringing_device_label"),
                    &mut self.config.audio.ringtone_device,
                    &output_names,
                );
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("settings.audio.ringing_device_caption")).color(palette.ink_muted).small());
                info_hint(ui, palette, &t("settings.audio.ringing_device_hint"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.audio.custom_ringtone_label"));
                let name = self
                    .config
                    .audio
                    .ringtone_file
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| t("settings.audio.built_in_tone"));
                ui.label(RichText::new(name).color(palette.ink_muted));
                if ui.small_button(t("settings.choose_button")).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file()
                {
                    self.config.audio.ringtone_file = Some(path.to_string_lossy().into_owned());
                    edited = true;
                }
                if self.config.audio.ringtone_file.is_some() && ui.small_button(t("common.clear_button")).clicked() {
                    self.config.audio.ringtone_file = None;
                    edited = true;
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, palette, &t("settings.audio.ringtone_volume_label"));
                edited |= styled_slider(
                    ui,
                    palette,
                    egui::Slider::new(&mut self.config.audio.ringtone_volume, 0.0..=2.0).fixed_decimals(2),
                )
                .changed();
            });

            ui.add_space(6.0);
            edited |= ui
                .checkbox(&mut self.config.audio.echo_cancellation, t("settings.audio.echo_cancellation_checkbox"))
                .changed();
            ui.horizontal(|ui| {
                edited |= ui.checkbox(&mut self.config.audio.agc_enabled, t("settings.audio.agc_checkbox")).changed();
                info_hint(ui, palette, &t("settings.audio.agc_hint"));
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edited |= ui
                    .checkbox(&mut self.config.recording_enabled, t("settings.audio.record_calls_checkbox"))
                    .changed();
                info_hint(ui, palette, &t("settings.audio.record_calls_hint"));
            });
            if self.config.recording_enabled {
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.audio.format_label"));
                    egui::ComboBox::from_id_salt("settings_recording_format")
                        .selected_text(match self.config.recording_format {
                            RecordingFormat::Wav => t("settings.audio.format_wav"),
                            RecordingFormat::Mp3 => t("settings.audio.format_mp3"),
                        })
                        .show_ui(ui, |ui| {
                            edited |= ui
                                .selectable_value(
                                    &mut self.config.recording_format,
                                    RecordingFormat::Wav,
                                    t("settings.audio.format_wav"),
                                )
                                .changed();
                            edited |= ui
                                .selectable_value(
                                    &mut self.config.recording_format,
                                    RecordingFormat::Mp3,
                                    t("settings.audio.format_mp3"),
                                )
                                .changed();
                        });
                });
                ui.horizontal(|ui| {
                    field_label(ui, palette, &t("settings.audio.save_to_label"));
                    let default_shown = t("settings.audio.save_to_default");
                    let shown = self.config.recordings_dir_override.as_deref().unwrap_or(default_shown.as_str());
                    ui.label(RichText::new(shown).color(palette.ink_muted));
                    if ui.small_button(t("settings.choose_button")).clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.config.recordings_dir_override = Some(dir.to_string_lossy().into_owned());
                        edited = true;
                    }
                    if self.config.recordings_dir_override.is_some()
                        && ui.small_button(t("settings.reset_button")).clicked()
                    {
                        self.config.recordings_dir_override = None;
                        edited = true;
                    }
                });
            }
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
    let devices = if input { host.input_devices() } else { host.output_devices() };
    match devices {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).filter(|name| !is_irrelevant_alsa_device(name)).collect(),
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

#[cfg(test)]
#[path = "../../../tests/unit/settings.rs"]
mod tests;
