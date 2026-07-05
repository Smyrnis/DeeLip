//! Simple adaptive microphone gain control -- a feedback loop that nudges a
//! linear gain multiplier toward a target RMS level every frame, with a
//! hard clip-safety ceiling. Runs in `MediaEngine`'s send task alongside
//! echo cancellation, not the realtime cpal capture callback (see
//! `crate::aec`'s module doc for why that placement is safe/cheap here).

/// ~ -14 dBFS RMS -- a comfortable mid-level target that leaves headroom
/// before clipping even on a peaky voice signal.
const TARGET_RMS: f32 = 0.2;
const MIN_GAIN: f32 = 0.5;
const MAX_GAIN: f32 = 8.0;
/// Fraction of the way from the current gain toward the ideal gain moved
/// per 20ms frame -- limits how fast gain can change so it doesn't audibly
/// "pump" by snapping straight to the ideal value on every frame.
const ADAPT_RATE: f32 = 0.05;
/// Below this RMS, treat the frame as silence and leave gain alone rather
/// than dividing by a near-zero denominator (which would otherwise blow
/// gain up toward `MAX_GAIN` during any pause in speech).
const SILENCE_RMS: f32 = 1e-4;

pub struct AutomaticGainControl {
    gain: f32,
}

impl AutomaticGainControl {
    pub fn new() -> Self {
        Self { gain: 1.0 }
    }

    pub fn process(&mut self, pcm: &[i16]) -> Vec<i16> {
        if pcm.is_empty() {
            return Vec::new();
        }
        let sum_sq: f64 = pcm
            .iter()
            .map(|&s| {
                let f = s as f64 / i16::MAX as f64;
                f * f
            })
            .sum();
        let rms = (sum_sq / pcm.len() as f64).sqrt() as f32;

        if rms > SILENCE_RMS {
            let desired_gain = (TARGET_RMS / rms).clamp(MIN_GAIN, MAX_GAIN);
            self.gain += (desired_gain - self.gain) * ADAPT_RATE;
        }

        pcm.iter()
            .map(|&s| (s as f32 * self.gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
            .collect()
    }
}

impl Default for AutomaticGainControl {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../tests/unit/agc.rs"]
mod tests;
