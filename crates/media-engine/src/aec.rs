//! Acoustic echo cancellation, bridging the mismatch between DeeLip's fixed
//! 160-sample (20ms @ 8kHz) RTP framing and `fdaf-aec`'s power-of-two frame
//! size requirement. Runs non-realtime (in `MediaEngine`'s send task, not the
//! cpal capture callback) since it does FFT + allocation work per frame.

use std::collections::VecDeque;

use crate::audio::EchoRefBuf;

/// fdaf-aec requires a power-of-two FFT size; frame_size = fft_size / 2.
/// 128 samples (16ms @ 8kHz) doesn't evenly divide our 160-sample RTP frame,
/// but LCM(160, 128) = 640 — every 4 RTP frames align exactly with 5 AEC
/// hops, so steady state has no drift, just a small fixed buffering delay.
const AEC_FFT_SIZE: usize = 256;
const AEC_FRAME_SAMPLES: usize = AEC_FFT_SIZE / 2;
/// Deliberately conservative: empirically, step sizes above ~0.1 cause a large
/// transient energy blow-up (verified with a standalone repeated-tone test —
/// e.g. mu=0.5 spikes to ~150x the input energy before eventually recovering)
/// before eventually converging. 0.05 converges smoothly with no overshoot.
const AEC_STEP_SIZE: f32 = 0.05;

pub struct EchoCanceller {
    aec: fdaf_aec::FdafAec,
    mic_acc: VecDeque<i16>,
    ref_acc: VecDeque<i16>,
    out_acc: VecDeque<i16>,
}

impl EchoCanceller {
    pub fn new() -> Self {
        Self {
            aec: fdaf_aec::FdafAec::new(AEC_FFT_SIZE, AEC_STEP_SIZE),
            mic_acc: VecDeque::new(),
            ref_acc: VecDeque::new(),
            out_acc: VecDeque::new(),
        }
    }

    /// Feed one mic frame plus the matching amount of far-end reference
    /// audio pulled from `echo_ref` (zero-padded on underrun, e.g. before
    /// playback has started), returning an echo-cancelled frame of the same
    /// length to encode instead of the raw mic input.
    pub fn process(&mut self, mic: &[i16], echo_ref: &EchoRefBuf) -> Vec<i16> {
        let want = mic.len();
        self.mic_acc.extend(mic.iter().copied());

        {
            let mut buf = echo_ref.lock().unwrap();
            for _ in 0..want {
                self.ref_acc.push_back(buf.pop_front().unwrap_or(0));
            }
        }

        while self.mic_acc.len() >= AEC_FRAME_SAMPLES && self.ref_acc.len() >= AEC_FRAME_SAMPLES {
            let mic_frame: Vec<f32> = self.mic_acc.drain(..AEC_FRAME_SAMPLES)
                .map(|s| s as f32 / i16::MAX as f32)
                .collect();
            let ref_frame: Vec<f32> = self.ref_acc.drain(..AEC_FRAME_SAMPLES)
                .map(|s| s as f32 / i16::MAX as f32)
                .collect();

            let out = self.aec.process(&ref_frame, &mic_frame);
            self.out_acc.extend(out.iter().map(|&s| (s * i16::MAX as f32) as i16));
        }

        let ready = self.out_acc.len().min(want);
        let mut result: Vec<i16> = self.out_acc.drain(..ready).collect();
        if result.len() < want {
            // Startup transient only (before enough AEC hops have run) —
            // degrade gracefully with uncancelled mic audio rather than
            // gapping the stream.
            let shortfall = want - result.len();
            result.extend_from_slice(&mic[mic.len() - shortfall..]);
        }
        result
    }
}

impl Default for EchoCanceller {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
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
}
