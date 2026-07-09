use std::net::SocketAddr;

// ── Codec identity ────────────────────────────────────────────────────────────

pub const OPUS_PAYLOAD_TYPE: u8 = 111;
/// Dynamic PT for iLBC (RFC 3952 has no static assignment) — picked clear
/// of every other PT already in use here (0/3/8/9 static, 101/111 dynamic).
pub const ILBC_PAYLOAD_TYPE: u8 = 98;
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
}

/// Every codec this codebase knows how to negotiate, in the historical
/// default preference order — used as the fallback when an account's
/// configured codec list is empty (shouldn't normally happen; the Settings
/// UI itself refuses to let the last enabled codec be disabled).
pub const ALL_CODECS: [AudioCodec; 7] = [
    AudioCodec::Opus,
    AudioCodec::G722,
    AudioCodec::Pcmu,
    AudioCodec::Pcma,
    AudioCodec::Gsm,
    AudioCodec::Ilbc,
    AudioCodec::G729,
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
        }
    }

    /// `a=rtpmap` name/clock string, e.g. "PCMU/8000" or "opus/48000/2".
    /// G722's clock is spec-mandated as 8000 (RFC 3551) despite the codec
    /// operating at 16kHz internally -- a well-known historical quirk, not
    /// a mistake.
    fn rtpmap(self) -> &'static str {
        match self {
            AudioCodec::Pcmu => "PCMU/8000",
            AudioCodec::Pcma => "PCMA/8000",
            AudioCodec::Opus => "opus/48000/2",
            AudioCodec::G722 => "G722/8000",
            AudioCodec::Gsm => "GSM/8000",
            AudioCodec::Ilbc => "iLBC/8000",
            AudioCodec::G729 => "G729/8000",
        }
    }

    /// Extra `a=fmtp` line for this codec's payload type, if any.
    fn fmtp(self) -> Option<String> {
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

fn rtpmap_and_fmtp_lines(codec: AudioCodec) -> String {
    let pt = codec.payload_type();
    let mut out = format!("a=rtpmap:{pt} {}\r\n", codec.rtpmap());
    if let Some(fmtp) = codec.fmtp() {
        out.push_str(&fmtp);
    }
    out
}

// ── SRTP (SDES) ──────────────────────────────────────────────────────────────

pub const SRTP_MASTER_KEY_LEN: usize = 16;
pub const SRTP_MASTER_SALT_LEN: usize = 14;
const SRTP_SUITE: &str = "AES_CM_128_HMAC_SHA1_80";

/// SDES-SRTP master key + salt (RFC 4568), carried in `a=crypto:` SDP lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtpParams {
    pub key: [u8; SRTP_MASTER_KEY_LEN],
    pub salt: [u8; SRTP_MASTER_SALT_LEN],
}

impl SrtpParams {
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = [0u8; SRTP_MASTER_KEY_LEN];
        let mut salt = [0u8; SRTP_MASTER_SALT_LEN];
        let mut rng = rand::rngs::OsRng;
        rng.fill_bytes(&mut key);
        rng.fill_bytes(&mut salt);
        Self { key, salt }
    }

    fn to_crypto_line(&self, tag: u32) -> String {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let mut combined = Vec::with_capacity(SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN);
        combined.extend_from_slice(&self.key);
        combined.extend_from_slice(&self.salt);
        let inline = STANDARD.encode(combined);
        format!("a=crypto:{tag} {SRTP_SUITE} inline:{inline}\r\n")
    }

    fn parse_crypto_line(line: &str) -> Option<Self> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        // "a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:<base64>[|2^20|1:4]"
        let rest = line.trim().strip_prefix("a=crypto:")?;
        let mut parts = rest.split_whitespace();
        parts.next()?; // tag
        let suite = parts.next()?;
        if suite != SRTP_SUITE {
            return None;
        }
        let key_param = parts.next()?;
        let b64 = key_param.strip_prefix("inline:")?.split('|').next()?;
        let raw = STANDARD.decode(b64).ok()?;
        if raw.len() != SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN {
            return None;
        }
        let mut key = [0u8; SRTP_MASTER_KEY_LEN];
        let mut salt = [0u8; SRTP_MASTER_SALT_LEN];
        key.copy_from_slice(&raw[..SRTP_MASTER_KEY_LEN]);
        salt.copy_from_slice(&raw[SRTP_MASTER_KEY_LEN..]);
        Some(Self { key, salt })
    }
}

