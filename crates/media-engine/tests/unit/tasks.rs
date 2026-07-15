use super::*;

#[test]
fn mix_frames_sums_and_clamps() {
    let a = vec![100i16, -100, i16::MAX, i16::MIN];
    let b = vec![50i16, -50, i16::MAX, i16::MIN];
    let mixed = mix_frames(&a, &b);
    // Each leg halved (integer truncation) before summing:
    // 100/2 + 50/2 = 75; -100/2 + -50/2 = -75;
    // MAX/2 + MAX/2 = 32766 (truncation loses 1); MIN/2 + MIN/2 = MIN exactly.
    assert_eq!(mixed, vec![75, -75, 32766, i16::MIN]);
}
