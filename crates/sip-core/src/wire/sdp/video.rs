//! A separate, parallel path through the whole call-setup pipeline for the
//! `m=video` media section -- not folded into the audio types in
//! `codec.rs`/`build.rs`/`parse.rs`. Full picture (why it's kept separate,
//! how the call/lifecycle/* call sites use these):
//! docs/crates/sip-core.md's "Video negotiation" section.

use std::net::SocketAddr;

use super::build::{IceAttrs, crypto_lines, ice_lines, savp_profile};
use super::srtp::SrtpParams;

/// Negotiated video codec. H.264 only for now (via the `openh264` crate,
/// self-compiled from Cisco's BSD-2-licensed source -- a deliberate choice
/// over VP8, whose only Rust binding is stale and needs a system `libvpx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
}

/// Dynamic PT for H.264 -- picked clear of every other PT already in use in
/// this module (0/3/8/9/13/18 static, 98/101/111 dynamic).
pub const H264_PAYLOAD_TYPE: u8 = 100;

impl VideoCodec {
    pub fn payload_type(self) -> u8 {
        match self {
            VideoCodec::H264 => H264_PAYLOAD_TYPE,
        }
    }

    /// `a=rtpmap` name/clock string -- RFC 6184 mandates a 90kHz clock for H.264.
    fn rtpmap(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H264/90000",
        }
    }

    /// `a=fmtp` line -- baseline profile (`42e01f`), level 3.1, single-NAL
    /// packetization mode. Conservative/broadly-compatible defaults, chosen
    /// for interop over squeezing out `H264Encoder`'s full capability.
    fn fmtp(self) -> Option<String> {
        match self {
            VideoCodec::H264 => Some(format!(
                "a=fmtp:{} profile-level-id=42e01f;packetization-mode=1;level-asymmetry-allowed=1\r\n",
                self.payload_type()
            )),
        }
    }
}

fn video_rtpmap_and_fmtp_lines(codec: VideoCodec) -> String {
    let pt = codec.payload_type();
    let mut out = format!("a=rtpmap:{pt} {}\r\n", codec.rtpmap());
    if let Some(fmtp) = codec.fmtp() {
        out.push_str(&fmtp);
    }
    out
}

/// Parsed `m=video` section -- deliberately mirrors `ParsedSdp`'s field
/// shape (minus `dtmf_type`/`cn_type`, which don't apply to video) so a
/// future unified multi-media parse result is a mechanical fold rather than
/// a redesign.
#[derive(Debug, Clone)]
pub struct ParsedVideoMedia {
    pub rtp_addr: SocketAddr,
    pub codec: VideoCodec,
    pub payload_type: u8,
    pub is_sendonly: bool,
    pub srtp: Option<SrtpParams>,
    pub ice_ufrag: Option<String>,
    pub ice_pwd: Option<String>,
    pub ice_candidates: Vec<String>,
}

/// Build just a `m=video ...` block, its own `c=` line, and its own `a=`
/// lines (rtpmap/fmtp/crypto/ICE/sendrecv) -- not a whole standalone SDP.
/// Includes its own media-level `c=` line (RFC 4566 §5.7) rather than
/// relying on the session-level one from `build_offer`/`build_answer` --
/// `split_media_sections` deliberately doesn't leak lines from before the
/// first `m=` line into any section's group (see its own doc comment), so a
/// parser working from an isolated video section needs this line present
/// locally. Reuses `crypto_lines`/`ice_lines` verbatim so this concatenates
/// cleanly onto `build_offer`'s/`build_answer`'s existing output -- see
/// `call/lifecycle/outgoing.rs::prepare_video_offer`/
/// `incoming.rs::prepare_video_answer` for where that concatenation happens.
pub fn build_video_media_section(
    local_ip: &str, rtp_port: u16, codec: VideoCodec, srtp: Option<&SrtpParams>, ice: Option<&IceAttrs>,
) -> String {
    let profile = savp_profile(srtp);
    let pt = codec.payload_type();
    format!(
        "m=video {rtp_port} {profile} {pt}\r\n\
         c=IN IP4 {local_ip}\r\n\
         {codec_lines}\
         {crypto}\
         {ice_lines}\
         a=sendrecv\r\n",
        codec_lines = video_rtpmap_and_fmtp_lines(codec),
        crypto = crypto_lines(srtp),
        ice_lines = ice_lines(ice),
    )
}

