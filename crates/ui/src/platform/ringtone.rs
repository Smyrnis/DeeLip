//! Ring cadence for incoming calls and outgoing ringback. Incoming can be a
//! user-picked WAV file; both fall back to a synthesized two-tone cadence
//! generated sine waves rather than a bundled audio file, since there's no
//! license-safe ringtone asset to embed in the repo, or if no file is set or
//! it fails to load.

use std::fs::File;
use std::io::BufReader;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

#[derive(Clone, Copy, PartialEq, Eq)]
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
    /// `device_name`: cpal output device to ring through, `None` for the
    /// system default -- independent of the in-call audio device, so a
    /// headset can handle the call while the ring itself plays on speakers.
    /// `ringtone_file`: a WAV path to play instead of the synthesized tone,
    /// consulted only for `RingKind::Incoming`; falls back to the synthesized
    /// cadence if unset or it fails to load/decode (a bad ringtone file
    /// should never mean a silently-missed call). `volume`: linear gain
    /// applied via `Sink::set_volume`, uniformly across both the custom-WAV
    /// and synthesized-tone paths -- `1.0` is unchanged/full volume.
    pub fn start(
        kind: RingKind, device_name: Option<&str>, ringtone_file: Option<&str>, volume: f32,
    ) -> anyhow::Result<Self> {
        let (stream, handle) = open_stream(device_name)?;

        let sink = Sink::try_new(&handle)?;
        sink.set_volume(volume);
        if kind == RingKind::Incoming {
            if let Some(path) = ringtone_file {
                match load_wav_looped(path) {
                    Ok(source) => {
                        sink.append(source);
                        sink.play();
                        return Ok(Self { _stream: stream, sink });
                    }
                    Err(e) => tracing::warn!("Custom ringtone {path} failed to load ({e}), using built-in tone"),
                }
            }
        }
        sink.append(RingSource::new(kind));
        sink.play();
        Ok(Self { _stream: stream, sink })
    }
}

/// Opens the named cpal output device if given and found; otherwise (or on
/// any failure to match it) falls back to the system default, exactly as
/// before this device-selection option existed.
fn open_stream(device_name: Option<&str>) -> anyhow::Result<(OutputStream, OutputStreamHandle)> {
    if let Some(name) = device_name {
        let found = cpal::default_host()
            .output_devices()
            .ok()
            .and_then(|mut devices| devices.find(|d| d.name().is_ok_and(|n| n == name)));
        if let Some(device) = found {
            return Ok(OutputStream::try_from_device(&device)?);
        }
        tracing::warn!("Ringtone device {name} not found, using system default");
    }
    Ok(OutputStream::try_default()?)
}

/// Decode `path` as WAV and loop it indefinitely for as long as the ringtone
/// plays -- a single ring cycle in the file repeats until the `Ringtone` (and
/// therefore its `Sink`) is dropped.
fn load_wav_looped(path: &str) -> anyhow::Result<impl Source<Item = i16> + Send + 'static> {
    let file = File::open(path)?;
    let source = Decoder::new(BufReader::new(file))?;
    Ok(source.repeat_infinite())
}

impl Drop for Ringtone {
    fn drop(&mut self) {
        self.sink.stop();
    }
}

/// Infinite generated cadence: a two-tone chord for `on_samples`, then
/// silence for the rest of `period_samples`, repeating forever.
struct RingSource {
    sample_rate: u32,
    on_samples: usize,
    period_samples: usize,
    freqs: (f32, f32),
    phase: usize,
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
            on_samples: (on_secs * sample_rate as f32) as usize,
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
            0.12 * ((2.0 * std::f32::consts::PI * f1 * t).sin() + (2.0 * std::f32::consts::PI * f2 * t).sin())
        } else {
            0.0
        };
        self.phase = self.phase.wrapping_add(1);
        Some(sample)
    }
}

impl Source for RingSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
