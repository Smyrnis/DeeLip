use std::net::SocketAddr;

// ── Codec identity ────────────────────────────────────────────────────────────

pub const OPUS_PAYLOAD_TYPE: u8 = 111;

/// Negotiated voice codec. The numeric RTP payload type for the wire is
/// derived from this (see `AudioCodec::payload_type`); it's shared by both
/// call legs once negotiated (RFC 3264 answers echo the offer's PT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Pcmu,
    Pcma,
    Opus,
}

impl AudioCodec {
    pub fn payload_type(self) -> u8 {
        match self {
            AudioCodec::Pcmu => 0,
            AudioCodec::Pcma => 8,
            AudioCodec::Opus => OPUS_PAYLOAD_TYPE,
        }
    }

    /// `a=rtpmap` name/clock string, e.g. "PCMU/8000" or "opus/48000/2".
    fn rtpmap(self) -> &'static str {
        match self {
            AudioCodec::Pcmu => "PCMU/8000",
            AudioCodec::Pcma => "PCMA/8000",
            AudioCodec::Opus => "opus/48000/2",
        }
    }

    /// Extra `a=fmtp` line for this codec's payload type, if any.
    fn fmtp(self) -> Option<String> {
        match self {
            AudioCodec::Opus => Some(format!("a=fmtp:{} useinbandfec=1\r\n", self.payload_type())),
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

pub const SRTP_MASTER_KEY_LEN:  usize = 16;
pub const SRTP_MASTER_SALT_LEN: usize = 14;
const SRTP_SUITE: &str = "AES_CM_128_HMAC_SHA1_80";

/// SDES-SRTP master key + salt (RFC 4568), carried in `a=crypto:` SDP lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtpParams {
    pub key:  [u8; SRTP_MASTER_KEY_LEN],
    pub salt: [u8; SRTP_MASTER_SALT_LEN],
}

impl SrtpParams {
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key  = [0u8; SRTP_MASTER_KEY_LEN];
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
        if suite != SRTP_SUITE { return None; }
        let key_param = parts.next()?;
        let b64 = key_param.strip_prefix("inline:")?.split('|').next()?;
        let raw = STANDARD.decode(b64).ok()?;
        if raw.len() != SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN { return None; }
        let mut key  = [0u8; SRTP_MASTER_KEY_LEN];
        let mut salt = [0u8; SRTP_MASTER_SALT_LEN];
        key.copy_from_slice(&raw[..SRTP_MASTER_KEY_LEN]);
        salt.copy_from_slice(&raw[SRTP_MASTER_KEY_LEN..]);
        Some(Self { key, salt })
    }
}

/// Both sides' SRTP keys for one call. Per RFC 4568, each side's a=crypto line
/// declares the key IT uses to encrypt what it sends: encrypt outgoing traffic
/// with `local`'s own key, decrypt incoming traffic with `remote`'s key.
pub struct SrtpSession {
    pub local:  SrtpParams,
    pub remote: SrtpParams,
}

// ── SDP offer/answer builders ─────────────────────────────────────────────────

fn savp_profile(srtp: Option<&SrtpParams>) -> &'static str {
    if srtp.is_some() { "RTP/SAVP" } else { "RTP/AVP" }
}

fn crypto_lines(srtp: Option<&SrtpParams>) -> String {
    srtp.map(|s| s.to_crypto_line(1)).unwrap_or_default()
}

/// Build a full SDP offer: Opus (preferred) + PCMU + PCMA + telephone-event (DTMF).
pub fn build_offer(local_ip: &str, rtp_port: u16, srtp: Option<&SrtpParams>) -> String {
    let sid = now_ntp();
    let opus_pt = AudioCodec::Opus.payload_type();
    let profile = savp_profile(srtp);
    format!(
        "v=0\r\n\
         o=- {sid} {sid} IN IP4 {local_ip}\r\n\
         s=-\r\n\
         c=IN IP4 {local_ip}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} {profile} {opus_pt} 0 8 101\r\n\
         {opus_lines}\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=fmtp:101 0-15\r\n\
         {crypto}\
         a=ptime:20\r\n\
         a=sendrecv\r\n",
        opus_lines = rtpmap_and_fmtp_lines(AudioCodec::Opus),
        crypto = crypto_lines(srtp),
    )
}

