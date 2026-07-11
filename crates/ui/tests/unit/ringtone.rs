use super::*;

#[test]
fn source_metadata_is_fixed_mono_48khz_indefinite() {
    let source = RingSource::new(RingKind::Incoming);
    assert_eq!(source.channels(), 1);
    assert_eq!(source.sample_rate(), 48_000);
    assert_eq!(source.current_frame_len(), None);
    assert_eq!(source.total_duration(), None);
}

#[test]
fn incoming_and_outgoing_use_different_cadences() {
    let incoming = RingSource::new(RingKind::Incoming);
    let outgoing = RingSource::new(RingKind::Outgoing);
    assert_ne!(incoming.freqs, outgoing.freqs);
    assert_ne!(incoming.on_samples, outgoing.on_samples);
}

#[test]
fn stays_silent_after_the_on_period_within_a_cycle() {
    let mut source = RingSource::new(RingKind::Outgoing);
    // 1.0s "on" out of a 3.0s period at 48kHz -- well past on_samples but
    // still inside the same period, must be silence (exactly 0.0).
    let silent_index = source.on_samples + 1000;
    let samples: Vec<f32> = (0..=silent_index).map(|_| source.next().unwrap()).collect();
    assert_eq!(samples[silent_index], 0.0);
}

#[test]
fn produces_a_nonzero_tone_shortly_into_the_on_period() {
    // Sample 0 itself is a sine zero-crossing (t=0) for both tones -- check
    // a little further in, still well inside `on_samples`.
    let mut source = RingSource::new(RingKind::Incoming);
    let sample = (0..100).map(|_| source.next().unwrap()).last().unwrap();
    assert_ne!(sample, 0.0);
}

#[test]
fn cadence_repeats_across_periods_within_float_precision() {
    let mut a = RingSource::new(RingKind::Incoming);
    let mut b = RingSource::new(RingKind::Incoming);
    // Advance `b` by exactly one full period so it's back at the same phase
    // within its cadence as `a` started at. The underlying sine phase is
    // never wrapped (see `next()`), so this is only equal up to `f32`
    // accumulation error over `period_samples` steps, not bit-exact.
    for _ in 0..a.period_samples {
        b.next();
    }
    for _ in 0..10 {
        let (sa, sb) = (a.next().unwrap(), b.next().unwrap());
        assert!((sa - sb).abs() < 1e-3, "{sa} vs {sb}");
    }
}

#[test]
fn never_clips_outside_the_unit_range() {
    let mut source = RingSource::new(RingKind::Incoming);
    for _ in 0..(source.period_samples * 2) {
        let sample = source.next().unwrap();
        assert!((-1.0..=1.0).contains(&sample), "sample {sample} out of range");
    }
}
