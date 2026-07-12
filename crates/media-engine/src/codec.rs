//! G.711 μ-law (PCMU, payload type 0) and A-law (PCMA, payload type 8).
//! Both operate at 8000 Hz, 1 byte per sample.
//! Reference: ITU-T G.711, Sun Microsystems / FreeSWITCH implementation.

// ── PCMU (μ-law) ──────────────────────────────────────────────────────────────

const ULAW_BIAS: i32 = 0x84;
const ULAW_CLIP: i32 = 32_635;

// Maps (pcm + bias) >> 7 (0-255) to exponent (0-7)
static ULAW_EXP: [i32; 256] = [
    0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
];

pub fn pcm_to_ulaw(pcm: i16) -> u8 {
    let mut s = pcm as i32;
    let sign = if s < 0 {
        s = -s;
        0x80u8
    } else {
        0u8
    };
    if s > ULAW_CLIP {
        s = ULAW_CLIP;
    }
    s += ULAW_BIAS;
    let exp = ULAW_EXP[((s >> 7) & 0xFF) as usize];
    let mant = ((s >> (exp + 3)) & 0x0F) as u8;
    !(sign | ((exp as u8) << 4) | mant)
}

pub fn ulaw_to_pcm(ulaw: u8) -> i16 {
    let u = !ulaw;
    let sign = u & 0x80;
    let exp = ((u >> 4) & 0x07) as i32;
    let mant = (u & 0x0F) as i32;
    let s = (((mant << 3) + ULAW_BIAS) << exp) - ULAW_BIAS;
    if sign != 0 { -(s as i16) } else { s as i16 }
}

// ── PCMA (A-law) ──────────────────────────────────────────────────────────────

pub fn pcm_to_alaw(pcm: i16) -> u8 {
    // Convert 16-bit PCM to 13-bit signed, then A-law encode.
    let mut s = pcm as i32 >> 3;

    let mask: u8 = if s >= 0 {
        0xD5 // positive: encode with sign=1 then XOR alternation
    } else {
        s = -s - 1;
        0x55
    };

    // Clip to 12-bit magnitude
    if s > 4095 {
        s = 4095;
    }

    // Find segment and encode mantissa
    let aval: u8 = if s < 32 {
        (s >> 1) as u8 // seg 0: step 2
    } else if s < 64 {
        0x10 | ((s - 32) >> 1) as u8 // seg 1: step 2
    } else {
        let seg = (31u32 - (s as u32).leading_zeros()) as u8 - 4; // = floor(log2(s)) - 4
        (seg << 4) | ((s >> seg as i32) & 0x0F) as u8
    };

    aval ^ mask
}

pub fn alaw_to_pcm(alaw: u8) -> i16 {
    let a = alaw ^ 0x55;
    let mant = (a & 0x0F) as i32;
    let seg = (a >> 4) & 0x07;

    let s = match seg {
        0 => mant * 2 + 1,
        1 => mant * 2 + 33,
        _ => ((mant + 0x10) << seg as i32) + (1 << (seg as i32 - 1)),
    };

    // Scale back to 16-bit
    let s = (s << 3) as i16;
    if a & 0x80 != 0 { s } else { -s }
}

// ── Batch helpers ─────────────────────────────────────────────────────────────

pub fn encode_pcmu(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| pcm_to_ulaw(s)).collect()
}
pub fn decode_pcmu(raw: &[u8]) -> Vec<i16> {
    raw.iter().map(|&b| ulaw_to_pcm(b)).collect()
}
pub fn encode_pcma(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| pcm_to_alaw(s)).collect()
}
pub fn decode_pcma(raw: &[u8]) -> Vec<i16> {
    raw.iter().map(|&b| alaw_to_pcm(b)).collect()
}

// ── Opus (narrowband, matching the 8kHz pipeline -- see docs/crates/media-engine.md) ────

use audiopus::coder::{Decoder as RawOpusDecoder, Encoder as RawOpusEncoder};
use audiopus::{Application, Channels, SampleRate};

/// Max size of an Opus-encoded frame at these bitrates; comfortably above worst case.
const OPUS_MAX_PACKET: usize = 400;

pub struct OpusEncoder(RawOpusEncoder);

