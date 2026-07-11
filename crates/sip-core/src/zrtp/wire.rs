//! ZRTP (RFC 6189) packet/message wire format -- hand-rolled, matching this
//! crate's existing style for `wire/sdp.rs`/`wire/message.rs`. Provenance of
//! the framing constants below, verification status, and the scope cuts
//! from the full RFC: `docs/zrtp.md`.

use crc::{Crc, CRC_32_ISO_HDLC};

/// ASCII "ZRTP" -- identifies a ZRTP packet sharing the RTP socket, since
/// the first two octets (see `HEADER_PREFIX`) alone aren't part of any
/// standard RTP payload-type/version combination a real RTP stack would
/// otherwise mistake this for.
pub const ZRTP_MAGIC_COOKIE: [u8; 4] = *b"ZRTP";
/// First two octets of every ZRTP packet -- deliberately not a valid RTP
/// version+flags combination (RTP v2 packets always have their top two bits
/// set to `10`), so a plain RTP receiver that doesn't know about ZRTP can
/// still tell packets apart on sight if it ever needed to.
const HEADER_PREFIX: [u8; 2] = [0x10, 0x00];
/// Two-octet signature at the start of every ZRTP *message* (inside the
/// packet, after the 12-byte outer header).
const MSG_PREAMBLE: u16 = 0x505a;

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

fn message_type_bytes(tag: &str) -> [u8; 8] {
    let mut out = [b' '; 8];
    let bytes = tag.as_bytes();
    let n = bytes.len().min(8);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("packet too short")]
    TooShort,
    #[error("bad magic cookie")]
    BadMagicCookie,
    #[error("bad message preamble")]
    BadPreamble,
    #[error("bad CRC")]
    BadCrc,
    #[error("truncated message body")]
    Truncated,
    #[error("unknown message type: {0:?}")]
    UnknownMessageType(String),
    #[error("unsupported algorithm")]
    UnsupportedAlgorithm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub version: [u8; 4],
    pub client_id: [u8; 16],
    pub h3: [u8; 32],
    pub zid: [u8; 12],
    pub mitm_capable: bool,
    pub hashes: Vec<[u8; 4]>,
    pub ciphers: Vec<[u8; 4]>,
    pub auths: Vec<[u8; 4]>,
    pub key_agreements: Vec<[u8; 4]>,
    pub sas_types: Vec<[u8; 4]>,
    pub mac: [u8; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub h2: [u8; 32],
    pub zid: [u8; 12],
    pub hash: [u8; 4],
    pub cipher: [u8; 4],
    pub auth: [u8; 4],
    pub key_agreement: [u8; 4],
    pub sas: [u8; 4],
    pub hvi: [u8; 32],
    pub mac: [u8; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhPart {
    pub h1: [u8; 32],
    pub rs1_id: [u8; 8],
    pub rs2_id: [u8; 8],
    pub aux_id: [u8; 8],
    pub pbx_id: [u8; 8],
    /// Public key value -- 64 bytes (uncompressed P-256 X||Y) for EC25, the
    /// only key-agreement algorithm this implementation supports.
    pub pv: Vec<u8>,
    pub mac: [u8; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub confirm_mac: [u8; 8],
    pub cfb_iv: [u8; 16],
    /// AES-128-CFB-encrypted payload: h0 (32 bytes) || cache expiration
    /// interval (4 bytes, big-endian seconds). No signature block (not
    /// implemented).
    pub encrypted: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Commit(Commit),
    DhPart1(DhPart),
    DhPart2(DhPart),
    Confirm1(Confirm),
    Confirm2(Confirm),
    Conf2Ack,
}

impl Message {
    fn type_tag(&self) -> &'static str {
        match self {
            Message::Hello(_) => "Hello",
            Message::Commit(_) => "Commit",
            Message::DhPart1(_) => "DHPart1",
            Message::DhPart2(_) => "DHPart2",
            Message::Confirm1(_) => "Confirm1",
            Message::Confirm2(_) => "Confirm2",
            Message::Conf2Ack => "Conf2ACK",
        }
    }

    /// Encode just the message (preamble + length + type + body + CRC),
    /// without the outer 12-byte packet header -- used both standalone and
    /// by `Packet::encode`.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        match self {
            Message::Hello(h) => encode_hello(h, &mut body),
            Message::Commit(c) => encode_commit(c, &mut body),
            Message::DhPart1(d) | Message::DhPart2(d) => encode_dhpart(d, &mut body),
            Message::Confirm1(c) | Message::Confirm2(c) => encode_confirm(c, &mut body),
            Message::Conf2Ack => {}
        }

        let mut out = Vec::with_capacity(4 + 8 + body.len() + 4);
        out.extend_from_slice(&MSG_PREAMBLE.to_be_bytes());
        // Length in octets, covering the type block + body + CRC
        // (everything after the length field itself). A plain byte count
        // rather than a word count (unlike, per general recollection of
        // the protocol, RFC 6189 itself) -- sidesteps a word-alignment
        // requirement this implementation has no need for, since nothing
        // outside this module ever inspects this field directly. See this
        // file's module doc comment: the exact on-the-wire convention here
        // wasn't obtainable from spec text this session, so this is a
        // self-consistent choice, not a claim of RFC fidelity.
        let byte_len = (8 + body.len() + 4) as u16;
        out.extend_from_slice(&byte_len.to_be_bytes());
        out.extend_from_slice(&message_type_bytes(self.type_tag()));
        out.extend_from_slice(&body);
        let crc = CRC32.checksum(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() < 4 + 8 + 4 {
            return Err(WireError::Truncated);
        }
        let preamble = u16::from_be_bytes([bytes[0], bytes[1]]);
        if preamble != MSG_PREAMBLE {
            return Err(WireError::BadPreamble);
        }
        let byte_len = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        let total_len = 4 + byte_len;
        if bytes.len() < total_len {
            return Err(WireError::Truncated);
        }
        let bytes = &bytes[..total_len];

        let crc_at = total_len - 4;
        let expected_crc = u32::from_be_bytes(bytes[crc_at..].try_into().unwrap());
        let actual_crc = CRC32.checksum(&bytes[..crc_at]);
        if expected_crc != actual_crc {
            return Err(WireError::BadCrc);
        }

        let type_tag = String::from_utf8_lossy(&bytes[4..12]).trim_end().to_string();
        let body = &bytes[12..crc_at];

        match type_tag.as_str() {
            "Hello" => decode_hello(body).map(Message::Hello),
            "Commit" => decode_commit(body).map(Message::Commit),
            "DHPart1" => decode_dhpart(body).map(Message::DhPart1),
            "DHPart2" => decode_dhpart(body).map(Message::DhPart2),
            "Confirm1" => decode_confirm(body).map(Message::Confirm1),
            "Confirm2" => decode_confirm(body).map(Message::Confirm2),
            "Conf2ACK" => Ok(Message::Conf2Ack),
            other => Err(WireError::UnknownMessageType(other.to_string())),
        }
    }
}

fn encode_algo_list(list: &[[u8; 4]], out: &mut Vec<u8>) {
    out.push(list.len() as u8);
    for algo in list {
        out.extend_from_slice(algo);
    }
}

fn decode_algo_list(body: &[u8], pos: &mut usize) -> Result<Vec<[u8; 4]>, WireError> {
    let count = *body.get(*pos).ok_or(WireError::Truncated)? as usize;
    *pos += 1;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let chunk = body.get(*pos..*pos + 4).ok_or(WireError::Truncated)?;
        out.push(chunk.try_into().unwrap());
        *pos += 4;
    }
    Ok(out)
}

