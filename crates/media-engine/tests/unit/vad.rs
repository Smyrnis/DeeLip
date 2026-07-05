use super::*;

fn loud_frame() -> Vec<i16> {
    (0..160)
        .map(|i| ((i as f32 * 0.3).sin() * 10000.0) as i16)
        .collect()
}

#[test]
fn stays_talking_while_loud() {
    let mut vad = VoiceActivityDetector::new();
    let frame = loud_frame();
    for _ in 0..50 {
        assert_eq!(vad.process(&frame), VadDecision::Talking);
    }
}

#[test]
fn silence_stays_talking_through_hangover_then_switches() {
    let mut vad = VoiceActivityDetector::new();
    let silence = vec![0i16; 160];

    let mut saw_comfort_noise = false;
    for i in 0..(HANGOVER_FRAMES + 5) {
        match vad.process(&silence) {
            VadDecision::Talking => assert!(
                i < HANGOVER_FRAMES,
                "should have switched out of Talking by frame {i}"
            ),
            VadDecision::SendComfortNoise(_) => saw_comfort_noise = true,
            VadDecision::Skip => {}
        }
    }
    assert!(saw_comfort_noise, "silence should eventually trigger a SID packet");
}

#[test]
fn returns_to_talking_immediately_when_loud_again() {
    let mut vad = VoiceActivityDetector::new();
    let silence = vec![0i16; 160];
    let frame = loud_frame();

    for _ in 0..(HANGOVER_FRAMES + 2) {
        vad.process(&silence);
    }
    assert_eq!(vad.process(&frame), VadDecision::Talking);
}

#[test]
fn resends_comfort_noise_periodically_during_sustained_silence() {
    let mut vad = VoiceActivityDetector::new();
    let silence = vec![0i16; 160];

    for _ in 0..(HANGOVER_FRAMES) {
        vad.process(&silence);
    }
    let mut sid_count = 0;
    for _ in 0..(CN_REPEAT_FRAMES * 2 + 1) {
        if matches!(vad.process(&silence), VadDecision::SendComfortNoise(_)) {
            sid_count += 1;
        }
    }
    assert!(sid_count >= 2, "expected periodic SID retransmission, got {sid_count}");
}

#[test]
fn noise_level_byte_is_monotonic_with_quietness() {
    let quiet = noise_level_byte(0.001);
    let louder = noise_level_byte(0.02);
    assert!(quiet > louder, "quieter RMS should map to a larger -dBov byte");
}

#[test]
fn synthesize_comfort_noise_returns_requested_length() {
    let mut state = ComfortNoiseState::new();
    let out = synthesize_comfort_noise(40, 160, &mut state);
    assert_eq!(out.len(), 160);
}
