//! Split from a single `sdp.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior/API change -- every item re-exported below was already `pub`
//! in the original file, at the same `sdp::` path.

mod build;
mod codec;
mod parse;
mod srtp;
mod video;

pub use build::{build_answer, build_hold_offer, build_offer, build_resume_offer, IceAttrs};
pub use codec::{AudioCodec, ALL_CODECS, CN_PAYLOAD_TYPE, ILBC_PAYLOAD_TYPE, OPUS_PAYLOAD_TYPE};
pub use parse::{parse_sdp, parse_sdp_forcing, ParsedSdp};
pub use srtp::{SrtpParams, SrtpSession, SRTP_MASTER_KEY_LEN, SRTP_MASTER_SALT_LEN};
pub use video::{
    build_video_media_section, parse_video_section, split_media_sections, ParsedVideoMedia, VideoCodec,
    H264_PAYLOAD_TYPE,
};

#[cfg(test)]
#[path = "../../../tests/unit/sdp.rs"]
mod tests;
