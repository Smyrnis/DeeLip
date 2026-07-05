use super::*;

/// Regression guard for `DIGIT_FONT` typos (digit "3" originally had a
/// garbled diagonal-squiggle shape instead of the standard two-bar
/// glyph, only caught by rendering it and looking -- checked here by a
/// hand-verified reference table instead, matching the plain ASCII-art
/// each glyph draws when printed row-by-row column-by-column).
#[test]
fn digit_font_matches_reference_glyphs() {
    const REFERENCE: [[&str; 7]; 10] = [
        [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ], // 0
        [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ], // 1
        [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ], // 2
        [
            "11110", "00001", "00001", "00110", "00001", "00001", "11110",
        ], // 3
        [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ], // 4
        [
            "11111", "10000", "11110", "00001", "00001", "10001", "01110",
        ], // 5
        [
            "00110", "01000", "10000", "11110", "10001", "10001", "01110",
        ], // 6
        [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ], // 7
        [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ], // 8
        [
            "01110", "10001", "10001", "01111", "00001", "00010", "01100",
        ], // 9
    ];
    for (digit, rows) in REFERENCE.iter().enumerate() {
        for (row, expected) in rows.iter().enumerate() {
            let expected = u8::from_str_radix(expected, 2).unwrap();
            assert_eq!(
                DIGIT_FONT[digit][row], expected,
                "digit {digit} row {row}: expected {expected:05b}, got {:05b}",
                DIGIT_FONT[digit][row],
            );
        }
    }
}