/// Both sides' SRTP keys for one call. Per RFC 4568, each side's a=crypto line
/// declares the key IT uses to encrypt what it sends: encrypt outgoing traffic
/// with `local`'s own key, decrypt incoming traffic with `remote`'s key.
#[derive(Debug, Clone)]
pub struct SrtpSession {
    pub local: SrtpParams,
    pub remote: SrtpParams,
}

// ── SDP offer/answer builders ─────────────────────────────────────────────────

fn savp_profile(srtp: Option<&SrtpParams>) -> &'static str {
    if srtp.is_some() {
        "RTP/SAVP"
    } else {
        "RTP/AVP"
    }
}

fn crypto_lines(srtp: Option<&SrtpParams>) -> String {
    srtp.map(|s| s.to_crypto_line(1)).unwrap_or_default()
}

// ── ICE (RFC 8445) ───────────────────────────────────────────────────────────

/// ICE parameters for one side of a call, gathered/generated by
/// `deelip_nat::ice` and embedded in an offer/answer. Kept as a plain struct
/// here (no dependency on `deelip-nat`) — same "protocol-layer crates stay
/// decoupled from the app-level glue" reasoning already used for
/// `AudioCodec`/`SipAccount::codec_order`. `candidates` are already
/// fully-formed RFC 8839 values (from `Candidate::marshal()`), just missing
/// the `a=candidate:` line prefix.
pub struct IceAttrs {
    pub ufrag: String,
    pub pwd: String,
    pub candidates: Vec<String>,
}

fn ice_lines(ice: Option<&IceAttrs>) -> String {
    let Some(ice) = ice else { return String::new() };
    let mut out = format!("a=ice-ufrag:{}\r\na=ice-pwd:{}\r\n", ice.ufrag, ice.pwd);
    for c in &ice.candidates {
        out.push_str(&format!("a=candidate:{c}\r\n"));
    }
    out
}

