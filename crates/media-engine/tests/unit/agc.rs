use super::*;

fn frame_rms(frame: &[i16]) -> f32 {
    let sum_sq: f64 = frame
        .iter()
        .map(|&s| {
            let f = s as f64 / i16::MAX as f64;
            f * f
        })
        .sum();
    (sum_sq / frame.len() as f64).sqrt() as f32
}

#[test]
fn silence_stays_silent() {
    let mut agc = AutomaticGainControl::new();
    let silence = vec![0i16; 160];
    for _ in 0..10 {
        let out = agc.process(&silence);
        assert!(out.iter().all(|&s| s == 0));
    }
}

#[test]
fn quiet_signal_gets_boosted_toward_target() {
    let mut agc = AutomaticGainControl::new();
    let quiet: Vec<i16> = (0..160).map(|i| ((i as f32 * 0.3).sin() * 500.0) as i16).collect();
    let quiet_rms = frame_rms(&quiet);

    let mut out = quiet.clone();
    for _ in 0..200 {
        out = agc.process(&quiet);
    }
    let out_rms = frame_rms(&out);
    assert!(
        out_rms > quiet_rms * 1.5,
        "a quiet signal should end up louder after AGC converges (in={quiet_rms}, out={out_rms})"
    );
}

#[test]
fn loud_signal_gets_attenuated_toward_target() {
    let mut agc = AutomaticGainControl::new();
    let loud: Vec<i16> = (0..160).map(|i| ((i as f32 * 0.3).sin() * i16::MAX as f32 * 0.9) as i16).collect();
    let loud_rms = frame_rms(&loud);

    let mut out = loud.clone();
    for _ in 0..200 {
        out = agc.process(&loud);
    }
    let out_rms = frame_rms(&out);
    assert!(
        out_rms < loud_rms * 0.7,
        "a loud signal should end up quieter after AGC converges (in={loud_rms}, out={out_rms})"
    );
}