/// Build an SDP answer, selecting the negotiated voice `codec`.
/// telephone-event is included if the offer contained it.
pub fn build_answer(local_ip: &str, rtp_port: u16, codec: AudioCodec, srtp: Option<&SrtpParams>) -> String {
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

/// Build a hold SDP (a=sendonly) for re-INVITE.
pub fn build_hold_offer(local_ip: &str, rtp_port: u16, codec: AudioCodec, srtp: Option<&SrtpParams>) -> String {
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
pub fn build_resume_offer(local_ip: &str, rtp_port: u16, codec: AudioCodec, srtp: Option<&SrtpParams>) -> String {
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
    pub rtp_addr:    SocketAddr,
    /// Negotiated voice codec.
    pub codec:        AudioCodec,
    /// Negotiated voice codec's RTP payload type on the wire.
    pub payload_type: u8,
    /// DTMF telephone-event PT if present (commonly 101).
    pub dtmf_type:   Option<u8>,
    /// True if remote set a=sendonly (they are holding us).
    pub is_sendonly: bool,
    /// Remote's offered/answered SRTP key, if the SDP included a supported `a=crypto:` line.
    pub srtp: Option<SrtpParams>,
}

pub fn parse_sdp(sdp: &str) -> Option<ParsedSdp> {
    let mut connection_ip: Option<String>      = None;
    let mut rtp_port:      Option<u16>         = None;
    let mut pt_list:       Vec<u8>             = Vec::new();
    let mut rtpmaps:       Vec<(u8, String)>   = Vec::new();
    let mut dtmf_type:     Option<u8>          = None;
    let mut is_sendonly                        = false;
    let mut srtp:          Option<SrtpParams>  = None;

    for line in sdp.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("c=IN IP4 ") {
            connection_ip = Some(val.trim().to_string());
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

    let ip   = connection_ip?;
    let port = rtp_port?;
    let rtp_addr: SocketAddr = format!("{ip}:{port}").parse().ok()?;

    // Pick the first payload type in the m= line's preference order that we
    // recognize, either from an explicit rtpmap or (for 0/8) the static
    // RTP/AVP defaults when no rtpmap overrides them.
    let (codec, payload_type) = pt_list.iter().find_map(|&pt| {
        if let Some((_, name)) = rtpmaps.iter().find(|(p, _)| *p == pt) {
            if name.starts_with("opus") { return Some((AudioCodec::Opus, pt)); }
            if name.starts_with("pcmu") { return Some((AudioCodec::Pcmu, pt)); }
            if name.starts_with("pcma") { return Some((AudioCodec::Pcma, pt)); }
            return None;
        }
        match pt {
            0 => Some((AudioCodec::Pcmu, pt)),
            8 => Some((AudioCodec::Pcma, pt)),
            _ => None,
        }
    })?;

    Some(ParsedSdp { rtp_addr, codec, payload_type, dtmf_type, is_sendonly, srtp })
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
mod tests {
    use super::*;

    #[test]
    fn offer_prefers_opus() {
        let sdp = build_offer("192.0.2.1", 40000, None);
        let parsed = parse_sdp(&sdp).unwrap();
        assert_eq!(parsed.codec, AudioCodec::Opus);
        assert_eq!(parsed.payload_type, OPUS_PAYLOAD_TYPE);
        assert_eq!(parsed.dtmf_type, Some(101));
        assert!(!parsed.is_sendonly);
        assert!(parsed.srtp.is_none());
    }

    #[test]
    fn answer_honors_selected_codec() {
        for codec in [AudioCodec::Pcmu, AudioCodec::Pcma, AudioCodec::Opus] {
            let sdp = build_answer("192.0.2.2", 40002, codec, None);
            let parsed = parse_sdp(&sdp).unwrap();
            assert_eq!(parsed.codec, codec);
            assert_eq!(parsed.payload_type, codec.payload_type());
        }
    }

    #[test]
    fn offer_with_srtp_uses_savp_and_carries_crypto() {
        let srtp = SrtpParams::generate();
        let sdp = build_offer("192.0.2.1", 40000, Some(&srtp));
        assert!(sdp.contains("RTP/SAVP"));
        let parsed = parse_sdp(&sdp).unwrap();
        assert_eq!(parsed.codec, AudioCodec::Opus);
        assert_eq!(parsed.srtp, Some(srtp));
    }

    #[test]
    fn srtp_crypto_line_roundtrip() {
        let params = SrtpParams::generate();
        let line = params.to_crypto_line(1);
        let parsed = SrtpParams::parse_crypto_line(&line).unwrap();
        assert_eq!(parsed, params);
    }

    #[test]
    fn parse_falls_back_when_opus_unsupported() {
        // Remote offer without opus at all -- PCMA should win as it's first in the list.
        let sdp = "v=0\r\n\
                   o=- 1 1 IN IP4 198.51.100.1\r\n\
                   s=-\r\n\
                   c=IN IP4 198.51.100.1\r\n\
                   t=0 0\r\n\
                   m=audio 30000 RTP/AVP 8 0 101\r\n\
                   a=rtpmap:8 PCMA/8000\r\n\
                   a=rtpmap:0 PCMU/8000\r\n\
                   a=rtpmap:101 telephone-event/8000\r\n\
                   a=sendrecv\r\n";
        let parsed = parse_sdp(sdp).unwrap();
        assert_eq!(parsed.codec, AudioCodec::Pcma);
        assert_eq!(parsed.payload_type, 8);
    }

    #[test]
    fn hold_offer_is_sendonly() {
        let sdp = build_hold_offer("192.0.2.3", 40004, AudioCodec::Opus, None);
        let parsed = parse_sdp(&sdp).unwrap();
        assert!(parsed.is_sendonly);
        assert_eq!(parsed.codec, AudioCodec::Opus);
    }
}
