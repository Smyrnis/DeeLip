//! DeeLip's design system -- named semantic color tokens plus the
//! JetBrains-Mono-everywhere type scale (see `lib.rs::install_fonts`),
//! instead of ad hoc `Color32` literals and whatever font egui ships by
//! default.
//!
//! The one rule every view follows: color communicates call *state*, not
//! decoration. `signal` means active/connected/positive, `ringing` means
//! pending/incoming/hold, `danger` means destructive -- nothing else
//! borrows them. Everything else is drawn from the neutral canvas/surface/
//! border/ink scale.
//!
//! Palette revision history and the list of confirmed-broken icon glyphs in
//! the bundled Phosphor font are documented in `docs/crates/ui.md`'s Theming
//! section -- see that before changing any hex value or reaching for a new
//! icon constant.

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
    /// Active/connected/outbound/positive call-state signal -- genuine
    /// state indicators only (connected badge, presence dot, the ringing
    /// screen's Accept button, ZRTP SAS text). NOT general interactive
    /// chrome (buttons/tabs/selection) -- see this module's v3.1 doc note;
    /// those use `surface`/`surface_hover` grey instead.
    pub signal: Color32,
    /// Incoming/pending/dialing/on-hold.
    pub ringing: Color32,
    /// Hang up / reject / delete / destructive actions.
    pub danger: Color32,
    /// Hyperlink text color only (see this module's v3.1 doc note) -- kept
    /// separate from `signal` so "blue" never leaks back into general
    /// chrome even though nothing in-app currently renders a hyperlink.
    pub link: Color32,
}

impl Palette {
    /// The app's one and only theme -- see `docs/crates/ui.md`'s Theming section
    /// for the real IntelliJ Light values used here and why there's still
    /// no `dark()`/`for_theme()` counterpart.
    pub fn light() -> Self {
        Self {
            canvas: Color32::from_rgb(0xF7, 0xF8, 0xFA),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_hover: Color32::from_rgb(0xEB, 0xEC, 0xF0),
            border: Color32::from_rgb(0xC9, 0xCC, 0xD6),
            ink: Color32::from_rgb(0x00, 0x00, 0x00),
            ink_muted: Color32::from_rgb(0x81, 0x85, 0x94),
            signal: Color32::from_rgb(0x20, 0x8A, 0x3C),
            ringing: Color32::from_rgb(0xA4, 0x67, 0x04),
            danger: Color32::from_rgb(0xBC, 0x30, 0x3E),
            link: Color32::from_rgb(0x31, 0x5F, 0xBD),
        }
    }
}

/// Named-family font ids for the selective-emphasis call sites that need a
/// heavier weight than the `Proportional`/`Monospace` family default
/// (`jbmono-regular`, set in `lib.rs::install_fonts`). All three point at
/// JetBrains Mono weights now -- there's no separate heading typeface in
/// the Darcula-everywhere pass, just Regular vs Medium.
pub fn font_heading(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("jbmono-medium".into()))
}

pub fn font_medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("jbmono-medium".into()))
}

/// Emphasized numerals -- the in-call timer, a focused dial-pad digit.
/// Plain data (SIP URIs, timestamps, ordinary dial-pad digits) should use
/// the `Monospace` `TextStyle` instead, which is already `jbmono-regular`.
pub fn font_mono_medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("jbmono-medium".into()))
}

/// "This field holds something SIP-address-shaped" -- a plain (non-medium)
/// 13px monospace, for a directly-editable/displayed SIP URI/host field
/// (Settings' Server field, the Transfer/Contact-dialog number entry, a
/// bare-address fallback label). Named here since it was previously
/// copy-pasted at 4 call sites as a raw `FontId::new(13.0, Monospace)`.
pub fn font_address() -> egui::FontId {
    egui::FontId::new(13.0, egui::FontFamily::Monospace)
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
    // Grey chrome, not accent-colored -- see this module's v3.1 doc note.
    visuals.selection.bg_fill = palette.surface_hover;
    visuals.selection.stroke.color = palette.ink;
    visuals.hyperlink_color = palette.link;
    visuals.window_stroke = egui::Stroke::new(1.0, palette.border);

    // v3: near-flat IDE-panel corners -- sharper than v2's already-tightened
    // rounding, matching Darcula's own squared-off widget chrome.
    let rounding = egui::Rounding::same(2.0);
    visuals.window_rounding = egui::Rounding::same(2.0);
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
    // v3.1: loosened back up from v2's "too much chrome" density pass --
    // that ended up reading as too tight/cramped once lived with.
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
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
        .rounding(egui::Rounding::same(2.0))
        .inner_margin(egui::Margin::same(14.0))
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
pub fn full_width_card<R>(ui: &mut egui::Ui, palette: Palette, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame(&palette)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}
