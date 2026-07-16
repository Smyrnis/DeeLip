//! The audio SDP parser (`parse_sdp`/`parse_sdp_forcing`) and its result
//! type, `ParsedSdp`.

use std::net::SocketAddr;

use super::codec::AudioCodec;
use super::dtls::{DtlsFingerprint, Setup};
use super::srtp::SrtpParams;

#[derive(Debug, Clone)]
pub struct ParsedSdp {
    /// Remote RTP endpoint from `c=` + `m=audio` port.
    pub rtp_addr: SocketAddr,
    /// Negotiated voice codec.
    pub codec: AudioCodec,
    /// Negotiated voice codec's RTP payload type on the wire.
    pub payload_type: u8,
    /// DTMF telephone-event PT if present (commonly 101).
    pub dtmf_type: Option<u8>,
    /// Comfort-noise (RFC 3389) PT the remote signaled, if any (commonly
    /// `CN_PAYLOAD_TYPE`/13) -- our own CN sends to them use this PT, same
    /// idiom as `dtmf_type` for RFC 2833 telephone-event.
    pub cn_type: Option<u8>,
    /// True if remote set a=sendonly (they are holding us).
    pub is_sendonly: bool,
    /// Remote's offered/answered SRTP key, if the SDP included a supported `a=crypto:` line.
    pub srtp: Option<SrtpParams>,
    /// Remote's ICE username fragment, if this SDP signaled ICE support at all.
    pub ice_ufrag: Option<String>,
    /// Remote's ICE password, if this SDP signaled ICE support at all.
    pub ice_pwd: Option<String>,
    /// Remote's ICE candidates (raw values, without the `a=candidate:` prefix).
    pub ice_candidates: Vec<String>,
    /// Remote's DTLS certificate fingerprint, if this SDP signaled
    /// DTLS-SRTP (`a=fingerprint`) at all.
    pub fingerprint: Option<DtlsFingerprint>,
    /// Remote's DTLS role (`a=setup`), if this SDP signaled DTLS-SRTP.
    pub setup: Option<Setup>,
}

/// Parse an SDP offer/answer, picking the first payload type in the `m=`
/// line's preference order that both we recognize AND is in `allowed` (a
/// disabled codec is treated as unrecognized — it's skipped just like a
/// codec this codebase never implemented). Equivalent to
/// `parse_sdp_forcing(sdp, allowed, None)`.
pub fn parse_sdp(sdp: &str, allowed: &[AudioCodec]) -> Option<ParsedSdp> {
    parse_sdp_forcing(sdp, allowed, None)
}

