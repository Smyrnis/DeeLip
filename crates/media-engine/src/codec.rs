/// G.711 μ-law (PCMU, payload type 0) and A-law (PCMA, payload type 8).
/// Both operate at 8000 Hz, 1 byte per sample.
/// Reference: ITU-T G.711, Sun Microsystems / FreeSWITCH implementation.

// ── PCMU (μ-law) ──────────────────────────────────────────────────────────────

const ULAW_BIAS: i32 = 0x84;
const ULAW_CLIP: i32 = 32_635;

// Maps (pcm + bias) >> 7 (0-255) to exponent (0-7)
static ULAW_EXP: [i32; 256] = [
    0,0,1,1,2,2,2,2,3,3,3,3,3,3,3,3,
    4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,
    5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
    5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
];

pub fn pcm_to_ulaw(pcm: i16) -> u8 {
    let mut s = pcm as i32;
    let sign = if s < 0 { s = -s; 0x80u8 } else { 0u8 };
    if s > ULAW_CLIP { s = ULAW_CLIP; }
    s += ULAW_BIAS;
    let exp = ULAW_EXP[((s >> 7) & 0xFF) as usize];
    let mant = ((s >> (exp + 3)) & 0x0F) as u8;
    !(sign | ((exp as u8) << 4) | mant)
}

pub fn ulaw_to_pcm(ulaw: u8) -> i16 {
    let u = !ulaw;
    let sign = u & 0x80;
    let exp  = ((u >> 4) & 0x07) as i32;
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
    if s > 4095 { s = 4095; }

    // Find segment and encode mantissa
    let aval: u8 = if s < 32 {
        (s >> 1) as u8                               // seg 0: step 2
    } else if s < 64 {
        0x10 | ((s - 32) >> 1) as u8                // seg 1: step 2
    } else {
        let seg = (31u32 - (s as u32).leading_zeros()) as u8 - 4; // = floor(log2(s)) - 4
        (seg << 4) | ((s >> seg as i32) & 0x0F) as u8
    };

    aval ^ mask
}

pub fn alaw_to_pcm(alaw: u8) -> i16 {
    let a    = alaw ^ 0x55;
    let mant = (a & 0x0F) as i32;
    let seg  = (a >> 4) & 0x07;

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

pub fn encode_pcmu(pcm: &[i16]) -> Vec<u8> { pcm.iter().map(|&s| pcm_to_ulaw(s)).collect() }
pub fn decode_pcmu(raw: &[u8])  -> Vec<i16> { raw.iter().map(|&b| ulaw_to_pcm(b)).collect() }
pub fn encode_pcma(pcm: &[i16]) -> Vec<u8>  { pcm.iter().map(|&s| pcm_to_alaw(s)).collect() }
pub fn decode_pcma(raw: &[u8])  -> Vec<i16> { raw.iter().map(|&b| alaw_to_pcm(b)).collect() }

// ── Opus ──────────────────────────────────────────────────────────────────────
//
// The audio pipeline captures/plays at 8 kHz mono (`audio::SAMPLE_RATE`), and the
// Opus encoder/decoder are configured to match — narrowband, no resampling needed.
// Per RFC 7587 the RTP clock rate for Opus is always signalled as 48000/2 in SDP
// regardless of the codec's internal sample rate; `rtp::RtpSender` is given a
// matching timestamp increment by the caller (see `engine.rs`).

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
            Err(e) => { tracing::error!("Opus encode failed: {e}"); Vec::new() }
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
            Err(e) => { tracing::error!("Opus decode failed: {e}"); Vec::new() }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ulaw_error_pct(original: i16, decoded: i16) -> f32 {
        let err = (original as i32 - decoded as i32).abs() as f32;
        let mag = original.unsigned_abs() as f32;
        if mag < 1.0 { err } else { err / mag * 100.0 }
    }

    #[test]
    fn ulaw_roundtrip() {
        for &sample in &[0i16, 100, 1000, 10000, -100, -1000, -10000] {
            let decoded = ulaw_to_pcm(pcm_to_ulaw(sample));
            let err_pct = ulaw_error_pct(sample, decoded);
            assert!(err_pct < 5.0, "μ-law roundtrip: sample={sample}, decoded={decoded}, err={err_pct:.1}%");
        }
        // At full scale, clipping adds error; up to 2% is within G.711 spec
        let clip_decoded = ulaw_to_pcm(pcm_to_ulaw(i16::MAX));
        assert!((i16::MAX as i32 - clip_decoded as i32).abs() < 1000);
    }

    #[test]
    fn alaw_roundtrip() {
        for &sample in &[0i16, 100, 1000, 10000, -100, -1000, -10000] {
            let decoded = alaw_to_pcm(pcm_to_alaw(sample));
            let err = (sample as i32 - decoded as i32).abs();
            let mag = sample.unsigned_abs() as i32;
            let err_pct = if mag > 0 { err * 100 / mag } else { err };
            assert!(err_pct < 10, "A-law roundtrip: sample={sample}, decoded={decoded}, err={err}");
        }
    }

    #[test]
    fn ulaw_known_values() {
        // μ-law silence (0) encodes to 0xFF
        assert_eq!(pcm_to_ulaw(0), 0xFF);
    }

    #[test]
    fn opus_roundtrip() {
        let mut encoder = OpusEncoder::new().unwrap();
        let mut decoder = OpusDecoder::new().unwrap();

        // One 20ms frame (160 samples @ 8kHz) of a synthetic tone.
        let frame: Vec<i16> = (0..crate::audio::FRAME_SAMPLES)
            .map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16)
            .collect();

        let encoded = encoder.encode(&frame);
        assert!(!encoded.is_empty(), "Opus should produce a non-empty packet");
        assert!(encoded.len() <= OPUS_MAX_PACKET);

        let decoded = decoder.decode(&encoded);
        assert_eq!(decoded.len(), crate::audio::FRAME_SAMPLES);
    }
}
