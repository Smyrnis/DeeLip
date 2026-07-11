//! Call-recording writers -- a stereo (left = near-end mic, right = far-end
//! received/mixed) file for the duration of a call, in either WAV (`hound`,
//! lossless) or MP3 (`mp3lame-encoder`, lossy but far smaller) format.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Context;
use mp3lame_encoder::{max_required_buffer_size, Bitrate, Builder, Encoder, FlushNoGap, InterleavedPcm, Quality};

use deelip_config::RecordingFormat;

use crate::audio::SAMPLE_RATE;

pub type WavWriter = hound::WavWriter<BufWriter<File>>;

/// Replace anything outside `[A-Za-z0-9._-]` with `_` (SIP Call-IDs can
/// contain `@` and other characters not safe verbatim in a filename).
fn sanitize_filename(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect()
}

/// Whether/how/where to record a call -- bundled into one struct since
/// these three always travel together at `MediaEngine::start`'s call sites.
#[derive(Debug, Clone, Default)]
pub struct RecordingOptions {
    pub enabled: bool,
    pub format: RecordingFormat,
    pub dir_override: Option<String>,
}

pub enum RecordingWriter {
    Wav(WavWriter),
    Mp3(Mp3Writer),
}

impl RecordingWriter {
    /// Open a recording for `call_id` under `dir_override` (or the default
    /// `recordings_dir()` if unset), in `format`.
    pub fn create(call_id: &str, dir_override: Option<&str>, format: RecordingFormat) -> anyhow::Result<Self> {
        let dir = deelip_config::recordings_dir(dir_override).context("Resolving recordings dir")?;
        let timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let stem = format!("{timestamp}_{}", sanitize_filename(call_id));
        match format {
            RecordingFormat::Wav => {
                let path = dir.join(format!("{stem}.wav"));
                let spec = hound::WavSpec {
                    channels: 2,
                    sample_rate: SAMPLE_RATE,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let writer = hound::WavWriter::create(&path, spec)
                    .with_context(|| format!("Creating recording at {}", path.display()))?;
                Ok(Self::Wav(writer))
            }
            RecordingFormat::Mp3 => {
                let path = dir.join(format!("{stem}.mp3"));
                Ok(Self::Mp3(Mp3Writer::create(&path)?))
            }
        }
    }

    /// Write one interleaved stereo frame (left = near-end `near`, right =
    /// the already-mixed far-end audio for this same frame). Errors are
    /// logged by the caller, not propagated -- a single bad frame shouldn't
    /// abort an otherwise-fine recording.
    pub fn write_frame(&mut self, near: &[i16], far: &[i16]) -> anyhow::Result<()> {
        match self {
            Self::Wav(w) => {
                for (i, &near_sample) in near.iter().enumerate() {
                    let far_sample = far.get(i).copied().unwrap_or(0);
                    w.write_sample(near_sample)?;
                    w.write_sample(far_sample)?;
                }
                Ok(())
            }
            Self::Mp3(w) => w.write_frame(near, far),
        }
    }

    /// Flush/finalize the underlying file -- WAV needs this to fix up the
    /// RIFF header sizes; MP3 needs it to flush the encoder's last partial
    /// frame. Drop alone would leave either format malformed/truncated.
    pub fn finalize(self) -> anyhow::Result<()> {
        match self {
            Self::Wav(w) => w.finalize().context("Finalizing WAV recording"),
            Self::Mp3(w) => w.finalize(),
        }
    }
}

pub struct Mp3Writer {
    encoder: Encoder,
    file: BufWriter<File>,
    /// Reused across `write_frame` calls purely to avoid a fresh
    /// allocation every 20ms frame.
    interleave_buf: Vec<i16>,
    encode_buf: Vec<u8>,
}

impl Mp3Writer {
    fn create(path: &Path) -> anyhow::Result<Self> {
        let encoder = Builder::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to create LAME encoder"))?
            .with_num_channels(2)
            .map_err(|e| anyhow::anyhow!("LAME set channels: {e}"))?
            .with_sample_rate(SAMPLE_RATE)
            .map_err(|e| anyhow::anyhow!("LAME set sample rate: {e}"))?
            .with_brate(Bitrate::Kbps128)
            .map_err(|e| anyhow::anyhow!("LAME set bitrate: {e}"))?
            .with_quality(Quality::Good)
            .map_err(|e| anyhow::anyhow!("LAME set quality: {e}"))?
            .build()
            .map_err(|e| anyhow::anyhow!("LAME build: {e}"))?;
        let file = File::create(path).with_context(|| format!("Creating recording at {}", path.display()))?;
        Ok(Self { encoder, file: BufWriter::new(file), interleave_buf: Vec::new(), encode_buf: Vec::new() })
    }

    fn write_frame(&mut self, near: &[i16], far: &[i16]) -> anyhow::Result<()> {
        self.interleave_buf.clear();
        for (i, &near_sample) in near.iter().enumerate() {
            let far_sample = far.get(i).copied().unwrap_or(0);
            self.interleave_buf.push(near_sample);
            self.interleave_buf.push(far_sample);
        }
        self.encode_buf.clear();
        // `encode_to_vec` writes into the vec's *spare* capacity directly (see
        // its own doc example) -- an unreserved `Vec` has none, and LAME will
        // write past it regardless, corrupting the heap. Reserving here is a
        // no-op on every call after the first, since `clear()` keeps capacity.
        self.encode_buf.reserve(max_required_buffer_size(near.len()));
        self.encoder
            .encode_to_vec(InterleavedPcm(&self.interleave_buf), &mut self.encode_buf)
            .map_err(|e| anyhow::anyhow!("MP3 encode: {e}"))?;
        self.file.write_all(&self.encode_buf).context("Writing MP3 data")
    }

    fn finalize(mut self) -> anyhow::Result<()> {
        // Same reservation requirement as `write_frame`'s `encode_to_vec` --
        // `max_required_buffer_size(0)` still accounts for the fixed 7200-byte
        // frame headroom the flush needs.
        let mut tail = Vec::with_capacity(max_required_buffer_size(0));
        self.encoder.flush_to_vec::<FlushNoGap>(&mut tail).map_err(|e| anyhow::anyhow!("MP3 flush: {e}"))?;
        self.file.write_all(&tail).context("Writing final MP3 frame")?;
        self.file.flush().context("Flushing MP3 file")
    }
}

#[cfg(test)]
#[path = "../tests/unit/recording.rs"]
mod tests;