/// Same as `parse_sdp`, but if `force` is `Some` and the offer's payload-type
/// list contains it at all (and it's in `allowed`), that codec wins
/// regardless of the offer's own preference order -- "Force Codec for
/// Incoming" (`SipAccount::force_incoming_codec`). Falls back to ordinary
/// preference-order selection if `force` is `None`, isn't in `allowed`, or
/// the remote simply didn't offer it.
pub fn parse_sdp_forcing(sdp: &str, allowed: &[AudioCodec], force: Option<AudioCodec>) -> Option<ParsedSdp> {
    let mut connection_ip: Option<String> = None;
    let mut rtp_port: Option<u16> = None;
    let mut pt_list: Vec<u8> = Vec::new();
    let mut rtpmaps: Vec<(u8, String)> = Vec::new();
    let mut dtmf_type: Option<u8> = None;
    let mut cn_type: Option<u8> = None;
    let mut is_sendonly = false;
    let mut srtp: Option<SrtpParams> = None;
    let mut ice_ufrag: Option<String> = None;
    let mut ice_pwd: Option<String> = None;
    let mut ice_candidates: Vec<String> = Vec::new();
    let mut fingerprint: Option<DtlsFingerprint> = None;
    let mut setup: Option<Setup> = None;

    for line in sdp.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("c=IN IP4 ") {
            connection_ip = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=ice-ufrag:") {
            ice_ufrag = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=ice-pwd:") {
            ice_pwd = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("a=candidate:") {
            ice_candidates.push(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("m=audio ") {
            let mut parts = val.split_whitespace();
            if let Some(port_str) = parts.next() {
                rtp_port = port_str.parse().ok();
            }
            parts.next(); // skip "RTP/AVP" or "RTP/SAVP"
            for pt_str in parts {
                if let Ok(pt) = pt_str.parse::<u8>() {
                    pt_list.push(pt);
                }
            }
        } else if let Some(val) = line.strip_prefix("a=rtpmap:") {
            let mut parts = val.splitn(2, ' ');
            if let (Some(pt_str), Some(name)) = (parts.next(), parts.next())
                && let Ok(pt) = pt_str.parse::<u8>()
            {
                let lname = name.to_ascii_lowercase();
                if lname.starts_with("telephone-event") {
                    dtmf_type = Some(pt);
                } else if lname.starts_with("cn") {
                    cn_type = Some(pt);
                } else {
                    rtpmaps.push((pt, lname));
                }
            }
        } else if line == "a=sendonly" {
            is_sendonly = true;
        } else if line.starts_with("a=crypto:") && srtp.is_none() {
            srtp = SrtpParams::parse_crypto_line(line);
        } else if line.starts_with("a=fingerprint:") && fingerprint.is_none() {
            fingerprint = DtlsFingerprint::parse_line(line);
        } else if let Some(val) = line.strip_prefix("a=setup:")
            && setup.is_none()
        {
            setup = Setup::parse(val);
        }
    }

    let ip = connection_ip?;
    let port = rtp_port?;
    let rtp_addr: SocketAddr = format!("{ip}:{port}").parse().ok()?;

    // Resolve one payload type to the codec it names, either from an
    // explicit rtpmap or (for 0/8) the static RTP/AVP defaults when no
    // rtpmap overrides them.
    let resolve = |pt: u8| -> Option<AudioCodec> {
        if let Some((_, name)) = rtpmaps.iter().find(|(p, _)| *p == pt) {
            if name.starts_with("opus") {
                Some(AudioCodec::Opus)
            } else if name.starts_with("pcmu") {
                Some(AudioCodec::Pcmu)
            } else if name.starts_with("pcma") {
                Some(AudioCodec::Pcma)
            } else if name.starts_with("g722") {
                Some(AudioCodec::G722)
            } else if name.starts_with("gsm") {
                Some(AudioCodec::Gsm)
            } else if name.starts_with("ilbc") {
                Some(AudioCodec::Ilbc)
            } else if name.starts_with("g729") {
                Some(AudioCodec::G729)
            } else if name.starts_with("l16") {
                Some(AudioCodec::L16)
            } else {
                None
            }
        } else {
            match pt {
                0 => Some(AudioCodec::Pcmu),
                3 => Some(AudioCodec::Gsm),
                8 => Some(AudioCodec::Pcma),
                9 => Some(AudioCodec::G722),
                18 => Some(AudioCodec::G729),
                _ => None,
            }
        }
    };

    // Forced codec wins if the offer contains it at all, regardless of its
    // own preference order; otherwise (or with no force) fall back to the
    // first payload type in the m= line's order that we recognize AND allow.
    let (codec, payload_type) = force
        .and_then(|forced| {
            pt_list.iter().find_map(|&pt| resolve(pt).filter(|&c| c == forced && allowed.contains(&c)).map(|c| (c, pt)))
        })
        .or_else(|| pt_list.iter().find_map(|&pt| resolve(pt).filter(|c| allowed.contains(c)).map(|c| (c, pt))))?;

    Some(ParsedSdp {
        rtp_addr,
        codec,
        payload_type,
        dtmf_type,
        cn_type,
        is_sendonly,
        srtp,
        ice_ufrag,
        ice_pwd,
        ice_candidates,
        fingerprint,
        setup,
    })
}
