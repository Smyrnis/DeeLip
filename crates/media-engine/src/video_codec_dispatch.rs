//! Per-codec video encoder/decoder dispatch -- mirrors `codec_dispatch.rs`'s
//! `AudioEncoder`/`AudioDecoder` enum-dispatch pattern for video, so a
//! future second video codec (VP8) slots in the same low-friction way new
//! audio codecs already do. Only `H264` exists today -- see
//! `docs/crates/media-engine.md`.

use deelip_sip::sdp::VideoCodec;

use crate::video_codec::{H264Decoder, H264Encoder, Yuv420Frame};

pub(crate) enum VideoEncoder {
    H264(H264Encoder),
}

impl VideoEncoder {
    pub(crate) fn new(codec: VideoCodec, target_bitrate_bps: u32) -> anyhow::Result<Self> {
        Ok(match codec {
            VideoCodec::H264 => Self::H264(H264Encoder::new(target_bitrate_bps)?),
        })
    }

    pub(crate) fn encode(&mut self, frame: &Yuv420Frame) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::H264(e) => e.encode(frame),
        }
    }
}

/// Decoder counterpart of `VideoEncoder` -- see its doc comment.
pub(crate) enum VideoDecoder {
    H264(H264Decoder),
}

impl VideoDecoder {
    pub(crate) fn new(codec: VideoCodec) -> anyhow::Result<Self> {
        Ok(match codec {
            VideoCodec::H264 => Self::H264(H264Decoder::new()?),
        })
    }

    pub(crate) fn decode(&mut self, nal_data: &[u8]) -> anyhow::Result<Option<Yuv420Frame>> {
        match self {
            Self::H264(d) => d.decode(nal_data),
        }
    }
}
