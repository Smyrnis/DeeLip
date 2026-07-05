//! Energy-threshold voice activity detection driving RFC 3389 comfort-noise
//! generation -- during a detected silence, the send task stops encoding
//! and sending continuous audio and instead sends an occasional Comfort
//! Noise/SID packet carrying a noise-level byte, exactly like the AGC/AEC
//! stages this sits alongside in `MediaEngine`'s send task (see their own
//! module docs for why this non-realtime placement is fine).
//!
//! Simplification worth being explicit about: this only sends a fresh SID
//! packet at silence onset and then periodically (`CN_REPEAT_FRAMES`) while
//! silence continues -- it does not attempt to synthesize *continuous*
//! comfort noise on the receive side spanning the gaps between SID packets
//! (see `generate_comfort_noise` in this module and its call site in
//! `engine.rs`), which would need a background filler tied to the jitter
//! buffer's own playout clock rather than packet arrival. Between SID
//! updates the receive side falls back to plain silence (the jitter
//! buffer's existing zero-pad-on-underrun behavior), not a continuous
//! background hiss.

/// ~ -32 dBFS RMS -- quieter than this is treated as silence. Well below
/// the AGC's ~ -14 dBFS target level, so the two don't fight (AGC only
/// meaningfully boosts once a talk spurt is already underway).
const SILENCE_RMS: f32 = 0.025;
/// Consecutive quiet frames required before switching into the Silent
/// state (300ms) -- avoids treating a natural pause mid-sentence as real
/// silence and clipping the front of the next word's comfort-noise ramp-down.
const HANGOVER_FRAMES: u32 = 15;
/// How often (in frames) to re-send a SID packet while continuously
/// silent, so a lost SID doesn't leave the far end stuck on a stale noise
/// level indefinitely (RFC 3389 recommends periodic repetition).
const CN_REPEAT_FRAMES: u32 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    /// Encode and send this frame normally.
    Talking,
    /// Send a comfort-noise (SID) packet with this noise level byte instead
    /// of encoding real audio.
    SendComfortNoise(u8),
    /// Silence continues and no SID is due yet -- send nothing this tick
    /// (but the caller must still advance its RTP timestamp, see
    /// `RtpSender::skip_tick`).
    Skip,
}

pub struct VoiceActivityDetector {
    quiet_frames: u32,
    silent: bool,
    frames_since_sid: u32,
}

impl VoiceActivityDetector {
    pub fn new() -> Self {
        Self {
            quiet_frames: 0,
            silent: false,
            frames_since_sid: 0,
        }
    }

    pub fn process(&mut self, pcm: &[i16]) -> VadDecision {
        let rms = frame_rms(pcm);

        if rms > SILENCE_RMS {
            self.quiet_frames = 0;
            self.silent = false;
            return VadDecision::Talking;
        }

        if !self.silent {
            self.quiet_frames += 1;
            if self.quiet_frames < HANGOVER_FRAMES {
                return VadDecision::Talking;
            }
            // Hangover elapsed -- silence confirmed, send the initial SID.
            self.silent = true;
            self.frames_since_sid = 0;
            return VadDecision::SendComfortNoise(noise_level_byte(rms));
        }

        self.frames_since_sid += 1;
        if self.frames_since_sid >= CN_REPEAT_FRAMES {
            self.frames_since_sid = 0;
            VadDecision::SendComfortNoise(noise_level_byte(rms))
        } else {
            VadDecision::Skip
        }
    }
}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn frame_rms(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = pcm
        .iter()
        .map(|&s| {
            let f = s as f64 / i16::MAX as f64;
            f * f
        })
        .sum();
    (sum_sq / pcm.len() as f64).sqrt() as f32
}

/// RFC 3389 encodes comfort-noise level as one octet representing the level
/// in -dBov (dB below full scale) -- bigger byte value means quieter
/// background noise. Inverse of `synthesize_comfort_noise`'s decode.
fn noise_level_byte(rms: f32) -> u8 {
    let db_below_full_scale = -20.0 * rms.max(1e-5).log10();
    db_below_full_scale.round().clamp(0.0, 127.0) as u8
}

/// Cheap, dependency-free xorshift PRNG state for synthesizing comfort
/// noise on the receive side -- doesn't need to be cryptographically
/// random, just varied enough that repeated comfort-noise frames don't
/// sound identical.
pub struct ComfortNoiseState(u32);

impl ComfortNoiseState {
    pub fn new() -> Self {
        // Any nonzero seed works for xorshift; the exact value is arbitrary.
        Self(0x9E3779B9)
    }

    fn next_uniform(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

impl Default for ComfortNoiseState {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate `len` samples of synthetic white-noise comfort noise at
/// roughly the level `level_byte` (an RFC 3389 SID payload's first octet)
/// indicates -- played into the jitter buffer in place of real decoded
/// audio for a received comfort-noise packet.
pub fn synthesize_comfort_noise(level_byte: u8, len: usize, state: &mut ComfortNoiseState) -> Vec<i16> {
    let target_rms = 10f32.powf(-(level_byte as f32) / 20.0);
    let amplitude = target_rms * i16::MAX as f32;
    (0..len)
        .map(|_| (state.next_uniform() * amplitude) as i16)
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/vad.rs"]
mod tests;
