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
#[path = "../tests/unit/aec.rs"]
mod tests;
