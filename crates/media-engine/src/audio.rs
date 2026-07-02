use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use tokio::sync::mpsc;

pub const SAMPLE_RATE: u32    = 8_000;
pub const FRAME_SAMPLES: usize = 160; // 20ms at 8000 Hz

/// Captured PCM frames from the microphone.
pub type CaptureRx = mpsc::UnboundedReceiver<Vec<i16>>;
/// PCM frames to be played back.
pub type PlaybackTx = Arc<Mutex<VecDeque<i16>>>;

/// Holds the live cpal streams (dropped = stopped).
pub struct AudioStreams {
    _input:  cpal::Stream,
    _output: cpal::Stream,
}

/// Open default input + output devices at 8 kHz mono.
/// Returns the streams (keep alive), a receiver for captured audio,
/// and a shared jitter buffer to push playback audio into.
pub fn open_streams() -> anyhow::Result<(AudioStreams, CaptureRx, PlaybackTx)> {
    let host = cpal::default_host();

    let in_dev = host.default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No default input device"))?;
    let out_dev = host.default_output_device()
        .ok_or_else(|| anyhow::anyhow!("No default output device"))?;

    let config = StreamConfig {
        channels:    1,
        sample_rate: SampleRate(SAMPLE_RATE),
        buffer_size: cpal::BufferSize::Default,
    };

    // ── Capture ───────────────────────────────────────────────────────────────
    let (cap_tx, cap_rx) = mpsc::unbounded_channel::<Vec<i16>>();

    let input_stream = match in_dev.default_input_config()?.sample_format() {
        SampleFormat::I16 => build_input_i16(&in_dev, &config, cap_tx)?,
        SampleFormat::F32 => build_input_f32(&in_dev, &config, cap_tx)?,
        fmt => anyhow::bail!("Unsupported input sample format: {fmt:?}"),
    };

    // ── Playback ──────────────────────────────────────────────────────────────
    let jitter: PlaybackTx = Arc::new(Mutex::new(VecDeque::with_capacity(4800)));
    let jitter_out         = jitter.clone();

    let output_stream = match out_dev.default_output_config()?.sample_format() {
        SampleFormat::I16 => build_output_i16(&out_dev, &config, jitter_out)?,
        SampleFormat::F32 => build_output_f32(&out_dev, &config, jitter_out)?,
        fmt => anyhow::bail!("Unsupported output sample format: {fmt:?}"),
    };

    input_stream.play().context("Starting input stream")?;
    output_stream.play().context("Starting output stream")?;

    Ok((
        AudioStreams { _input: input_stream, _output: output_stream },
        cap_rx,
        jitter,
    ))
}

// ── I16 paths ─────────────────────────────────────────────────────────────────

fn build_input_i16(
    device: &cpal::Device,
    config: &StreamConfig,
    tx: mpsc::UnboundedSender<Vec<i16>>,
) -> anyhow::Result<cpal::Stream> {
    let mut buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES);
    let stream = device.build_input_stream(
        config,
        move |data: &[i16], _| {
            for &s in data {
                buf.push(s);
                if buf.len() >= FRAME_SAMPLES {
                    let _ = tx.send(buf.clone());
                    buf.clear();
                }
            }
        },
        |e| tracing::error!("Input stream error: {e}"),
        None,
    ).context("Building I16 input stream")?;
    Ok(stream)
}

fn build_output_i16(
    device: &cpal::Device,
    config: &StreamConfig,
    jitter: PlaybackTx,
) -> anyhow::Result<cpal::Stream> {
    let stream = device.build_output_stream(
        config,
        move |data: &mut [i16], _| {
            let mut buf = jitter.lock().unwrap();
            for s in data.iter_mut() {
                *s = buf.pop_front().unwrap_or(0);
            }
        },
        |e| tracing::error!("Output stream error: {e}"),
        None,
    ).context("Building I16 output stream")?;
    Ok(stream)
}

// ── F32 paths (convert to/from i16) ──────────────────────────────────────────

fn build_input_f32(
    device: &cpal::Device,
    config: &StreamConfig,
    tx: mpsc::UnboundedSender<Vec<i16>>,
) -> anyhow::Result<cpal::Stream> {
    let mut buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES);
    let stream = device.build_input_stream(
        config,
        move |data: &[f32], _| {
            for &s in data {
                buf.push((s * i16::MAX as f32) as i16);
                if buf.len() >= FRAME_SAMPLES {
                    let _ = tx.send(buf.clone());
                    buf.clear();
                }
            }
        },
        |e| tracing::error!("Input stream error: {e}"),
        None,
    ).context("Building F32 input stream")?;
    Ok(stream)
}

fn build_output_f32(
    device: &cpal::Device,
    config: &StreamConfig,
    jitter: PlaybackTx,
) -> anyhow::Result<cpal::Stream> {
    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _| {
            let mut buf = jitter.lock().unwrap();
            for s in data.iter_mut() {
                *s = buf.pop_front().unwrap_or(0) as f32 / i16::MAX as f32;
            }
        },
        |e| tracing::error!("Output stream error: {e}"),
        None,
    ).context("Building F32 output stream")?;
    Ok(stream)
}
