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

pub use app::DeelipApp;
pub use platform::tray;

/// Embedded Inter (body/display) and JetBrains Mono (data/numerals), both
/// SIL OFL 1.1 -- see `assets/fonts/OFL-*.txt` -- replacing egui's built-in
/// defaults; plus the Phosphor icon font for a coherent icon set instead of
/// ad hoc Unicode/emoji glyphs. Call once from the `eframe` creation
/// callback, before the app's first frame.
///
/// The "Signal" redesign's one typographic rule: names/labels render in
/// Inter (the default `Proportional` family, so every existing
/// `ui.label`/`ui.button` call site gets it for free), while anything that
/// *is* a number or address -- dial pad digits, call timers, SIP URIs,
/// timestamps -- renders in JetBrains Mono (the default `Monospace`
/// family). `inter-semibold`/`jbmono-medium` are registered as named
/// families for the selective emphasis call sites (headings, the in-call
/// timer) that need a heavier weight than the family default.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let embed = |fonts: &mut egui::FontDefinitions, key: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(key.into(), egui::FontData::from_static(bytes));
    };

    embed(
        &mut fonts,
        "inter-regular",
        include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
    );
    embed(
        &mut fonts,
        "inter-medium",
        include_bytes!("../../../assets/fonts/Inter-Medium.ttf"),
    );
    embed(
        &mut fonts,
        "inter-semibold",
        include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
    );
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
        .insert(0, "inter-regular".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "jbmono-regular".into());
    fonts.families.insert(
        egui::FontFamily::Name("inter-medium".into()),
        vec!["inter-medium".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("inter-semibold".into()),
        vec!["inter-semibold".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("jbmono-medium".into()),
        vec!["jbmono-medium".into()],
    );

    // `egui_phosphor::add_to_fonts` only appends its icon font as a fallback
    // onto the `Proportional` family -- any of the named families above
    // used on text that also contains an icon glyph (e.g. `TextStyle::Button`
    // resolving to "inter-medium", or a `RichText` combining an icon with
    // `theme::font_heading`) would otherwise render that glyph as a tofu
    // box. Append it to every family egui-phosphor doesn't already cover.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    for key in ["inter-medium", "inter-semibold", "jbmono-medium"] {
        fonts
            .families
            .entry(egui::FontFamily::Name(key.into()))
            .or_default()
            .push("phosphor".into());
    }
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("phosphor".into());

    tracing::debug!(
        "inter-medium family = {:?}",
        fonts.families.get(&egui::FontFamily::Name("inter-medium".into()))
    );
    tracing::debug!(
        "download_simple glyph = {:?}",
        egui_phosphor::regular::DOWNLOAD_SIMPLE
    );

    ctx.set_fonts(fonts);
}
