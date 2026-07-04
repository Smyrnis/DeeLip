mod app;
mod call_actions;
mod event_handling;
mod frame;
mod helpers;
mod hotkeys;
mod media;
mod notify;
mod ringtone;
mod theme;
pub mod tray;
mod view_contacts;
mod view_dialer;
mod view_history;
mod view_settings;

pub use app::DeelipApp;

/// Embedded Cantarell (GNOME's own default UI font, SIL OFL 1.1 -- see
/// `assets/OFL.txt`) as the app's proportional font, replacing egui's
/// built-in default; plus the Phosphor icon font for a coherent icon set
/// instead of ad hoc Unicode/emoji glyphs. Call once from the `eframe`
/// creation callback, before the app's first frame.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "cantarell".into(),
        egui::FontData::from_static(include_bytes!("../../../assets/Cantarell-VF.otf")),
    );
    fonts.families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "cantarell".into());

    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    ctx.set_fonts(fonts);
}
