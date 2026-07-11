mod app;
mod call_actions;
mod event_handling;
mod frame;
mod helpers;
mod media;
mod platform;
mod theme;
mod update;
mod views;

pub use app::{DeelipApp, SharedApp};
pub use platform::tray;

/// Embedded JetBrains Mono, SIL OFL 1.1 -- see
/// `assets/fonts/OFL-JetBrainsMono.txt` -- replacing egui's built-in
/// defaults for every text style, not just numeric/data content; plus the
/// Phosphor icon font for a coherent icon set instead of ad hoc
/// Unicode/emoji glyphs. Call once from the `eframe` creation callback,
/// before the app's first frame.
///
/// The Darcula pass's typographic rule (v3, replacing the Inter/JetBrains
/// split from the earlier "Signal" redesign): JetBrains Mono is the *only*
/// typeface, everywhere -- both `Proportional` and `Monospace` resolve to
/// it, so there's no longer a body/data distinction to maintain at call
/// sites. `jbmono-medium` is still registered as a named family for the
/// selective emphasis call sites (headings, the in-call timer) that need a
/// heavier weight than Regular.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let embed = |fonts: &mut egui::FontDefinitions, key: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(key.into(), egui::FontData::from_static(bytes));
    };

    embed(
        &mut fonts,
        "jbmono-regular",
        include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    embed(
        &mut fonts,
        "jbmono-medium",
        include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf"),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "jbmono-regular".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "jbmono-regular".into());
    fonts.families.insert(
        egui::FontFamily::Name("jbmono-medium".into()),
        vec!["jbmono-medium".into()],
    );

    // `egui_phosphor::add_to_fonts` only appends its icon font as a fallback
    // onto the `Proportional` family -- the named `jbmono-medium` family
    // used on text that also contains an icon glyph (e.g. a `RichText`
    // combining an icon with `theme::font_heading`) would otherwise render
    // that glyph as a tofu box. Append it there too.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts
        .families
        .entry(egui::FontFamily::Name("jbmono-medium".into()))
        .or_default()
        .push("phosphor".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("phosphor".into());

    ctx.set_fonts(fonts);
}