/// Build an SDP offer listing `codecs` in the given preference order (falls
/// back to `ALL_CODECS` if empty — defensive only, see `ALL_CODECS`'s doc).
/// `ice`, if given, adds `a=ice-ufrag`/`a=ice-pwd`/`a=candidate` lines
/// alongside the plain `c=`/`m=` address (which stays populated with our
/// best candidate regardless, so a peer that ignores ICE still works).
pub fn build_offer(
    local_ip: &str,
    rtp_port: u16,
    srtp: Option<&SrtpParams>,
    codecs: &[AudioCodec],
    ice: Option<&IceAttrs>,
    vad_enabled: bool,
) -> String {
    let sid = now_ntp();
    let codecs: &[AudioCodec] = if codecs.is_empty() {
        &ALL_CODECS
    } else {
        codecs
    };
    // Only offered if at least one candidate codec shares CN's 8000 Hz
    // clock -- see `CN_PAYLOAD_TYPE`'s doc comment. If the answerer ends up
    // choosing Opus anyway, `MediaEngine` separately re-checks the actually
    // negotiated codec before ever using this.
    let advertise_cn = vad_enabled && codecs.iter().any(|&c| c != AudioCodec::Opus);
    let pt_list: String = codecs
        .iter()
        .map(|c| c.payload_type().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let codec_lines: String = codecs.iter().map(|&c| rtpmap_and_fmtp_lines(c)).collect();
    let profile = savp_profile(srtp);
    let cn_pt_suffix = if advertise_cn { format!(" {CN_PAYLOAD_TYPE}") } else { String::new() };
    let cn_line = if advertise_cn {
        format!("a=rtpmap:{CN_PAYLOAD_TYPE} CN/8000\r\n")
    } else {
        String::new()
    };
    format!(
        "v=0\r\n\
         o=- {sid} {sid} IN IP4 {local_ip}\r\n\
         s=-\r\n\
         c=IN IP4 {local_ip}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} {profile} {pt_list} 101{cn_pt_suffix}\r\n\
         {codec_lines}\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=fmtp:101 0-15\r\n\
         {cn_line}\
         {crypto}\
         {ice_lines}\
         a=ptime:20\r\n\
         a=sendrecv\r\n",
        crypto = crypto_lines(srtp),
        ice_lines = ice_lines(ice),
    )
}

/// Build an SDP answer, selecting the negotiated voice `codec`.
/// telephone-event is included if the offer contained it.
pub fn build_answer(
    local_ip: &str,
    rtp_port: u16,
    codec: AudioCodec,
    srtp: Option<&SrtpParams>,
    ice: Option<&IceAttrs>,
    vad_enabled: bool,
) -> String {
    let sid = now_ntp();
    let pt = codec.payload_type();
    let profile = savp_profile(srtp);
    // See `CN_PAYLOAD_TYPE`'s doc comment for why this excludes Opus.
    let advertise_cn = vad_enabled && codec != AudioCodec::Opus;
    let pt_suffix = if advertise_cn { format!(" {CN_PAYLOAD_TYPE}") } else { String::new() };
    let cn_line = if advertise_cn {
        format!("a=rtpmap:{CN_PAYLOAD_TYPE} CN/8000\r\n")
    } else {
        String::new()
    };
    format!(
        "v=0\r\n\
         o=- {sid} {sid} IN IP4 {local_ip}\r\n\
         s=-\r\n\
         c=IN IP4 {local_ip}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} {profile} {pt} 101{pt_suffix}\r\n\
         {codec_lines}\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=fmtp:101 0-15\r\n\
         {cn_line}\
         {crypto}\
         {ice_lines}\
         a=ptime:20\r\n\
         a=sendrecv\r\n",
        codec_lines = rtpmap_and_fmtp_lines(codec),
        crypto = crypto_lines(srtp),
        ice_lines = ice_lines(ice),
    )
}

/// Build a hold SDP (a=sendonly) for re-INVITE.
pub fn build_hold_offer(
    local_ip: &str,
    rtp_port: u16,
    codec: AudioCodec,
    srtp: Option<&SrtpParams>,
) -> String {
    let sid = now_ntp();
    let pt = codec.payload_type();
    let profile = savp_profile(srtp);
    format!(
        "v=0\r\n\
         o=- {sid} {sid} IN IP4 {local_ip}\r\n\
         s=-\r\n\
         c=IN IP4 {local_ip}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} {profile} {pt} 101\r\n\
         {codec_lines}\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=fmtp:101 0-15\r\n\
         {crypto}\
         a=ptime:20\r\n\
         a=sendonly\r\n",
        codec_lines = rtpmap_and_fmtp_lines(codec),
        crypto = crypto_lines(srtp),
    )
}

/// Build a resume SDP (a=sendrecv) for re-INVITE.
pub fn build_resume_offer(
    local_ip: &str,
    rtp_port: u16,
    codec: AudioCodec,
    srtp: Option<&SrtpParams>,
) -> String {
    let sid = now_ntp();
    let pt = codec.payload_type();
    let profile = savp_profile(srtp);
    format!(
        "v=0\r\n\
         o=- {sid} {sid} IN IP4 {local_ip}\r\n\
         s=-\r\n\
         c=IN IP4 {local_ip}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} {profile} {pt} 101\r\n\
         {codec_lines}\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=fmtp:101 0-15\r\n\
         {crypto}\
         a=ptime:20\r\n\
         a=sendrecv\r\n",
        codec_lines = rtpmap_and_fmtp_lines(codec),
        crypto = crypto_lines(srtp),
    )
}

// ── SDP parser ────────────────────────────────────────────────────────────────

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
pub fn parse_sdp_forcing(
    sdp: &str,
    allowed: &[AudioCodec],
    force: Option<AudioCodec>,
) -> Option<ParsedSdp> {
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
            if let (Some(pt_str), Some(name)) = (parts.next(), parts.next()) {
                if let Ok(pt) = pt_str.parse::<u8>() {
                    let lname = name.to_ascii_lowercase();
                    if lname.starts_with("telephone-event") {
                        dtmf_type = Some(pt);
                    } else if lname.starts_with("cn") {
                        cn_type = Some(pt);
                    } else {
                        rtpmaps.push((pt, lname));
                    }
                }
            }
        } else if line == "a=sendonly" {
            is_sendonly = true;
        } else if line.starts_with("a=crypto:") && srtp.is_none() {
            srtp = SrtpParams::parse_crypto_line(line);
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
            pt_list.iter().find_map(|&pt| {
                resolve(pt)
                    .filter(|&c| c == forced && allowed.contains(&c))
                    .map(|c| (c, pt))
            })
        })
        .or_else(|| {
            pt_list.iter().find_map(|&pt| {
                resolve(pt).filter(|c| allowed.contains(c)).map(|c| (c, pt))
            })
        })?;

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
    })
}