impl OpusEncoder {
    pub fn new() -> anyhow::Result<Self> {
        let enc = RawOpusEncoder::new(SampleRate::Hz8000, Channels::Mono, Application::Voip)?;
        Ok(Self(enc))
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let mut out = [0u8; OPUS_MAX_PACKET];
        match self.0.encode(pcm, &mut out) {
            Ok(len) => out[..len].to_vec(),
            Err(e) => {
                tracing::error!("Opus encode failed: {e}");
                Vec::new()
            }
        }
    }
}

pub struct OpusDecoder(RawOpusDecoder);

impl OpusDecoder {
    pub fn new() -> anyhow::Result<Self> {
        let dec = RawOpusDecoder::new(SampleRate::Hz8000, Channels::Mono)?;
        Ok(Self(dec))
    }

    pub fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        let mut out = [0i16; crate::audio::FRAME_SAMPLES];
        match self.0.decode(Some(payload), &mut out[..], false) {
            Ok(len) => out[..len].to_vec(),
            Err(e) => {
                tracing::error!("Opus decode failed: {e}");
                Vec::new()
            }
        }
    }
}

// ── G.722 (interop-only, resampled at the boundary -- see docs/crates/media-engine.md) ──

use audio_codec::g722::{G722Decoder as RawG722Decoder, G722Encoder as RawG722Encoder};
use audio_codec::{Decoder as _, Encoder as _, Resampler};

const G722_NARROWBAND_HZ: usize = crate::audio::SAMPLE_RATE as usize;
const G722_WIDEBAND_HZ: usize = 16_000;

pub struct G722Encoder {
    codec: RawG722Encoder,
    resampler: Resampler,
}

impl Default for G722Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl G722Encoder {
    pub fn new() -> Self {
        Self { codec: RawG722Encoder::new(), resampler: Resampler::new(G722_NARROWBAND_HZ, G722_WIDEBAND_HZ) }
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let wideband = self.resampler.resample(pcm);
        self.codec.encode(&wideband)
    }
}

pub struct G722Decoder {
    codec: RawG722Decoder,
    resampler: Resampler,
}

impl Default for G722Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl G722Decoder {
    pub fn new() -> Self {
        Self { codec: RawG722Decoder::new(), resampler: Resampler::new(G722_WIDEBAND_HZ, G722_NARROWBAND_HZ) }
    }

    pub fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        let wideband = self.codec.decode(payload);
        self.resampler.resample(&wideband)
    }
}

// ── G.729 (native 8kHz, pure-Rust g729-sys -- see docs/crates/media-engine.md) ──────────

use audio_codec::g729::{G729Decoder as RawG729Decoder, G729Encoder as RawG729Encoder};

pub struct G729Encoder(RawG729Encoder);

impl Default for G729Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl G729Encoder {
    pub fn new() -> Self {
        Self(RawG729Encoder::new())
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        self.0.encode(pcm)
    }
}

pub struct G729Decoder(RawG729Decoder);

impl Default for G729Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl G729Decoder {
    pub fn new() -> Self {
        Self(RawG729Decoder::new())
    }

    pub fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        self.0.decode(payload)
    }
}

// ── GSM 06.10 (vendored reference C impl -- see docs/crates/media-engine.md) ───────────
// 160 samples (20ms @ 8kHz) <-> one 33-byte GSM full-rate frame.

pub struct GsmEncoder(gsm_sys::Gsm);

// Safety: `gsm_sys::Gsm` (`*mut GsmState`) is a raw pointer, so it isn't
// `Send` by default -- but this struct is its exclusive owner (created in
// `new()`, freed in `Drop`, never shared with or accessed from another
// thread concurrently), and libgsm's per-instance state is entirely
// self-contained (no thread-local or global state), so moving it to
// another thread (e.g. into `tokio::spawn`'s task) is sound.
unsafe impl Send for GsmEncoder {}

impl Default for GsmEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl GsmEncoder {
    pub fn new() -> Self {
        // Safety: `gsm_create` just allocates and zero-initializes the
        // codec's internal state struct; the returned pointer is non-null
        // on any real allocator (libgsm has no other failure mode here).
        Self(unsafe { gsm_sys::gsm_create() })
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let mut frame: gsm_sys::GsmFrame = [0u8; 33];
        // Safety: `gsm_encode` reads exactly 160 `GsmSignal` (i16) samples
        // from `arg2` and writes exactly 33 bytes to `arg3` -- both
        // buffers are sized to match, and `self.0` was built by
        // `gsm_create` above.
        unsafe {
            gsm_sys::gsm_encode(self.0, pcm.as_ptr() as *mut _, frame.as_mut_ptr());
        }
        frame.to_vec()
    }
}

