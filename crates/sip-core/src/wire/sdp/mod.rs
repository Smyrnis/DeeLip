//! Split from a single `sdp.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior/API change -- every item re-exported below was already `pub`
//! in the original file, at the same `sdp::` path.

mod build;
mod codec;
mod dtls;
mod parse;
mod srtp;
mod video;

pub use build::{IceAttrs, build_answer, build_hold_offer, build_offer, build_resume_offer};
pub use codec::{ALL_CODECS, AudioCodec, CN_PAYLOAD_TYPE, ILBC_PAYLOAD_TYPE, OPUS_PAYLOAD_TYPE};
pub use dtls::{DtlsFingerprint, Setup, generate_dtls_cert};
pub use parse::{ParsedSdp, parse_sdp, parse_sdp_forcing};
pub use srtp::{SRTP_MASTER_KEY_LEN, SRTP_MASTER_SALT_LEN, SrtpParams, SrtpSession};
pub use video::{
    H264_PAYLOAD_TYPE, ParsedVideoMedia, VideoCodec, build_video_media_section, parse_video_section,
    split_media_sections,
};

#[cfg(test)]
#[path = "../../../tests/unit/sdp.rs"]
mod tests;
