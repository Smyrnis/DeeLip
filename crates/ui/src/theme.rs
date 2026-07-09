//! DeeLip's "Signal" design system -- named semantic color tokens plus the
//! Inter/JetBrains Mono type scale (see `lib.rs::install_fonts`), instead of
//! ad hoc `Color32` literals and whatever font egui ships by default. One
//! `Palette` instance per theme, refreshed whenever `AppConfig.dark_mode`
//! changes.
//!
//! The one rule every view follows: color communicates call *state*, not
//! decoration. `signal` means active/connected/positive, `ringing` means
//! pending/incoming/hold, `danger` means destructive -- nothing else
//! borrows them. Everything else is drawn from the neutral canvas/surface/
//! border/ink scale.
//!
//! **v2 revision (2026-07-08, later same day as the original Signal pass)**:
//! the user felt the first pass read as too playful -- oversaturated color,
//! circular/bubbly shapes everywhere, and a large animated pulse-ring as
//! the in-call signature. This is a deliberate second iteration on the same
//! system, not a different one: same three semantic hues, pulled down in
//! saturation; same card/surface structure, with a smaller corner radius
//! (rectangles, not circles, outside of avatars); the pulse ring replaced
//! by a small static-avatar + status-dot + text-badge convention (see
//! `views/dialer.rs::call_avatar`), closer to how Stripe/Slack/Notion show
//! live status than to a hero animation. Density also tightened (smaller
//! type scale, tighter spacing) per the "too much chrome" feedback.
//!
//! **Known broken icons**: the bundled `egui-phosphor` 0.6.0 "Regular"
//! variant font has several codepoints whose cmap resolves to the wrong
//! glyph -- not a tofu box, but a real (wrong) Latin letter or punctuation
//! mark, discovered by rendering every icon constant this app uses at a
//! large size and inspecting the actual shape. Confirmed broken so far:
//! `INFO`, `BACKSPACE`, `ARROW_BEND_UP_RIGHT`, `ARROW_DOWN_LEFT`,
//! `ARROW_UP_RIGHT`, `DOWNLOAD`, `DOWNLOAD_SIMPLE`, `FILE_ARROW_DOWN`,
//! `FLOPPY_DISK`, `ARROW_DOWN` -- these render fine: `EXPORT`,
//! `UPLOAD_SIMPLE`, `ARROW_SQUARE_OUT`. Call sites needing a broken one use
//! a plain Unicode character instead (e.g. "⌫", "↱", "(i)") rather than the
//! phosphor constant. If a newly-added icon renders as a stray letter
//! instead of a glyph, this is why -- verify it large before trusting it.

use egui::Color32;

#[derive(Clone, Copy)]
pub struct Palette {
    /// Window/panel background -- the canvas everything else sits on.
    pub canvas: Color32,
    /// Cards, rows, inputs, dial keys -- one step off the canvas.
    pub surface: Color32,
    /// Hovered row / pressed-adjacent surface state.
    pub surface_hover: Color32,
    /// Hairline dividers and card/input strokes -- barely-there, not a
    /// heavy boxed-in border.
    pub border: Color32,
    /// Primary text.
    pub ink: Color32,
    /// Secondary text -- timestamps, hints, placeholders, captions.
    pub ink_muted: Color32,
    /// Active/connected/outbound/positive -- the app's one accent, wired
    /// into `Visuals::selection.bg_fill`/`hyperlink_color` so it's the
    /// actual accent everywhere, not just a few buttons' text color.
    pub signal: Color32,
    /// Incoming/pending/dialing/on-hold.
    pub ringing: Color32,
    /// Hang up / reject / delete / destructive actions.
    pub danger: Color32,
}

impl Palette {
    pub fn for_theme(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }

