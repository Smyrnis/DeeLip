//! Video-panel texture caching: converts decoded/captured frames to egui
//! textures on change, and renders one side of the picture-in-picture panel.

use deelip_media::video_codec::Yuv420Frame;
use egui::{RichText, Ui};

use crate::app::VideoViewCache;
use crate::helpers::empty_state;
use crate::strings::t;
use crate::theme::Palette;

/// Convert `frame` (if it differs from `cache.frame`, the last one already
/// uploaded) to RGB and create/update `cache.texture` -- a no-op if `frame`
/// is `None` or unchanged, so an unchanged decoded/captured frame isn't
/// reconverted/re-uploaded every repaint.
pub(super) fn update_video_view(
    ctx: &egui::Context, cache: &mut VideoViewCache, frame: Option<Yuv420Frame>, texture_name: &str,
) {
    let Some(frame) = frame else { return };
    if cache.frame.as_ref() == Some(&frame) {
        return;
    }
    let rgb = frame.to_rgb8();
    let size = [frame.width as usize, frame.height as usize];
    let image = egui::ColorImage::from_rgb(size, &rgb);
    match &mut cache.texture {
        Some(tex) => tex.set(image, egui::TextureOptions::default()),
        None => cache.texture = Some(ctx.load_texture(texture_name, image, egui::TextureOptions::default())),
    }
    cache.frame = Some(frame);
}

/// Render one side of the video panel: the cached texture if one exists
/// yet, else a muted placeholder ("No video yet" for the self-view, which
/// never gets one without a camera; "Waiting for video…" for the remote
/// side, which should fill in shortly after the call connects). `is_local`
/// picks which placeholder applies -- previously inferred by comparing
/// `label == "You"`, which broke once `label` became a localized string
/// that isn't literally "You" in every language.
pub(super) fn show_video_view(ui: &mut Ui, palette: &Palette, cache: &VideoViewCache, label: &str, is_local: bool) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).color(palette.ink_muted).small());
        match &cache.texture {
            Some(tex) => {
                ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(160.0, 120.0)));
            }
            None => {
                let text = if is_local { t("dialer.no_video_yet") } else { t("dialer.waiting_for_video") };
                empty_state(ui, palette, &text);
            }
        }
    });
}