fn encode_hello(h: &Hello, out: &mut Vec<u8>) {
    out.extend_from_slice(&h.version);
    out.extend_from_slice(&h.client_id);
    out.extend_from_slice(&h.h3);
    out.extend_from_slice(&h.zid);
    out.push(u8::from(h.mitm_capable));
    encode_algo_list(&h.hashes, out);
    encode_algo_list(&h.ciphers, out);
    encode_algo_list(&h.auths, out);
    encode_algo_list(&h.key_agreements, out);
    encode_algo_list(&h.sas_types, out);
    out.extend_from_slice(&h.mac);
}

fn decode_hello(body: &[u8]) -> Result<Hello, WireError> {
    if body.len() < 4 + 16 + 32 + 12 + 1 {
        return Err(WireError::Truncated);
    }
    let mut pos = 0;
    let version: [u8; 4] = body[pos..pos + 4].try_into().unwrap();
    pos += 4;
    let client_id: [u8; 16] = body[pos..pos + 16].try_into().unwrap();
    pos += 16;
    let h3: [u8; 32] = body[pos..pos + 32].try_into().unwrap();
    pos += 32;
    let zid: [u8; 12] = body[pos..pos + 12].try_into().unwrap();
    pos += 12;
    let mitm_capable = body[pos] != 0;
    pos += 1;
    let hashes = decode_algo_list(body, &mut pos)?;
    let ciphers = decode_algo_list(body, &mut pos)?;
    let auths = decode_algo_list(body, &mut pos)?;
    let key_agreements = decode_algo_list(body, &mut pos)?;
    let sas_types = decode_algo_list(body, &mut pos)?;
    let mac = body.get(pos..pos + 8).ok_or(WireError::Truncated)?;
    Ok(Hello {
        version,
        client_id,
        h3,
        zid,
        mitm_capable,
        hashes,
        ciphers,
        auths,
        key_agreements,
        sas_types,
        mac: mac.try_into().unwrap(),
    })
}

fn encode_commit(c: &Commit, out: &mut Vec<u8>) {
    out.extend_from_slice(&c.h2);
    out.extend_from_slice(&c.zid);
    out.extend_from_slice(&c.hash);
    out.extend_from_slice(&c.cipher);
    out.extend_from_slice(&c.auth);
    out.extend_from_slice(&c.key_agreement);
    out.extend_from_slice(&c.sas);
    out.extend_from_slice(&c.hvi);
    out.extend_from_slice(&c.mac);
}

