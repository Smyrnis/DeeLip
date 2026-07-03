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
}

impl Palette {
    pub fn for_theme(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn dark() -> Self {
        Self {
            accent: Color32::from_rgb(0x4C, 0xC3, 0x8A),
            info:   Color32::from_rgb(0x4C, 0x9E, 0xEB),
            danger: Color32::from_rgb(0xE5, 0x48, 0x4D),
            warn:   Color32::from_rgb(0xE8, 0xA3, 0x3D),
            muted:  Color32::from_rgb(0x9A, 0xA0, 0xA6),
        }
    }

    pub fn light() -> Self {
        Self {
            accent: Color32::from_rgb(0x1F, 0x8B, 0x4C),
            info:   Color32::from_rgb(0x24, 0x70, 0xC4),
            danger: Color32::from_rgb(0xC7, 0x36, 0x2B),
            warn:   Color32::from_rgb(0xE8, 0xA3, 0x3D),
            muted:  Color32::from_rgb(0x6B, 0x72, 0x80),
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

    let rounding = egui::Rounding::same(6.0);
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
