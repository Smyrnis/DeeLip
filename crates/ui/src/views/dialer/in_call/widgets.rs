//! Stateless drawing primitives for the in-call screen -- avatar, status
//! badge, and the two button styles -- with no `DeelipApp` dependency of
//! their own.

use egui::{Align2, Color32, RichText, Ui};

use crate::theme::{self, Palette};

/// Which state `call_avatar`/`state_badge` reflect -- design history (this
/// replaced an earlier animated dual-ring pulse): `docs/crates/ui.md`'s "status-dot
/// redesign" note.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum RingState {
    Pending,
    Connected,
}

/// A caller initial on a small surface circle, with a state-colored status
/// dot at its corner.
pub(super) fn call_avatar(ui: &mut Ui, palette: &Palette, display_name: &str, state: RingState) {
    let avatar_d = 68.0;
    let pad = 8.0; // room for the status dot to sit outside the avatar's own edge
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(avatar_d + pad, avatar_d + pad), egui::Sense::hover());
    let center = rect.center() - egui::vec2(pad / 2.0, pad / 2.0);
    let painter = ui.painter();
    let avatar_r = avatar_d / 2.0;

    painter.circle_filled(center, avatar_r, palette.surface);
    painter.circle_stroke(center, avatar_r, egui::Stroke::new(1.0, palette.border));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        avatar_initial(display_name).to_string(),
        theme::font_heading(22.0),
        palette.ink,
    );

    let dot_color = match state {
        RingState::Pending => palette.ringing,
        RingState::Connected => palette.signal,
    };
    let dot_alpha = match state {
        RingState::Pending => {
            // A slow opacity fade, not a bounce. No extra `request_repaint()`
            // -- `frame.rs`'s own 50ms cadence already redraws this often
            // enough to read as smooth.
            let t = ui.input(|i| i.time) as f32;
            let phase = (t * 1.6).sin() * 0.5 + 0.5;
            (110.0 + phase * 145.0) as u8
        }
        RingState::Connected => 255,
    };
    let dot_center = center + egui::vec2(avatar_r * 0.78, avatar_r * 0.78);
    // A canvas-colored ring first, so the dot reads as sitting on top of
    // (cut out from) the avatar's own edge rather than overlapping it raw.
    painter.circle_filled(dot_center, 7.0, palette.canvas);
    painter.circle_filled(dot_center, 5.0, with_alpha(dot_color, dot_alpha));
}

/// A small filled pill with muted-tint background -- the live-status
/// convention (a short label in a colored chip) this redesign pass adopted
/// in place of the original pulse-ring animation. `text` should be
/// lowercase, matching the rest of this screen's quiet, unshouty labels.
pub(super) fn state_badge(ui: &mut Ui, text: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(with_alpha(color, 35))
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(7.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(egui::FontId::new(10.5, egui::FontFamily::Monospace)).color(color));
        });
}

pub(super) fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// First meaningful character of a display name/address, uppercased --
/// `call_avatar`'s center glyph. Falls back to a phone glyph-friendly `#`
/// on the (practically unreachable) empty-string case.
pub(super) fn avatar_initial(display_name: &str) -> char {
    display_name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).unwrap_or('#')
}

/// The caller's name in Inter, or a bare address in JetBrains Mono when no
/// contact resolved it -- the one typographic rule (numbers/addresses are
/// mono, names are Inter) applied to the in-call screen's hero label.
pub(super) fn caller_name_label(ui: &mut Ui, palette: &Palette, name: &str, is_name: bool) {
    let font = if is_name { theme::font_heading(24.0) } else { egui::FontId::new(20.0, egui::FontFamily::Monospace) };
    ui.label(RichText::new(name).font(font).color(palette.ink));
}

/// A large rounded-square icon-only button for the focused-call screen's
/// primary actions (Accept/Reject/Hang Up) -- same rounded-square language
/// as `phone_keypad`'s digit buttons, not a full circle.
pub(super) fn circular_action_button(ui: &mut Ui, icon: &str, color: egui::Color32) -> bool {
    let button = egui::Button::new(RichText::new(icon).size(22.0).color(egui::Color32::WHITE))
        .fill(color)
        .rounding(egui::Rounding::same(14.0));
    ui.add_sized([64.0, 64.0], button).clicked()
}

/// Column width reserved per button in the Mute/Record/Xfer/Keypad row --
/// wider than the 48px button itself so "Record"/"Keypad" have room not to
/// wrap (see `icon_toggle_button`'s doc comment for why a column that wraps
/// its caption while its neighbors don't caused a real bug). Also used by
/// `show_focused_call_controls` to compute that row's own centering width.
pub(super) const ICON_TOGGLE_COL_WIDTH: f32 = 60.0;

/// A smaller icon-only rounded-square button with a small caption
/// underneath -- the secondary in-call actions (Mute, Record, Transfer,
/// Keypad), same icon+caption idiom `phone_keypad` already uses for its
/// digit+letters. `active` (the surface_hover fill, matching this theme's
/// existing "toggled on" convention e.g. the tab bar's selected state)
/// reflects the button's own on/off state (muted, currently recording,
/// panel open); `danger` additionally recolors the icon+caption to
/// `palette.danger` for a state that's not just "on" but actively
/// consequential (recording right now).
///
/// Deliberately built from raw `ui.painter()` calls on one
/// `ui.allocate_exact_size` rect, not `egui::Button` + a layout container --
/// two layout-based approaches were tried first and both had a real,
/// live-desktop-only box-position bug. Full writeup: `docs/crates/ui.md`.
pub(super) fn icon_toggle_button(
    ui: &mut Ui, palette: &Palette, icon: &str, caption: &str, active: bool, danger: bool,
) -> bool {
    const BTN: f32 = 48.0;
    let icon_color = if danger { palette.danger } else { palette.ink };
    let fill = if active { palette.surface_hover } else { palette.surface };

    let (col_rect, response) = ui.allocate_exact_size(egui::vec2(ICON_TOGGLE_COL_WIDTH, 64.0), egui::Sense::click());
    let btn_rect =
        egui::Rect::from_min_size(egui::pos2(col_rect.center().x - BTN / 2.0, col_rect.min.y), egui::vec2(BTN, BTN));

    let painter = ui.painter();
    painter.rect(btn_rect, egui::Rounding::same(12.0), fill, egui::Stroke::new(1.0, palette.border));
    // Per-glyph vertical nudge -- the Phosphor `MICROPHONE`/
    // `MICROPHONE_SLASH` glyph's ink sits visibly higher within its own
    // font-metrics line box than `RECORD`/`EXPORT`/`NUMPAD` do (confirmed
    // via a zoomed side-by-side screenshot), unrelated to the box-position
    // bug above -- this only recenters that one glyph's ink within an
    // already-correctly-positioned button.
    let nudge_y = if icon == egui_phosphor::regular::MICROPHONE || icon == egui_phosphor::regular::MICROPHONE_SLASH {
        3.0
    } else {
        0.0
    };
    painter.text(
        btn_rect.center() + egui::vec2(0.0, nudge_y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(18.0),
        icon_color,
    );
    painter.text(
        egui::pos2(col_rect.center().x, btn_rect.max.y + 2.0),
        egui::Align2::CENTER_TOP,
        caption,
        egui::FontId::new(11.0, egui::FontFamily::Proportional), // matches `RichText::small()`
        icon_color,
    );
    response.clicked()
}
