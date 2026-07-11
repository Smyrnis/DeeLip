use egui::RichText;

use crate::app::{DeelipApp, SharedApp};
use crate::helpers::window_icon;

/// Shared scaffolding for this app's "real separate OS window" pattern --
/// used by Settings, Transfer Call, the DTMF Keypad, and the Contact
/// dialog (Messages is the one exception: see its own doc comment on
/// `show_messages_window` for why its side-by-side `SidePanel`+
/// `CentralPanel` layout can't share this).
///
/// Every one of these windows needs the same ~35-line skeleton: check
/// `ctx.embed_viewports()` up front and render a fallback in-canvas
/// `egui::Window` directly against `app` if the backend can't open a real
/// second native window (deciding this up front matters: if
/// `show_viewport_deferred` were called unconditionally, its closure would
/// run *synchronously* on an embedding backend, and locking `self_app`
/// there would deadlock against the lock this call's own caller already
/// holds), otherwise open a genuine `Deferred` viewport with a titlebar
/// (a plain heading-styled label -- no in-app Close button, removed
/// earlier since real window decorations already provide one) and wire up
/// `close_requested()` to whatever this window's own close action is.
///
/// `is_open`/`on_close`/`title` are plain `fn` pointers rather than general
/// closures -- every real call site's version is already a non-capturing
/// closure (e.g. `|app| app.settings_open`, or Transfer Call's two-field
/// `|app| app.showing_transfer || app.showing_attended`), which Rust
/// coerces to `fn` for free, so there's no need for `Clone + Send + Sync`
/// bounds just to store one. `content` stays a real closure since it's the
/// one genuinely different piece of code per window -- bound as `Fn`, not
/// `FnMut`: `show_viewport_deferred` itself requires the outer closure to
/// be `Fn + Send + Sync` (it may be invoked repeatedly through a shared
/// reference), so a `content` that needed its *own* captured state to
/// mutate across calls wouldn't fit without interior mutability -- none of
/// this app's actual pop-out windows need that (`content` always just
/// forwards to a method on the `app`/`ui` it's handed, no captured state of
/// its own), so plain `Fn` is both sufficient and simpler.
#[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
                                     // piece of this window's identity; bundling them
                                     // into a struct wouldn't reduce real complexity.
pub(crate) fn show_pop_out_window(
    app: &mut DeelipApp,
    ctx: &egui::Context,
    self_app: SharedApp,
    key: &'static str,
    window_title: &'static str,
    size: [f32; 2],
    min_size: [f32; 2],
    resizable: bool,
    is_open: fn(&DeelipApp) -> bool,
    on_close: fn(&mut DeelipApp),
    title: fn(&DeelipApp) -> String,
    content: impl Fn(&mut DeelipApp, &mut egui::Ui) + Send + Sync + 'static,
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

            // Explicit margin -- confirmed live (Settings, at its own real
            // default width) that `CentralPanel::default()`'s bare default
            // left content rendered flush against the window's right edge
            // with zero breathing room. Applied to every pop-out window
            // now, not just Settings, to preempt the same class of bug
            // recurring in one of the others later.
            let frame = egui::Frame::central_panel(&child_ctx.style()).inner_margin(14.0);
            egui::CentralPanel::default()
                .frame(frame)
                .show(child_ctx, |ui| content(&mut app, ui));

            if child_ctx.input(|i| i.viewport().close_requested()) {
                on_close(&mut app);
            }
        },
    );
}
