//! H.264 encode/decode wrapper around `openh264` (self-compiled from
//! Cisco's BSD-2-licensed source). Used by `video_engine::VideoEngine` for
//! a live call's video leg via `video_rtp.rs`'s RTP packetization. Full
//! picture: `docs/crates/media-engine.md`.

use openh264::OpenH264API;
use openh264::decoder::Decoder as OpenH264Decoder;
use openh264::encoder::{BitRate, Encoder as OpenH264Encoder, EncoderConfig};
use openh264::formats::YUVSource;

/// Owned I420 (planar YUV 4:2:0) frame -- the pixel format both the
/// encoder and decoder operate on. `width`/`height` must be even (I420's
/// chroma planes are half-resolution in both dimensions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420Frame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv420Frame {
    /// A flat solid-color synthetic frame -- used by this module's and
    /// `video_rtp.rs`'s tests in place of a real camera frame (this sandbox
    /// has no camera device at all: no `/dev/video*`, no `uvcvideo` kernel
    /// module). Real capture is `video_capture.rs`; the encoder/decoder
    /// here don't care which source a `Yuv420Frame` came from.
    pub fn solid_color(width: u32, height: u32, y_val: u8, u_val: u8, v_val: u8) -> Self {
        let (cw, ch) = (width as usize / 2, height as usize / 2);
        Self {
            width,
            height,
            y: vec![y_val; width as usize * height as usize],
            u: vec![u_val; cw * ch],
            v: vec![v_val; cw * ch],
        }
    }
}

impl Yuv420Frame {
    /// Convert to packed RGB8 (BT.601, same convention as
    /// `video_capture::rgb8_to_yuv420`'s reverse direction) -- for UI
    /// display only (`egui::ColorImage`/`TextureHandle`). Not reused from
    /// `openh264::formats::yuv2rgb` since that module is `pub(crate)` in
    /// the `openh264` crate and not reachable from here.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let y_val = f32::from(self.y[y * w + x]);
                let cu = f32::from(self.u[(y / 2) * (w / 2) + (x / 2)]) - 128.0;
                let cv = f32::from(self.v[(y / 2) * (w / 2) + (x / 2)]) - 128.0;
                let y_scaled = 1.164 * (y_val - 16.0);
                let r = (y_scaled + 1.596 * cv).clamp(0.0, 255.0) as u8;
                let g = (y_scaled - 0.392 * cu - 0.813 * cv).clamp(0.0, 255.0) as u8;
                let b = (y_scaled + 2.017 * cu).clamp(0.0, 255.0) as u8;
                let i = (y * w + x) * 3;
                out[i] = r;
                out[i + 1] = g;
                out[i + 2] = b;
            }
        }
        out
    }
}

impl YUVSource for Yuv420Frame {
    fn dimensions(&self) -> (usize, usize) {
        (self.width as usize, self.height as usize)
    }

    fn strides(&self) -> (usize, usize, usize) {
        (self.width as usize, self.width as usize / 2, self.width as usize / 2)
    }

    fn y(&self) -> &[u8] {
        &self.y
    }

    fn u(&self) -> &[u8] {
        &self.u
    }

    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// Wraps `openh264::encoder::Encoder`. Width/height are taken from each
/// frame passed to `encode` (the encoder re-initializes internally if they
/// change between calls, per `openh264`'s own behavior) -- only the target
/// bitrate is fixed at construction.
pub struct H264Encoder {
    inner: OpenH264Encoder,
}

impl H264Encoder {
    pub fn new(target_bitrate_bps: u32) -> anyhow::Result<Self> {
        let config = EncoderConfig::new().bitrate(BitRate::from_bps(target_bitrate_bps));
        let inner = OpenH264Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| anyhow::anyhow!("Creating H.264 encoder: {e}"))?;
        Ok(Self { inner })
    }

    /// Encode one frame, returning the Annex-B (start-code-delimited) NAL
    /// bitstream -- may contain multiple NAL units (e.g. SPS+PPS+slice on
    /// a keyframe). Ready for `video_rtp::fragment_nal_units`.
    pub fn encode(&mut self, frame: &Yuv420Frame) -> anyhow::Result<Vec<u8>> {
        let bitstream = self.inner.encode(frame).map_err(|e| anyhow::anyhow!("H.264 encode: {e}"))?;
        Ok(bitstream.to_vec())
    }
}

/// Wraps `openh264::decoder::Decoder`.
pub struct H264Decoder {
    inner: OpenH264Decoder,
}

impl H264Decoder {
    pub fn new() -> anyhow::Result<Self> {
        let inner = OpenH264Decoder::new().map_err(|e| anyhow::anyhow!("Creating H.264 decoder: {e}"))?;
        Ok(Self { inner })
    }

    /// Feed one Annex-B NAL chunk (typically what `video_rtp::reassemble_nal_units`
    /// produces for one frame) to the decoder. Returns `None` if not enough
    /// data has accumulated yet to emit a frame (e.g. still waiting on a
    /// keyframe/SPS/PPS) -- matches `openh264`'s own `Option`-returning decode.
    pub fn decode(&mut self, nal_data: &[u8]) -> anyhow::Result<Option<Yuv420Frame>> {
        let decoded = self.inner.decode(nal_data).map_err(|e| anyhow::anyhow!("H.264 decode: {e}"))?;
        Ok(decoded.map(|d| {
            let (w, h) = d.dimensions();
            Yuv420Frame { width: w as u32, height: h as u32, y: d.y().to_vec(), u: d.u().to_vec(), v: d.v().to_vec() }
        }))
    }
}

#[cfg(test)]
#[path = "../tests/unit/video_codec.rs"]
mod tests;
