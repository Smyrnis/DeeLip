use egui::Color32;

use super::stats::format_bytes;
use super::widgets::{avatar_initial, with_alpha};

#[test]
fn with_alpha_255_is_a_pure_alpha_overwrite() {
    // `Color32::from_rgba_unmultiplied`'s alpha=255 fast path is a no-op
    // premultiply, so this is the one case where the rgb bytes stay exact.
    let opaque = Color32::from_rgb(0x11, 0x22, 0x33);
    assert_eq!(with_alpha(opaque, 255), opaque);
}

#[test]
fn with_alpha_0_is_fully_transparent() {
    let opaque = Color32::from_rgb(0x11, 0x22, 0x33);
    assert_eq!(with_alpha(opaque, 0), Color32::TRANSPARENT);
}

#[test]
fn with_alpha_sets_the_requested_alpha_channel() {
    let opaque = Color32::from_rgb(0x11, 0x22, 0x33);
    assert_eq!(with_alpha(opaque, 35).a(), 35);
}

#[test]
fn avatar_initial_picks_first_alphanumeric_uppercased() {
    assert_eq!(avatar_initial("alice"), 'A');
    assert_eq!(avatar_initial("  bob"), 'B');
    assert_eq!(avatar_initial("600"), '6');
    assert_eq!(avatar_initial("_underscore first"), 'U');
}

#[test]
fn avatar_initial_falls_back_to_hash_on_empty_or_symbols_only() {
    assert_eq!(avatar_initial(""), '#');
    assert_eq!(avatar_initial("---"), '#');
}

#[test]
fn format_bytes_stays_in_bytes_under_1024() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1023), "1023 B");
}

#[test]
fn format_bytes_switches_to_kb_at_1024() {
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(2048), "2.0 KB");
    assert_eq!(format_bytes(1536), "1.5 KB");
}
