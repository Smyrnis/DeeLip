//! DeeLip's color palette -- named semantic tokens instead of ad hoc
//! `Color32::LIGHT_BLUE`/`RED`/`GRAY` literals scattered through the UI code.
//! One instance per theme, refreshed whenever `AppConfig.dark_mode` changes.

use egui::Color32;

#[derive(Clone, Copy)]
pub struct Palette {
    /// Primary/active/outbound-call color -- also wired into
    /// `Visuals::selection.bg_fill`/`hyperlink_color` so it's the app's
    /// actual accent, not just a few buttons' text color.
    pub accent: Color32,
    /// Inbound calls.
    pub info: Color32,
    /// Hang up / reject / delete / destructive actions.
    pub danger: Color32,
    /// Hold / ringing / pending.
    pub warn: Color32,
    /// Secondary/timestamp/disabled text.
    pub muted: Color32,
    /// Flat "card"/section surface -- one step off the window background,
    /// used instead of `ui.group()`'s bordered box for a layered-not-boxed
    /// look (dialer sections, in-call screen, list containers).
    pub card: Color32,
    /// Hovered list-row background (History/Contacts/Messages rows).
    pub row_hover: Color32,
    /// Thin separator line between list rows / sections.
    pub divider: Color32,
}

impl Palette {
    pub fn for_theme(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn dark() -> Self {
        Self {
            accent:    Color32::from_rgb(0x4C, 0xC3, 0x8A),
            info:      Color32::from_rgb(0x4C, 0x9E, 0xEB),
            danger:    Color32::from_rgb(0xE5, 0x48, 0x4D),
            warn:      Color32::from_rgb(0xE8, 0xA3, 0x3D),
            muted:     Color32::from_rgb(0x9A, 0xA0, 0xA6),
            card:      Color32::from_rgb(0x26, 0x2A, 0x2F),
            row_hover: Color32::from_rgb(0x32, 0x37, 0x3D),
            divider:   Color32::from_rgb(0x38, 0x3D, 0x43),
        }
    }

    pub fn light() -> Self {
        Self {
            accent:    Color32::from_rgb(0x1F, 0x8B, 0x4C),
            info:      Color32::from_rgb(0x24, 0x70, 0xC4),
            danger:    Color32::from_rgb(0xC7, 0x36, 0x2B),
            warn:      Color32::from_rgb(0xE8, 0xA3, 0x3D),
            muted:     Color32::from_rgb(0x6B, 0x72, 0x80),
            card:      Color32::from_rgb(0xF2, 0xF3, 0xF5),
            row_hover: Color32::from_rgb(0xE8, 0xEA, 0xED),
            divider:   Color32::from_rgb(0xDF, 0xE1, 0xE4),
        }
    }
}

/// Apply the palette's accent color into `Visuals` (selection highlight,
/// hyperlinks) and give buttons/windows a softer, less boxy rounding than
/// egui's sharp-cornered default. Called once per frame alongside
/// `ctx.set_visuals`, since `Visuals::dark()`/`light()` must run first.
pub fn apply_style(ctx: &egui::Context, visuals: &mut egui::Visuals, palette: &Palette) {
    visuals.selection.bg_fill = palette.accent;
    visuals.hyperlink_color = palette.accent;

    let rounding = egui::Rounding::same(10.0);
    visuals.window_rounding = rounding;
    visuals.widgets.noninteractive.rounding = rounding;
    visuals.widgets.inactive.rounding = rounding;
    visuals.widgets.hovered.rounding = rounding;
    visuals.widgets.active.rounding = rounding;
    visuals.widgets.open.rounding = rounding;

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style(style);
}

/// A flat, borderless "card" surface (palette.card fill, rounded, padded) --
/// the replacement for `ui.group()`'s bordered box everywhere this redesign
/// wants a layered-not-boxed look.
pub fn card_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::none()
        .fill(palette.card)
        .rounding(egui::Rounding::same(10.0))
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
pub fn full_width_card<R>(ui: &mut egui::Ui, palette: Palette, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame(&palette).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add_contents(ui)
    }).inner
}