// ── Video (Phase 1: additive primitives only) ───────────────────────────────
//
// These build/parse a `m=video` media section in isolation -- they are not
// yet wired into `build_offer`/`build_answer`/`ParsedSdp`/`parse_sdp_forcing`
// or anywhere in the live call path (`media_setup.rs`, `call/lifecycle.rs`,
// `media-engine`'s `engine.rs`). That integration, plus camera capture and
// H.264 encode/decode, is future work. This section exists so it can be
// developed and tested in isolation with zero risk of regressing the
// audio-only call path every existing call still uses exclusively.

/// Negotiated video codec. H.264 only for now (via the `openh264` crate,
/// self-compiled from Cisco's BSD-2-licensed source -- a deliberate choice
/// over VP8, whose only Rust binding is stale and needs a system `libvpx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
}

/// Dynamic PT for H.264 -- picked clear of every other PT already in use in
/// this file (0/3/8/9/13/18 static, 98/101/111 dynamic).
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
    /// packetization mode. Conservative/broadly-compatible defaults; revisit
    /// once real encoder output parameters exist (Phase 2).
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
/// cleanly onto `build_offer`'s/`build_answer`'s existing output once
/// Phase 2 wires video in.
pub fn build_video_media_section(
    local_ip: &str,
    rtp_port: u16,
    codec: VideoCodec,
    srtp: Option<&SrtpParams>,
    ice: Option<&IceAttrs>,
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
/// are not included in any group.
///
/// This is the direct fix for the bug class a naive second `m=video` line
/// would hit today: `parse_sdp_forcing` folds *every* `a=rtpmap`/
/// `a=candidate`/`a=crypto` line in the whole SDP into one flat accumulator
/// regardless of which `m=` section it's actually under, so appending a
/// video section as-is would silently corrupt audio parsing (e.g. a video
/// `a=rtpmap:100 H264/90000` line would land in the same accumulator as
/// audio's rtpmaps). `parse_sdp_forcing` itself isn't touched here --
/// nothing in the live call path produces a two-`m=`-line SDP yet, so the
/// latent issue is dormant until Phase 2 actually wires video negotiation
/// in, at which point `parse_sdp_forcing` needs to become section-aware
/// too (likely by using this function internally).
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
pub fn parse_video_section(
    m_line: &str,
    attr_lines: &[&str],
    allowed: &[VideoCodec],
) -> Option<ParsedVideoMedia> {
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
            if let (Some(pt_str), Some(name)) = (parts.next(), parts.next()) {
                if let Ok(pt) = pt_str.parse::<u8>() {
                    rtpmaps.push((pt, name.to_ascii_lowercase()));
                }
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
        if name.starts_with("h264") {
            Some(VideoCodec::H264)
        } else {
            None
        }
    };
    let (codec, payload_type) = pt_list
        .iter()
        .find_map(|&pt| resolve(pt).filter(|c| allowed.contains(c)).map(|c| (c, pt)))?;

    Some(ParsedVideoMedia {
        rtp_addr,
        codec,
        payload_type,
        is_sendonly,
        srtp,
        ice_ufrag,
        ice_pwd,
        ice_candidates,
    })
}

fn now_ntp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 2_208_988_800 // seconds from NTP epoch (1900) to Unix epoch (1970)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "../../tests/unit/sdp.rs"]
mod tests;
