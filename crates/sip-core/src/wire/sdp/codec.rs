//! Audio codec identity: `AudioCodec`, its static/dynamic payload types, and
//! the `a=rtpmap`/`a=fmtp` line rendering `build.rs` uses.

pub const OPUS_PAYLOAD_TYPE: u8 = 111;
/// Dynamic PT for iLBC (RFC 3952 has no static assignment) — picked clear
/// of every other PT already in use here (0/3/8/9 static, 101/111 dynamic).
pub const ILBC_PAYLOAD_TYPE: u8 = 98;
/// Dynamic PT for `AudioCodec::L16` — RFC 3551 §4.5.11 does give L16 a
/// static assignment (PT 10/11), but at 44100 Hz stereo/mono, not this
/// pipeline's fixed 8kHz mono; a dynamic PT + explicit `a=rtpmap` describes
/// what we actually send, same reasoning as iLBC's dynamic PT above.
pub const L16_PAYLOAD_TYPE: u8 = 118;
/// RFC 3551 static assignment for Comfort Noise at an 8000 Hz clock (RFC
/// 3389). Only ever advertised/used alongside a codec whose own RTP clock
/// is also 8000 Hz (i.e. never Opus, which is 48000) -- CN packets share
/// the same `RtpSender` timestamp counter as the main codec's packets (see
/// `deelip_media::rtp::RtpSender::next_packet_with_pt`), so a clock
/// mismatch between them would corrupt playout timing on the far end.
pub const CN_PAYLOAD_TYPE: u8 = 13;

/// Negotiated voice codec. The numeric RTP payload type for the wire is
/// derived from this (see `AudioCodec::payload_type`); it's shared by both
/// call legs once negotiated (RFC 3264 answers echo the offer's PT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Pcmu,
    Pcma,
    Opus,
    /// Wideband (16kHz-internal) codec, interop-only in this codebase --
    /// DeeLip's own audio pipeline stays 8kHz throughout (mic/speaker/
    /// jitter buffer/AEC/mixing/recording), so a G722Encoder/Decoder
    /// (see `codec.rs`) resamples at the codec boundary rather than the
    /// whole pipeline running at 16kHz. This does NOT make DeeLip's own
    /// captured voice objectively clearer; it buys interop with phones/
    /// PBXes that prefer or require G.722 over Opus/G.711.
    G722,
    /// GSM 06.10 full-rate — legacy narrowband codec, RFC 3551 static PT 3.
    Gsm,
    /// iLBC (RFC 3951/3952), 20ms mode (304 bits/38 bytes per frame) to
    /// match DeeLip's fixed 20ms RTP framing throughout — the alternative
    /// 30ms mode is deliberately not offered.
    Ilbc,
    /// G.729 (RFC 3551 static PT 18) -- a low-bitrate (8kbps) narrowband
    /// codec, native 8kHz same as this pipeline throughout. Annex B (VAD/
    /// comfort-noise/DTX) is neither offered nor handled -- see the
    /// `annexb=no` fmtp this declares -- so the far end shouldn't send
    /// discontinuous-transmission frames we'd otherwise have no comfort-
    /// noise generator to decode.
    G729,
    /// Uncompressed 16-bit signed linear PCM, network (big-endian) byte
    /// order per RFC 3551 §4.5.11 -- no real "codec" logic, just raw
    /// samples on the wire. Mostly useful for lab/loopback testing or
    /// interop with gear that specifically wants uncompressed audio;
    /// double the bitrate of G.711 for no quality benefit at this
    /// pipeline's 8kHz sample rate, so it's not a sensible default for a
    /// real call. See `L16_PAYLOAD_TYPE`'s doc comment for why this is a
    /// dynamic PT rather than RFC 3551's static one.
    L16,
}

/// Every codec this codebase knows how to negotiate, in the historical
/// default preference order — used as the fallback when an account's
/// configured codec list is empty (shouldn't normally happen; the Settings
/// UI itself refuses to let the last enabled codec be disabled).
pub const ALL_CODECS: [AudioCodec; 8] = [
    AudioCodec::Opus,
    AudioCodec::G722,
    AudioCodec::Pcmu,
    AudioCodec::Pcma,
    AudioCodec::Gsm,
    AudioCodec::Ilbc,
    AudioCodec::G729,
    AudioCodec::L16,
];

impl AudioCodec {
    pub fn payload_type(self) -> u8 {
        match self {
            AudioCodec::Pcmu => 0,
            AudioCodec::Gsm => 3,
            AudioCodec::Pcma => 8,
            AudioCodec::G722 => 9,
            AudioCodec::G729 => 18,
            AudioCodec::Opus => OPUS_PAYLOAD_TYPE,
            AudioCodec::Ilbc => ILBC_PAYLOAD_TYPE,
            AudioCodec::L16 => L16_PAYLOAD_TYPE,
        }
    }

    /// `a=rtpmap` name/clock string, e.g. "PCMU/8000" or "opus/48000/2".
    /// G722's clock is spec-mandated as 8000 (RFC 3551) despite the codec
    /// operating at 16kHz internally -- a well-known historical quirk, not
    /// a mistake.
    pub(super) fn rtpmap(self) -> &'static str {
        match self {
            AudioCodec::Pcmu => "PCMU/8000",
            AudioCodec::Pcma => "PCMA/8000",
            AudioCodec::Opus => "opus/48000/2",
            AudioCodec::G722 => "G722/8000",
            AudioCodec::Gsm => "GSM/8000",
            AudioCodec::Ilbc => "iLBC/8000",
            AudioCodec::G729 => "G729/8000",
            AudioCodec::L16 => "L16/8000",
        }
    }

    /// Extra `a=fmtp` line for this codec's payload type, if any.
    pub(super) fn fmtp(self) -> Option<String> {
        match self {
            AudioCodec::Opus => Some(format!("a=fmtp:{} useinbandfec=1\r\n", self.payload_type())),
            // RFC 3952 §4.2 -- without this, a receiver defaults to the
            // 30ms/50-byte mode, which doesn't match what we actually send.
            AudioCodec::Ilbc => Some(format!("a=fmtp:{} mode=20\r\n", self.payload_type())),
            // Declares we neither send nor understand Annex B VAD/DTX
            // comfort-noise frames -- see `AudioCodec::G729`'s doc comment.
            AudioCodec::G729 => Some(format!("a=fmtp:{} annexb=no\r\n", self.payload_type())),
            _ => None,
        }
    }
}