    pub fn dark() -> Self {
        Self {
            canvas: Color32::from_rgb(0x0E, 0x10, 0x13),
            surface: Color32::from_rgb(0x16, 0x18, 0x1B),
            surface_hover: Color32::from_rgb(0x1C, 0x1F, 0x23),
            border: Color32::from_rgb(0x26, 0x2A, 0x2E),
            ink: Color32::from_rgb(0xE8, 0xE9, 0xEB),
            ink_muted: Color32::from_rgb(0x8A, 0x8F, 0x94),
            signal: Color32::from_rgb(0x6B, 0x7B, 0xAE),
            ringing: Color32::from_rgb(0xC9, 0x9A, 0x54),
            danger: Color32::from_rgb(0xC1, 0x5C, 0x56),
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: Color32::from_rgb(0xF7, 0xF7, 0xF8),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_hover: Color32::from_rgb(0xEF, 0xF0, 0xF1),
            border: Color32::from_rgb(0xE1, 0xE3, 0xE5),
            ink: Color32::from_rgb(0x1A, 0x1C, 0x1E),
            ink_muted: Color32::from_rgb(0x71, 0x76, 0x7B),
            signal: Color32::from_rgb(0x5A, 0x6B, 0x9E),
            ringing: Color32::from_rgb(0xB8, 0x87, 0x4A),
            danger: Color32::from_rgb(0xB0, 0x50, 0x4A),
        }
    }
}

/// Named-family font ids for the selective-emphasis call sites that need a
/// heavier weight than the `Proportional`/`Monospace` family defaults
/// (`inter-regular`/`jbmono-regular`, set in `lib.rs::install_fonts`).
pub fn font_heading(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("inter-semibold".into()))
}

pub fn font_medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("inter-medium".into()))
}

/// Emphasized numerals -- the in-call timer, a focused dial-pad digit.
/// Plain data (SIP URIs, timestamps, ordinary dial-pad digits) should use
/// the `Monospace` `TextStyle` instead, which is already `jbmono-regular`.
pub fn font_mono_medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("jbmono-medium".into()))
}

/// Apply the palette across `Visuals` (backgrounds, selection highlight,
/// hyperlinks, widget fills/strokes) and set the Inter/JetBrains Mono type
/// scale. Called once per frame alongside `ctx.set_visuals`, since
/// `Visuals::dark()`/`light()` must run first.
pub fn apply_style(ctx: &egui::Context, visuals: &mut egui::Visuals, palette: &Palette) {
    visuals.override_text_color = Some(palette.ink);
    visuals.panel_fill = palette.canvas;
    visuals.window_fill = palette.canvas;
    visuals.extreme_bg_color = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.selection.bg_fill = palette.signal;
    visuals.selection.stroke.color = palette.ink;
    visuals.hyperlink_color = palette.signal;
    visuals.window_stroke = egui::Stroke::new(1.0, palette.border);

    // v2: small-radius rectangles, not the original pass's rounder/softer
    // corners -- one of the concrete "less playful" changes.
    let rounding = egui::Rounding::same(5.0);
    visuals.window_rounding = egui::Rounding::same(6.0);
    for widgets in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widgets.rounding = rounding;
    }
    visuals.widgets.noninteractive.bg_fill = palette.canvas;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.hovered.bg_fill = palette.surface_hover;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.active.bg_fill = palette.surface_hover;
    visuals.widgets.open.bg_fill = palette.surface;

    let mut style = (*ctx.style()).clone();
    // v2: tighter spacing/type scale -- the "too much chrome" feedback.
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 5.0);
    style.text_styles = [
        (egui::TextStyle::Small, egui::FontId::new(11.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Body, egui::FontId::new(13.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Button, font_medium(13.0)),
        (egui::TextStyle::Heading, font_heading(16.0)),
        (egui::TextStyle::Monospace, egui::FontId::new(12.5, egui::FontFamily::Monospace)),
    ]
    .into();
    ctx.set_style(style);
}

/// A flat "card" surface -- `palette.surface` fill, a hairline
/// `palette.border` stroke, rounded, padded -- the redesign's replacement
/// for both `ui.group()`'s heavier box and the old solid-fill-only card.
pub fn card_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::none()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::same(10.0))
}

/// `card_frame(palette).show(ui, |ui| { ui.set_width(ui.available_width()); ... })`
/// in one call -- every call site across Dialer/Settings paired the two
/// identically, so this just removes that boilerplate (and the risk of a
/// site forgetting the `set_width`, which would leave the card sized to its
/// content instead of the full row). Takes `palette` by value (it's `Copy`)
/// rather than `&Palette`: call sites that also read `self` inside
/// `add_contents` (most of them) would otherwise hit a borrow conflict
/// between `&self.palette` and the closure capturing `self` mutably, since
/// both are evaluated as part of the same call.
pub fn full_width_card<R>(
    ui: &mut egui::Ui,
    palette: Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    card_frame(&palette)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}
