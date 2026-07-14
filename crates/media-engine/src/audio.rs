use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use tokio::sync::mpsc;

pub const SAMPLE_RATE: u32 = 8_000;
pub const FRAME_SAMPLES: usize = 160; // 20ms at 8000 Hz

/// Captured PCM frames from the microphone. Bounded (unlike the DTMF/ZRTP
/// channels elsewhere in this crate, which stay unbounded since they're
/// fed at human/protocol-handshake rates that can never realistically fill
/// one) -- this one is fed by the realtime capture callback every 20ms
/// regardless of whether the consumer (the send task, which can stall on a
/// congested/blocked network `send_to`) is keeping up. `CAPTURE_QUEUE_FRAMES`
/// matches the jitter/playback buffers' own 1s cap elsewhere in this crate.
/// The realtime callback uses `try_send`, so a full queue just drops the
/// newest frame (an audio glitch under sustained congestion) instead of
/// growing without bound.
const CAPTURE_QUEUE_FRAMES: usize = 50; // 1s at 20ms/frame
pub type CaptureRx = mpsc::Receiver<Vec<i16>>;
/// PCM frames to be played back.
pub type PlaybackTx = Arc<Mutex<VecDeque<i16>>>;
/// Far-end reference: a copy of every sample actually written to the output
/// device, for echo cancellation to compare against the mic capture.
pub type EchoRefBuf = Arc<Mutex<VecDeque<i16>>>;

/// Holds the live cpal streams (dropped = stopped).
pub struct AudioStreams {
    _input: cpal::Stream,
    _output: cpal::Stream,
}

/// Live-adjustable linear gain, shared between whichever thread wants to
/// change it (the UI, via `MediaEngine::set_output_gain`) and the realtime
/// cpal callback that reads it every sample -- stores an `f32`'s bits in an
/// `AtomicU32` since there's no `AtomicF32`. `1.0` is unity/unchanged.
pub type SharedGain = Arc<AtomicU32>;

pub fn new_shared_gain() -> SharedGain {
    Arc::new(AtomicU32::new(1.0f32.to_bits()))
}
pub fn load_gain(gain: &SharedGain) -> f32 {
    f32::from_bits(gain.load(Ordering::Relaxed))
}
pub fn store_gain(gain: &SharedGain, value: f32) {
    gain.store(value.to_bits(), Ordering::Relaxed);
}

/// Open input + output devices at 8 kHz mono -- `input_device`/`output_device`
/// name a specific cpal device to use, falling back to the system default if
/// unset or not found. See `docs/crates/media-engine.md` for the full audio data
/// flow this feeds into.
pub fn open_streams(
    input_device: Option<&str>, output_device: Option<&str>, echo_cancellation: bool,
) -> anyhow::Result<(AudioStreams, CaptureRx, PlaybackTx, Option<EchoRefBuf>, SharedGain)> {
    let host = cpal::default_host();

    let in_dev = find_device(&host, input_device, true).ok_or_else(|| anyhow::anyhow!("No default input device"))?;
    let out_dev =
        find_device(&host, output_device, false).ok_or_else(|| anyhow::anyhow!("No default output device"))?;

    let config =
        StreamConfig { channels: 1, sample_rate: SampleRate(SAMPLE_RATE), buffer_size: cpal::BufferSize::Default };

    // ── Capture ───────────────────────────────────────────────────────────────
    let (cap_tx, cap_rx) = mpsc::channel::<Vec<i16>>(CAPTURE_QUEUE_FRAMES);

    let input_stream = match in_dev.default_input_config()?.sample_format() {
        SampleFormat::I16 => build_input_i16(&in_dev, &config, cap_tx)?,
        SampleFormat::F32 => build_input_f32(&in_dev, &config, cap_tx)?,
        fmt => anyhow::bail!("Unsupported input sample format: {fmt:?}"),
    };

    // ── Playback ──────────────────────────────────────────────────────────────
    let jitter: PlaybackTx = Arc::new(Mutex::new(VecDeque::with_capacity(4800)));
    let jitter_out = jitter.clone();

    let echo_ref: Option<EchoRefBuf> = echo_cancellation.then(|| Arc::new(Mutex::new(VecDeque::with_capacity(4800))));
    let echo_ref_out = echo_ref.clone();

    let output_gain = new_shared_gain();
    let output_gain_out = output_gain.clone();

    let output_stream = match out_dev.default_output_config()?.sample_format() {
        SampleFormat::I16 => build_output_i16(&out_dev, &config, jitter_out, echo_ref_out, output_gain_out)?,
        SampleFormat::F32 => build_output_f32(&out_dev, &config, jitter_out, echo_ref_out, output_gain_out)?,
        fmt => anyhow::bail!("Unsupported output sample format: {fmt:?}"),
    };

    input_stream.play().context("Starting input stream")?;
    output_stream.play().context("Starting output stream")?;

    Ok((AudioStreams { _input: input_stream, _output: output_stream }, cap_rx, jitter, echo_ref, output_gain))
}

