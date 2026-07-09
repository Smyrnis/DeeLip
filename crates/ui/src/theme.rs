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
//! **v3 revision (2026-07-09)**: pulled back from the "Signal" redesign's
//! spacious/rounded/indigo look toward a Darcula-style IDE identity, per
//! user feedback that the app felt "too modern." Real IntelliJ Darcula
//! hex values (not an approximation): canvas `#2B2B2B`, surface `#3C3F41`,
//! ink `#A9B7C6` (Darcula's own iconic foreground), ringing-orange
//! `#CC7832` (Darcula's own keyword orange), danger-red `#BC3F3C`
//! (Darcula's error red). Darcula is a fixed dark identity in real
//! IntelliJ -- there's no official light counterpart, so unlike the
//! previous `dark()`/`light()` pair, this is deliberately single-theme now
//! (disclosed and accepted when the redesign mockup was approved).
//! Rounding also dropped to near-zero (sharp IDE-panel corners, not the
//! previous rounded cards) -- see `apply_style`/`card_frame`.
//!
//! **v3.1 (2026-07-10)**: first live use of v3 turned up real feedback --
//! the bright sky-blue `#6897BB` (Darcula's *numeric-literal* text color)
//! read as too saturated/"modern" once it was reused as general interactive
//! chrome (tab-bar selection, the Contacts FAB) rather than just text.
//! `signal` is now Darcula's string-green `#6A8759` instead -- same
//! semantic role (active/connected/positive, per the rule above), just a
//! color that doesn't read as "blue everywhere." Interactive *chrome*
//! (tab-bar/list selection highlight, the Contacts FAB, the Dialer's main
//! "Call" button) moved off `signal` entirely onto neutral
//! `surface`/`surface_hover` grey -- real Darcula's own button chrome is
//! grey, not accent-colored; `signal` now shows up only on genuine
//! call-state signals (connected badge, presence-available dot, the
//! ringing-screen's Accept button paired against a red Reject, ZRTP SAS
//! text, voicemail count). The old blue hex is kept as `link`, wired only
//! to `Visuals::hyperlink_color` -- there's no visible in-app hyperlink
//! today, but this keeps "blue = links/numbers only" true if one's ever
//! added, rather than quietly reintroducing blue as a second accent.
//! Spacing (`apply_style`'s `item_spacing`/`button_padding`, `card_frame`'s
//! `inner_margin`) also loosened -- the v2 "too much chrome" density pass
//! had gone further than this redesign's own margins needed, per feedback
//! that the whole app now read as too tight/cramped.
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
//! phosphor constant. **This isn't limited to the phosphor icon font
//! either**: a plain Unicode "☰" (hamburger/trigram symbol) was also found
//! silently rendering as "?" in this app's actual font stack (caught live
//! via Xvfb, not by reasoning about it) -- any icon-ish Unicode character,
//! not just phosphor constants, needs to be rendered large and actually
//! looked at before trusting it; when in doubt use a plain word instead.

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
    /// The app's one and only theme -- see this module's own v3-revision
    /// doc comment for why there's no `light()`/`for_theme()` counterpart
    /// anymore.
    pub fn dark() -> Self {
        Self {
            canvas: Color32::from_rgb(0x2B, 0x2B, 0x2B),
            surface: Color32::from_rgb(0x3C, 0x3F, 0x41),
            surface_hover: Color32::from_rgb(0x4B, 0x4E, 0x50),
            border: Color32::from_rgb(0x4B, 0x4B, 0x4B),
            ink: Color32::from_rgb(0xA9, 0xB7, 0xC6),
            ink_muted: Color32::from_rgb(0x80, 0x80, 0x80),
            signal: Color32::from_rgb(0x6A, 0x87, 0x59),
            ringing: Color32::from_rgb(0xCC, 0x78, 0x32),
            danger: Color32::from_rgb(0xBC, 0x3F, 0x3C),
            link: Color32::from_rgb(0x68, 0x97, 0xBB),
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
