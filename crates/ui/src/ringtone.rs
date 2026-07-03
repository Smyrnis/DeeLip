//! Synthesized ring cadence for incoming calls and outgoing ringback —
//! generated sine waves rather than a bundled audio file, since there's no
//! license-safe ringtone asset to embed in the repo.

use std::time::Duration;

use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

#[derive(Clone, Copy)]
pub enum RingKind {
    /// Played while a call is ringing at us, unanswered.
    Incoming,
    /// Played locally while dialing out and waiting for the far end to
    /// answer — not the real network ringback (no SDP early-media/183
    /// support), just a synthesized cue.
    Outgoing,
}

/// Owns the output stream/sink for as long as the ringtone should play;
/// dropping it stops the sound.
pub struct Ringtone {
    // Never read, but must stay alive — dropping it tears down the audio
    // backend and silences `sink`.
    _stream: OutputStream,
    sink: Sink,
}

impl Ringtone {
    pub fn start(kind: RingKind) -> anyhow::Result<Self> {
        let (stream, handle): (OutputStream, OutputStreamHandle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&handle)?;
        sink.append(RingSource::new(kind));
        sink.play();
        Ok(Self { _stream: stream, sink })
    }
}

impl Drop for Ringtone {
    fn drop(&mut self) {
        self.sink.stop();
    }
}

/// Infinite generated cadence: a two-tone chord for `on_samples`, then
/// silence for the rest of `period_samples`, repeating forever.
struct RingSource {
    sample_rate:    u32,
    on_samples:     usize,
    period_samples: usize,
    freqs:          (f32, f32),
    phase:          usize,
}

impl RingSource {
    fn new(kind: RingKind) -> Self {
        let sample_rate = 48_000;
        let (freqs, on_secs, period_secs) = match kind {
            RingKind::Incoming => ((480.0, 620.0), 1.2, 3.0),
            RingKind::Outgoing => ((440.0, 480.0), 1.0, 3.0),
        };
        Self {
            sample_rate,
            on_samples:     (on_secs * sample_rate as f32) as usize,
            period_samples: (period_secs * sample_rate as f32) as usize,
            freqs,
            phase: 0,
        }
    }
}

impl Iterator for RingSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let pos = self.phase % self.period_samples;
        let sample = if pos < self.on_samples {
            let t = self.phase as f32 / self.sample_rate as f32;
            let (f1, f2) = self.freqs;
            0.12 * ((2.0 * std::f32::consts::PI * f1 * t).sin()
                + (2.0 * std::f32::consts::PI * f2 * t).sin())
        } else {
            0.0
        };
        self.phase = self.phase.wrapping_add(1);
        Some(sample)
    }
}

impl Source for RingSource {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { 1 }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> { None }
}
