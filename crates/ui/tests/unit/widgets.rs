use super::*;

#[test]
fn avatar_color_is_deterministic_for_the_same_seed() {
    assert_eq!(avatar_color("alice"), avatar_color("alice"));
}

#[test]
fn avatar_color_picks_one_of_the_four_fixed_hues() {
    const HUES: [egui::Color32; 4] = [
        egui::Color32::from_rgb(0x68, 0x97, 0xBB),
        egui::Color32::from_rgb(0xCC, 0x78, 0x32),
        egui::Color32::from_rgb(0x98, 0x76, 0xAA),
        egui::Color32::from_rgb(0x6A, 0x87, 0x59),
    ];
    for seed in ["alice", "bob", "600", "", "sip:carol@example.com"] {
        assert!(HUES.contains(&avatar_color(seed)), "unexpected color for seed {seed:?}");
    }
}

#[test]
fn digit_letters_matches_phone_keypad_layout() {
    assert_eq!(digit_letters('2'), "ABC");
    assert_eq!(digit_letters('7'), "PQRS");
    assert_eq!(digit_letters('9'), "WXYZ");
}

#[test]
fn digit_letters_is_empty_for_non_letter_digits() {
    assert_eq!(digit_letters('0'), "");
    assert_eq!(digit_letters('1'), "");
    assert_eq!(digit_letters('*'), "");
    assert_eq!(digit_letters('#'), "");
}

#[test]
fn keypad_button_text_includes_the_digit_and_its_letters() {
    let job = keypad_button_text('7', Palette::light());
    assert_eq!(job.text, "7\nPQRS");
}

#[test]
fn keypad_button_text_has_no_letter_line_for_digits_without_letters() {
    let job = keypad_button_text('1', Palette::light());
    assert_eq!(job.text, "1");
}