/// Split a raw SDP into `(m= line, following attribute lines up to the next
/// `m=` line or EOF)` groups -- a pure line-classifier, one group per media
/// section, that doesn't interpret any line's meaning itself. Lines before
/// the first `m=` line (the session-level `v=`/`o=`/`s=`/`c=`/`t=` header)
/// are not included in any group. Used by the video call sites in
/// `call/lifecycle/{incoming,response}.rs` to isolate the video `m=`
/// section's own attribute lines before parsing them with
/// `parse_video_section` -- `parse_sdp_forcing` itself stays audio-only and
/// section-unaware; see docs/crates/sip-core.md's "Video negotiation" section for
/// why the two parses are kept deliberately separate.
pub fn split_media_sections(sdp: &str) -> Vec<(&str, Vec<&str>)> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in sdp.lines() {
        let line = line.trim();
        if line.starts_with("m=") {
            sections.push((line, Vec::new()));
        } else if let Some((_, attrs)) = sections.last_mut() {
            attrs.push(line);
        }
        // Lines before the first `m=` line (session-level header) are dropped.
    }
    sections
}

/// Parse one already-isolated video section (as produced by
/// `split_media_sections`) into a `ParsedVideoMedia`. `m_line` is the raw
/// `"m=video <port> <profile> <pt...>"` line; `attr_lines` are that
/// section's own following lines. Same first-match-wins-against-`allowed`
/// codec resolution as `parse_sdp_forcing`, scoped to a PT list already
/// known to belong to this section.
pub fn parse_video_section(m_line: &str, attr_lines: &[&str], allowed: &[VideoCodec]) -> Option<ParsedVideoMedia> {
    let rest = m_line.strip_prefix("m=video ")?;
    let mut parts = rest.split_whitespace();
    let rtp_port: u16 = parts.next()?.parse().ok()?;
    parts.next(); // skip "RTP/AVP" or "RTP/SAVP"
    let pt_list: Vec<u8> = parts.filter_map(|p| p.parse().ok()).collect();

    let mut connection_ip: Option<String> = None;
    let mut rtpmaps: Vec<(u8, String)> = Vec::new();
    let mut is_sendonly = false;
    let mut srtp: Option<SrtpParams> = None;
    let mut ice_ufrag: Option<String> = None;
    let mut ice_pwd: Option<String> = None;
    let mut ice_candidates: Vec<String> = Vec::new();

    for line in attr_lines {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("c=IN IP4 ") {
            connection_ip = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=ice-ufrag:") {
            ice_ufrag = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=ice-pwd:") {
            ice_pwd = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=candidate:") {
            ice_candidates.push(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=rtpmap:") {
            let mut parts = val.splitn(2, ' ');
            if let (Some(pt_str), Some(name)) = (parts.next(), parts.next())
                && let Ok(pt) = pt_str.parse::<u8>()
            {
                rtpmaps.push((pt, name.to_ascii_lowercase()));
            }
        } else if line == "a=sendonly" {
            is_sendonly = true;
        } else if line.starts_with("a=crypto:") && srtp.is_none() {
            srtp = SrtpParams::parse_crypto_line(line);
        }
    }

    let ip = connection_ip?;
    let rtp_addr: SocketAddr = format!("{ip}:{rtp_port}").parse().ok()?;

    let resolve = |pt: u8| -> Option<VideoCodec> {
        let (_, name) = rtpmaps.iter().find(|(p, _)| *p == pt)?;
        if name.starts_with("h264") { Some(VideoCodec::H264) } else { None }
    };
    let (codec, payload_type) =
        pt_list.iter().find_map(|&pt| resolve(pt).filter(|c| allowed.contains(c)).map(|c| (c, pt)))?;

    Some(ParsedVideoMedia { rtp_addr, codec, payload_type, is_sendonly, srtp, ice_ufrag, ice_pwd, ice_candidates })
}
