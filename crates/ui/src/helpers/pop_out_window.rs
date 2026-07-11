use egui::RichText;

use crate::app::{DeelipApp, SharedApp};
use crate::helpers::window_icon;

/// Shared scaffolding for this app's "real separate OS window" pattern --
/// used by Settings, Transfer Call, the DTMF Keypad, and the Contact
/// dialog (Messages is the one exception). Full rationale (the
/// `embed_viewports()` deadlock hazard, the `fn`-pointer-vs-closure design,
/// why Messages can't share this) is in `docs/crates/ui.md`'s "Pop-out windows"
/// section.
#[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
                                     // piece of this window's identity; bundling them
                                     // into a struct wouldn't reduce real complexity.
pub(crate) fn show_pop_out_window(
    app: &mut DeelipApp, ctx: &egui::Context, self_app: SharedApp, key: &'static str, window_title: String,
    size: [f32; 2], min_size: [f32; 2], resizable: bool, is_open: fn(&DeelipApp) -> bool, on_close: fn(&mut DeelipApp),
    title: fn(&DeelipApp) -> String, content: impl Fn(&mut DeelipApp, &mut egui::Ui) + Send + Sync + 'static,
) {
    if !is_open(app) {
        return;
    }

    if ctx.embed_viewports() {
        let mut open = true;
        egui::Window::new(title(app))
            .id(egui::Id::new(key).with("fallback"))
            .open(&mut open)
            .collapsible(false)
            .resizable(resizable)
            .default_size(size)
            .min_width(min_size[0])
            .show(ctx, |ui| content(app, ui));
        if !open {
            on_close(app);
        }
        return;
    }

    ctx.show_viewport_deferred(
        egui::ViewportId::from_hash_of(key),
        egui::ViewportBuilder::default()
            .with_title(window_title)
            .with_inner_size(size)
            .with_min_inner_size(min_size)
            .with_resizable(resizable)
            .with_icon(window_icon()),
        move |child_ctx, _class| {
            let mut app = self_app.lock();
            if !is_open(&app) {
                return;
            }

            let label = title(&app);
            egui::TopBottomPanel::top(format!("{key}_titlebar")).show(child_ctx, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(label).font(crate::theme::font_heading(16.0)));
                ui.add_space(4.0);
            });

            // Explicit margin -- egui's own default left content flush
            // against the window edge (see docs/crates/ui.md's pop-out section).
            let frame = egui::Frame::central_panel(&child_ctx.style()).inner_margin(14.0);
            egui::CentralPanel::default().frame(frame).show(child_ctx, |ui| content(&mut app, ui));

            if child_ctx.input(|i| i.viewport().close_requested()) {
                on_close(&mut app);
            }
        },
    );
}