/// Find a named cpal device (input or output), falling back to the system
/// default if `name` is `None` or doesn't match any available device.
fn find_device(host: &cpal::Host, name: Option<&str>, is_input: bool) -> Option<cpal::Device> {
    let default = || {
        if is_input { host.default_input_device() } else { host.default_output_device() }
    };
    let Some(name) = name else { return default() };

    let mut devices: Box<dyn Iterator<Item = cpal::Device>> = if is_input {
        match host.input_devices() {
            Ok(d) => Box::new(d),
            Err(_) => return default(),
        }
    } else {
        match host.output_devices() {
            Ok(d) => Box::new(d),
            Err(_) => return default(),
        }
    };

    match devices.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
        Some(device) => Some(device),
        None => {
            tracing::warn!("Configured audio device {name:?} not found, using default");
            default()
        }
    }
}

fn push_frame_to_echo_ref(echo_ref: &Option<EchoRefBuf>, samples: &[i16]) {
    let Some(echo_ref) = echo_ref else { return };
    let max = FRAME_SAMPLES * 50; // cap at 1 second, mirrors push_to_jitter's bound in engine.rs
    let mut buf = echo_ref.lock().unwrap();
    for &s in samples {
        if buf.len() < max {
            buf.push_back(s);
        }
    }
}

// ── I16 paths ─────────────────────────────────────────────────────────────────

fn build_input_i16(
    device: &cpal::Device, config: &StreamConfig, tx: mpsc::Sender<Vec<i16>>,
) -> anyhow::Result<cpal::Stream> {
    let mut buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[i16], _| {
                for &s in data {
                    buf.push(s);
                    if buf.len() >= FRAME_SAMPLES {
                        // `mem::replace` moves the full frame into the
                        // channel with no copy, leaving a fresh
                        // pre-allocated Vec in `buf` for the next frame --
                        // was `buf.clone()` + `buf.clear()`, a real
                        // allocation+memcpy on this realtime thread.
                        // `try_send` (non-blocking, unlike the async `send`
                        // a bounded channel would otherwise require here)
                        // just drops this frame if the send task has fallen
                        // behind -- see `CaptureRx`'s doc comment.
                        let frame = std::mem::replace(&mut buf, Vec::with_capacity(FRAME_SAMPLES));
                        let _ = tx.try_send(frame);
                    }
                }
            },
            |e| tracing::error!("Input stream error: {e}"),
            None,
        )
        .context("Building I16 input stream")?;
    Ok(stream)
}

fn build_output_i16(
    device: &cpal::Device, config: &StreamConfig, jitter: PlaybackTx, echo_ref: Option<EchoRefBuf>, gain: SharedGain,
) -> anyhow::Result<cpal::Stream> {
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| {
                let g = load_gain(&gain);
                let mut buf = jitter.lock().unwrap();
                for s in data.iter_mut() {
                    let sample = buf.pop_front().unwrap_or(0);
                    *s = (sample as f32 * g).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                }
                drop(buf);
                push_frame_to_echo_ref(&echo_ref, data);
            },
            |e| tracing::error!("Output stream error: {e}"),
            None,
        )
        .context("Building I16 output stream")?;
    Ok(stream)
}

// ── F32 paths (convert to/from i16) ──────────────────────────────────────────

fn build_input_f32(
    device: &cpal::Device, config: &StreamConfig, tx: mpsc::Sender<Vec<i16>>,
) -> anyhow::Result<cpal::Stream> {
    let mut buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _| {
                for &s in data {
                    buf.push((s * i16::MAX as f32) as i16);
                    if buf.len() >= FRAME_SAMPLES {
                        // See build_input_i16's matching comment.
                        let frame = std::mem::replace(&mut buf, Vec::with_capacity(FRAME_SAMPLES));
                        let _ = tx.try_send(frame);
                    }
                }
            },
            |e| tracing::error!("Input stream error: {e}"),
            None,
        )
        .context("Building F32 input stream")?;
    Ok(stream)
}

fn build_output_f32(
    device: &cpal::Device, config: &StreamConfig, jitter: PlaybackTx, echo_ref: Option<EchoRefBuf>, gain: SharedGain,
) -> anyhow::Result<cpal::Stream> {
    let mut written: Vec<i16> = Vec::new();
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| {
                let g = load_gain(&gain);
                let mut buf = jitter.lock().unwrap();
                written.clear();
                for s in data.iter_mut() {
                    let sample =
                        (buf.pop_front().unwrap_or(0) as f32 * g).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                    *s = sample as f32 / i16::MAX as f32;
                    written.push(sample);
                }
                drop(buf);
                push_frame_to_echo_ref(&echo_ref, &written);
            },
            |e| tracing::error!("Output stream error: {e}"),
            None,
        )
        .context("Building F32 output stream")?;
    Ok(stream)
}

#[cfg(test)]
#[path = "../tests/unit/audio.rs"]
mod tests;
