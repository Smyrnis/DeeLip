//! Per-codec encoder/decoder dispatch for `engine::MediaEngine` — split out of
//! `engine.rs` purely for file size (same precedent as `views/settings/`,
//! `views/dialer/`, `sip-core/src/call/lifecycle/`), not a behavior change.

use anyhow::Context;

use crate::codec::{
    G722Decoder, G722Encoder, G729Decoder, G729Encoder, GsmDecoder, GsmEncoder, IlbcDecoder, IlbcEncoder, OpusDecoder,
    OpusEncoder, decode_l16, decode_pcma, decode_pcmu, encode_l16, encode_pcma, encode_pcmu,
};
use deelip_sip::AudioCodec;

/// One live encoder for whichever `AudioCodec` a leg negotiated -- see
/// `docs/crates/media-engine.md` for the full enum-dispatch rationale. `leg_label`
/// is `""` for leg 1 or `" (leg2)"` for leg 2, used in construction-failure
/// messages (only Opus/iLBC can actually fail here).
pub(crate) enum AudioEncoder {
    Opus(OpusEncoder),
    // Boxed: G.722/G.729's encoder structs are far larger than the other
    // variants (lookup-table-heavy ADPCM/CELP state) -- boxing keeps every
    // `AudioEncoder` value itself small regardless of which codec is active.
    G722(Box<G722Encoder>),
    Gsm(GsmEncoder),
    Ilbc(IlbcEncoder),
    G729(Box<G729Encoder>),
    Pcma,
    Pcmu,
    L16,
}

impl AudioEncoder {
    pub(crate) fn new(codec: AudioCodec, leg_label: &str) -> anyhow::Result<Self> {
        Ok(match codec {
            AudioCodec::Opus => {
                Self::Opus(OpusEncoder::new().with_context(|| format!("Creating Opus encoder{leg_label}"))?)
            }
            AudioCodec::G722 => Self::G722(Box::default()),
            AudioCodec::Gsm => Self::Gsm(GsmEncoder::new()),
            AudioCodec::Ilbc => {
                Self::Ilbc(IlbcEncoder::new().with_context(|| format!("Creating iLBC encoder{leg_label}"))?)
            }
            AudioCodec::G729 => Self::G729(Box::default()),
            AudioCodec::Pcma => Self::Pcma,
            AudioCodec::Pcmu => Self::Pcmu,
            AudioCodec::L16 => Self::L16,
        })
    }

    pub(crate) fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        match self {
            Self::Opus(e) => e.encode(pcm),
            Self::G722(e) => e.encode(pcm),
            Self::Gsm(e) => e.encode(pcm),
            Self::Ilbc(e) => e.encode(pcm),
            Self::G729(e) => e.encode(pcm),
            Self::Pcma => encode_pcma(pcm),
            Self::Pcmu => encode_pcmu(pcm),
            Self::L16 => encode_l16(pcm),
        }
    }
}

/// Decoder counterpart of `AudioEncoder` -- see its doc comment.
pub(crate) enum AudioDecoder {
    Opus(OpusDecoder),
    // Boxed for the same reason as `AudioEncoder::G729` -- G722's decoder
    // struct (a lookup-table-heavy ADPCM state machine) is far larger than
    // the other small variants here.
    G722(Box<G722Decoder>),
    Gsm(GsmDecoder),
    Ilbc(IlbcDecoder),
    G729(Box<G729Decoder>),
    Pcma,
    Pcmu,
    L16,
}

impl AudioDecoder {
    pub(crate) fn new(codec: AudioCodec, leg_label: &str) -> anyhow::Result<Self> {
        Ok(match codec {
            AudioCodec::Opus => {
                Self::Opus(OpusDecoder::new().with_context(|| format!("Creating Opus decoder{leg_label}"))?)
            }
            AudioCodec::G722 => Self::G722(Box::default()),
            AudioCodec::Gsm => Self::Gsm(GsmDecoder::new()),
            AudioCodec::Ilbc => {
                Self::Ilbc(IlbcDecoder::new().with_context(|| format!("Creating iLBC decoder{leg_label}"))?)
            }
            AudioCodec::G729 => Self::G729(Box::default()),
            AudioCodec::Pcma => Self::Pcma,
            AudioCodec::Pcmu => Self::Pcmu,
            AudioCodec::L16 => Self::L16,
        })
    }

    pub(crate) fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        match self {
            Self::Opus(d) => d.decode(payload),
            Self::G722(d) => d.decode(payload),
            Self::Gsm(d) => d.decode(payload),
            Self::Ilbc(d) => d.decode(payload),
            Self::G729(d) => d.decode(payload),
            Self::Pcma => decode_pcma(payload),
            Self::Pcmu => decode_pcmu(payload),
            Self::L16 => decode_l16(payload),
        }
    }
}

/// Per-packet RTP timestamp increment for a 20ms frame, in units of the
/// codec's declared RTP clock rate. G.711's clock is 8000 Hz; Opus's RTP
/// clock is always 48000 Hz regardless of the audio's actual sample rate
/// (RFC 7587), even though our pipeline encodes/decodes Opus at 8 kHz.
pub(crate) fn ts_increment_for(codec: AudioCodec) -> u32 {
    match codec {
        AudioCodec::Opus => 960,
        AudioCodec::Pcmu
        | AudioCodec::Pcma
        | AudioCodec::G722
        | AudioCodec::Gsm
        | AudioCodec::Ilbc
        | AudioCodec::G729
        | AudioCodec::L16 => 160,
    }
}

/// RTP clock rate for jitter math (RFC 7587: Opus's RTP clock is always
/// 48000 regardless of the audio's actual sample rate; everything else
/// here is 8000 — see `ts_increment_for`'s own doc comment).
pub(crate) fn clock_hz_for(codec: AudioCodec) -> f64 {
    match codec {
        AudioCodec::Opus => 48000.0,
        AudioCodec::Pcmu
        | AudioCodec::Pcma
        | AudioCodec::G722
        | AudioCodec::Gsm
        | AudioCodec::Ilbc
        | AudioCodec::G729
        | AudioCodec::L16 => 8000.0,
    }
}

#[cfg(test)]
#[path = "../tests/unit/codec_dispatch.rs"]
mod tests;
