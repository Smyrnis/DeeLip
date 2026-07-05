use super::*;
use std::sync::{Arc, Mutex};

fn push_ref(echo_ref: &EchoRefBuf, samples: &[i16]) {
    echo_ref.lock().unwrap().extend(samples.iter().copied());
}

fn frame_energy(frame: &[i16]) -> f64 {
    frame.iter().map(|&s| (s as f64) * (s as f64)).sum()
}

#[test]
fn silence_in_silence_out() {
    let mut canceller = EchoCanceller::new();
    let echo_ref: EchoRefBuf = Arc::new(Mutex::new(VecDeque::new()));
    let mic = vec![0i16; 160];

    for _ in 0..10 {
        push_ref(&echo_ref, &mic);
        let out = canceller.process(&mic, &echo_ref);
        assert_eq!(out.len(), 160);
        assert!(out.iter().all(|&s| s.abs() < 100), "silence in should stay near-silent out, got {out:?}");
    }
}

#[test]
fn always_returns_the_input_frame_length() {
    let mut canceller = EchoCanceller::new();
    let echo_ref: EchoRefBuf = Arc::new(Mutex::new(VecDeque::new()));
    let mic = vec![1000i16; 160];

    for _ in 0..20 {
        push_ref(&echo_ref, &mic);
        let out = canceller.process(&mic, &echo_ref);
        assert_eq!(out.len(), 160, "must always return exactly one RTP frame's worth");
    }
}

#[test]
fn converges_on_a_repeated_perfect_echo() {
    let mut canceller = EchoCanceller::new();
    let echo_ref: EchoRefBuf = Arc::new(Mutex::new(VecDeque::new()));

    // A simple tone, used as both "what's playing" and "what the mic hears" —
    // simulates an undelayed, perfectly correlated acoustic echo.
    let tone: Vec<i16> = (0..160)
        .map(|i| ((i as f32 * 0.3).sin() * 8000.0) as i16)
        .collect();

    let mut first_energy = None;
    let mut last_energy = 0.0;
    for i in 0..200 {
        push_ref(&echo_ref, &tone);
        let out = canceller.process(&tone, &echo_ref);
        let energy = frame_energy(&out);
        if i == 0 { first_energy = Some(energy.max(1.0)); }
        last_energy = energy;
    }

    let first = first_energy.unwrap();
    assert!(
        last_energy < first * 0.5,
        "adaptive filter should suppress a repeated, perfectly-correlated echo over time \
         (first={first}, last={last_energy})"
    );
}
