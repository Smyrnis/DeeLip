mod app;
mod call_actions;
mod event_handling;
mod frame;
mod helpers;
mod media;
mod platform;
mod strings;
mod theme;
mod update;
mod views;

pub use app::{AccountSpawnMsg, DeelipApp, SharedApp};
pub use platform::tray;
pub use strings::init as init_strings;

/// Embedded JetBrains Mono, SIL OFL 1.1 -- see
/// `assets/fonts/OFL-JetBrainsMono.txt` -- replacing egui's built-in
/// defaults for every text style, plus the Phosphor icon font. Call once
/// from the `eframe` creation callback, before the app's first frame.
/// JetBrains Mono is the *only* typeface anywhere in this app (both
/// `Proportional` and `Monospace` resolve to it) -- see `docs/crates/ui.md`'s
/// Theming section for why.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let embed = |fonts: &mut egui::FontDefinitions, key: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(key.into(), egui::FontData::from_static(bytes).into());
    };

    embed(&mut fonts, "jbmono-regular", include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"));
    embed(&mut fonts, "jbmono-medium", include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf"));

    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "jbmono-regular".into());
    fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "jbmono-regular".into());
    fonts.families.insert(egui::FontFamily::Name("jbmono-medium".into()), vec!["jbmono-medium".into()]);

    // `egui_phosphor::add_to_fonts` only appends its icon font as a fallback
    // onto the `Proportional` family -- the named `jbmono-medium` family
    // used on text that also contains an icon glyph (e.g. a `RichText`
    // combining an icon with `theme::font_heading`) would otherwise render
    // that glyph as a tofu box. Append it there too.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts.families.entry(egui::FontFamily::Name("jbmono-medium".into())).or_default().push("phosphor".into());
    fonts.families.entry(egui::FontFamily::Monospace).or_default().push("phosphor".into());

    ctx.set_fonts(fonts);
}