#[allow(unused_assignments)] // the final `take!`'s trailing `pos += n` is dead, harmless
fn decode_commit(body: &[u8]) -> Result<Commit, WireError> {
    if body.len() < 32 + 12 + 4 * 5 + 32 + 8 {
        return Err(WireError::Truncated);
    }
    let mut pos = 0;
    macro_rules! take {
        ($n:expr) => {{
            let s = &body[pos..pos + $n];
            pos += $n;
            s
        }};
    }
    let h2: [u8; 32] = take!(32).try_into().unwrap();
    let zid: [u8; 12] = take!(12).try_into().unwrap();
    let hash: [u8; 4] = take!(4).try_into().unwrap();
    let cipher: [u8; 4] = take!(4).try_into().unwrap();
    let auth: [u8; 4] = take!(4).try_into().unwrap();
    let key_agreement: [u8; 4] = take!(4).try_into().unwrap();
    let sas: [u8; 4] = take!(4).try_into().unwrap();
    let hvi: [u8; 32] = take!(32).try_into().unwrap();
    let mac: [u8; 8] = take!(8).try_into().unwrap();
    Ok(Commit { h2, zid, hash, cipher, auth, key_agreement, sas, hvi, mac })
}

fn encode_dhpart(d: &DhPart, out: &mut Vec<u8>) {
    out.extend_from_slice(&d.h1);
    out.extend_from_slice(&d.rs1_id);
    out.extend_from_slice(&d.rs2_id);
    out.extend_from_slice(&d.aux_id);
    out.extend_from_slice(&d.pbx_id);
    out.extend_from_slice(&(d.pv.len() as u16).to_be_bytes());
    out.extend_from_slice(&d.pv);
    out.extend_from_slice(&d.mac);
}

fn decode_dhpart(body: &[u8]) -> Result<DhPart, WireError> {
    if body.len() < 32 + 8 * 4 + 2 {
        return Err(WireError::Truncated);
    }
    let mut pos = 0;
    macro_rules! take {
        ($n:expr) => {{
            let s = &body[pos..pos + $n];
            pos += $n;
            s
        }};
    }
    let h1: [u8; 32] = take!(32).try_into().unwrap();
    let rs1_id: [u8; 8] = take!(8).try_into().unwrap();
    let rs2_id: [u8; 8] = take!(8).try_into().unwrap();
    let aux_id: [u8; 8] = take!(8).try_into().unwrap();
    let pbx_id: [u8; 8] = take!(8).try_into().unwrap();
    let pv_len = u16::from_be_bytes(take!(2).try_into().unwrap()) as usize;
    let pv = body.get(pos..pos + pv_len).ok_or(WireError::Truncated)?.to_vec();
    pos += pv_len;
    let mac: [u8; 8] = body.get(pos..pos + 8).ok_or(WireError::Truncated)?.try_into().unwrap();
    Ok(DhPart { h1, rs1_id, rs2_id, aux_id, pbx_id, pv, mac })
}

fn encode_confirm(c: &Confirm, out: &mut Vec<u8>) {
    out.extend_from_slice(&c.confirm_mac);
    out.extend_from_slice(&c.cfb_iv);
    out.extend_from_slice(&(c.encrypted.len() as u16).to_be_bytes());
    out.extend_from_slice(&c.encrypted);
}

fn decode_confirm(body: &[u8]) -> Result<Confirm, WireError> {
    if body.len() < 8 + 16 + 2 {
        return Err(WireError::Truncated);
    }
    let confirm_mac: [u8; 8] = body[0..8].try_into().unwrap();
    let cfb_iv: [u8; 16] = body[8..24].try_into().unwrap();
    let enc_len = u16::from_be_bytes(body[24..26].try_into().unwrap()) as usize;
    let encrypted = body.get(26..26 + enc_len).ok_or(WireError::Truncated)?.to_vec();
    Ok(Confirm { confirm_mac, cfb_iv, encrypted })
}

/// One ZRTP packet: the 12-byte outer header (shares the RTP socket/port,
/// distinguished from real RTP by `ZRTP_MAGIC_COOKIE`) plus the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub sequence: u16,
    pub ssrc: u32,
    pub message: Message,
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + 64);
        out.extend_from_slice(&HEADER_PREFIX);
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&ZRTP_MAGIC_COOKIE);
        out.extend_from_slice(&self.ssrc.to_be_bytes());
        out.extend_from_slice(&self.message.encode());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() < 12 {
            return Err(WireError::TooShort);
        }
        if bytes[4..8] != ZRTP_MAGIC_COOKIE {
            return Err(WireError::BadMagicCookie);
        }
        let sequence = u16::from_be_bytes([bytes[2], bytes[3]]);
        let ssrc = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        let message = Message::decode(&bytes[12..])?;
        Ok(Packet { sequence, ssrc, message })
    }
}

/// Cheap sniff for "is this a ZRTP packet, not RTP/RTCP" -- checks just the
/// magic cookie at its fixed offset, for the RTP receive loop to branch on
/// before doing any real parsing.
pub fn is_zrtp_packet(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[4..8] == ZRTP_MAGIC_COOKIE
}

#[cfg(test)]
#[path = "../../tests/unit/zrtp/wire.rs"]
mod tests;
