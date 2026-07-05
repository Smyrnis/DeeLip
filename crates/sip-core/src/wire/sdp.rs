use std::net::SocketAddr;

// ── Codec identity ────────────────────────────────────────────────────────────

pub const OPUS_PAYLOAD_TYPE: u8 = 111;
/// Dynamic PT for iLBC (RFC 3952 has no static assignment) — picked clear
/// of every other PT already in use here (0/3/8/9 static, 101/111 dynamic).
pub const ILBC_PAYLOAD_TYPE: u8 = 98;

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
}

/// Every codec this codebase knows how to negotiate, in the historical
/// default preference order — used as the fallback when an account's
/// configured codec list is empty (shouldn't normally happen; the Settings
/// UI itself refuses to let the last enabled codec be disabled).
pub const ALL_CODECS: [AudioCodec; 6] = [
    AudioCodec::Opus,
    AudioCodec::G722,
    AudioCodec::Pcmu,
    AudioCodec::Pcma,
    AudioCodec::Gsm,
    AudioCodec::Ilbc,
];

impl AudioCodec {
    pub fn payload_type(self) -> u8 {
        match self {
            AudioCodec::Pcmu => 0,
            AudioCodec::Gsm => 3,
            AudioCodec::Pcma => 8,
            AudioCodec::G722 => 9,
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
        }
    }

    /// Extra `a=fmtp` line for this codec's payload type, if any.
    fn fmtp(self) -> Option<String> {
        match self {
            AudioCodec::Opus => Some(format!("a=fmtp:{} useinbandfec=1\r\n", self.payload_type())),
            // RFC 3952 §4.2 -- without this, a receiver defaults to the
            // 30ms/50-byte mode, which doesn't match what we actually send.
            AudioCodec::Ilbc => Some(format!("a=fmtp:{} mode=20\r\n", self.payload_type())),
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
) -> String {
    let sid = now_ntp();
    let codecs: &[AudioCodec] = if codecs.is_empty() {
        &ALL_CODECS
    } else {
        codecs
    };
    let pt_list: String = codecs
        .iter()
        .map(|c| c.payload_type().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let codec_lines: String = codecs.iter().map(|&c| rtpmap_and_fmtp_lines(c)).collect();
    let profile = savp_profile(srtp);
    format!(
        "v=0\r\n\
         o=- {sid} {sid} IN IP4 {local_ip}\r\n\
         s=-\r\n\
         c=IN IP4 {local_ip}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} {profile} {pt_list} 101\r\n\
         {codec_lines}\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=fmtp:101 0-15\r\n\
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
/// codec this codebase never implemented).
pub fn parse_sdp(sdp: &str, allowed: &[AudioCodec]) -> Option<ParsedSdp> {
    let mut connection_ip: Option<String> = None;
    let mut rtp_port: Option<u16> = None;
    let mut pt_list: Vec<u8> = Vec::new();
    let mut rtpmaps: Vec<(u8, String)> = Vec::new();
    let mut dtmf_type: Option<u8> = None;
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

    // Pick the first payload type in the m= line's preference order that we
    // recognize, either from an explicit rtpmap or (for 0/8) the static
    // RTP/AVP defaults when no rtpmap overrides them.
    let (codec, payload_type) = pt_list.iter().find_map(|&pt| {
        let recognized = if let Some((_, name)) = rtpmaps.iter().find(|(p, _)| *p == pt) {
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
            } else {
                None
            }
        } else {
            match pt {
                0 => Some(AudioCodec::Pcmu),
                3 => Some(AudioCodec::Gsm),
                8 => Some(AudioCodec::Pcma),
                9 => Some(AudioCodec::G722),
                _ => None,
            }
        };
        recognized.filter(|c| allowed.contains(c)).map(|c| (c, pt))
    })?;

    Some(ParsedSdp {
        rtp_addr,
        codec,
        payload_type,
        dtmf_type,
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