impl Drop for GsmEncoder {
    fn drop(&mut self) {
        // Safety: `self.0` was created by `gsm_create` in `new()` and is
        // never shared or freed elsewhere.
        unsafe { gsm_sys::gsm_destroy(self.0) };
    }
}

pub struct GsmDecoder(gsm_sys::Gsm);

// Safety: same reasoning as `GsmEncoder`'s `Send` impl above.
unsafe impl Send for GsmDecoder {}

impl Default for GsmDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl GsmDecoder {
    pub fn new() -> Self {
        Self(unsafe { gsm_sys::gsm_create() })
    }

    pub fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        if payload.len() != 33 {
            tracing::error!("GSM decode: expected a 33-byte frame, got {}", payload.len());
            return Vec::new();
        }
        let mut out = [0i16; crate::audio::FRAME_SAMPLES];
        // Safety: `gsm_decode` reads exactly 33 bytes from `arg2` (checked
        // above) and writes exactly 160 `GsmSignal` samples to `arg3`,
        // which `out` is sized to hold.
        let rc = unsafe { gsm_sys::gsm_decode(self.0, payload.as_ptr() as *mut _, out.as_mut_ptr()) };
        if rc != 0 {
            tracing::error!("GSM decode failed (rc={rc})");
            return Vec::new();
        }
        out.to_vec()
    }
}

impl Drop for GsmDecoder {
    fn drop(&mut self) {
        unsafe { gsm_sys::gsm_destroy(self.0) };
    }
}

// ── iLBC (20ms mode matches our framing directly -- see docs/crates/media-engine.md) ────

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, Packet, SampleFormat, TimeBase};
use oxideav_core::{Decoder as OxDecoder, Encoder as OxEncoder};

fn ilbc_params() -> CodecParameters {
    let mut params = CodecParameters::audio(CodecId::new(oxideav_ilbc::CODEC_ID_STR));
    params.sample_rate = Some(crate::audio::SAMPLE_RATE);
    params.channels = Some(1);
    params.sample_format = Some(SampleFormat::S16);
    // Default mode is 20ms (see `oxideav_ilbc`'s own encoder factory) --
    // matches DeeLip's fixed 20ms framing, so no `frame_ms` option needed.
    params
}

fn pcm_to_audio_frame(pcm: &[i16]) -> Frame {
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    Frame::Audio(AudioFrame { samples: pcm.len() as u32, pts: Some(0), data: vec![bytes] })
}

pub struct IlbcEncoder(Box<dyn OxEncoder>);

impl IlbcEncoder {
    pub fn new() -> anyhow::Result<Self> {
        let enc = oxideav_ilbc::encoder::make_encoder(&ilbc_params())
            .map_err(|e| anyhow::anyhow!("Creating iLBC encoder: {e}"))?;
        Ok(Self(enc))
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        if let Err(e) = self.0.send_frame(&pcm_to_audio_frame(pcm)) {
            tracing::error!("iLBC encode (send_frame) failed: {e}");
            return Vec::new();
        }
        // `receive_packet` returns `Error::NeedMore` if 160 samples haven't
        // accumulated into a full frame yet -- can't happen when called
        // with exactly one 160-sample frame at a time, as `engine.rs` does,
        // but treated as "nothing to send yet" rather than a hard error.
        self.0.receive_packet().map(|pkt| pkt.data).unwrap_or_default()
    }
}

pub struct IlbcDecoder(Box<dyn OxDecoder>);

impl IlbcDecoder {
    pub fn new() -> anyhow::Result<Self> {
        let dec = oxideav_ilbc::decoder::make_decoder(&ilbc_params())
            .map_err(|e| anyhow::anyhow!("Creating iLBC decoder: {e}"))?;
        Ok(Self(dec))
    }

    pub fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        let pkt = Packet::new(0, TimeBase::new(1, crate::audio::SAMPLE_RATE as i64), payload.to_vec());
        if let Err(e) = self.0.send_packet(&pkt) {
            tracing::error!("iLBC decode (send_packet) failed: {e}");
            return Vec::new();
        }
        match self.0.receive_frame() {
            Ok(Frame::Audio(af)) => af
                .data
                .first()
                .map(|bytes| bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
                .unwrap_or_default(),
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::error!("iLBC decode (receive_frame) failed: {e}");
                Vec::new()
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "../tests/unit/codec.rs"]
mod tests;
