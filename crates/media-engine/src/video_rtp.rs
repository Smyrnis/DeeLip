//! RFC 6184 H.264 RTP payload format -- fragmenting an Annex-B NAL
//! bitstream (as produced by `video_codec::H264Encoder`) into MTU-sized RTP
//! payloads, and the inverse. Operates one level below `rtp::RtpSender`:
//! this produces plain `Vec<u8>` RTP *payloads*; `video_engine.rs` wires
//! each one into an actual `RtpPacket` (one shared timestamp per video
//! frame, marker bit on the last packet). Full picture: `docs/crates/media-engine.md`.

/// RFC 6184 §5.8: the NAL-unit-type value (5 low bits of the first byte)
/// that marks a packet as an FU-A fragment rather than a plain NAL unit.
const FU_A_TYPE: u8 = 28;

/// Scan an Annex-B (start-code-delimited) H.264 bitstream and return each
/// NAL unit's raw bytes (start code stripped, header byte + payload per
/// output slice). Relies on encoder-side emulation prevention (which every
/// spec-compliant H.264 encoder, including `openh264`, applies) guaranteeing
/// real NAL payload never contains a raw `00 00 01`/`00 00 00 01` byte
/// sequence -- so a plain byte scan for that pattern unambiguously finds
/// only real start codes.
pub fn split_nal_units(annex_b: &[u8]) -> Vec<&[u8]> {
    let mut starts: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + 3 <= annex_b.len() {
        if annex_b[i] == 0 && annex_b[i + 1] == 0 && annex_b[i + 2] == 1 {
            starts.push(i);
            i += 3;
        } else {
            i += 1;
        }
    }

    let mut nals = Vec::with_capacity(starts.len());
    for (idx, &start) in starts.iter().enumerate() {
        let content_start = start + 3;
        let content_end = starts
            .get(idx + 1)
            .map(|&next| {
                // A 4-byte start code (`00 00 00 01`) is a 3-byte one with
                // one extra leading zero -- that zero belongs to the start
                // code, not this NAL's content, so trim it off.
                let mut end = next;
                while end > content_start && annex_b[end - 1] == 0 {
                    end -= 1;
                }
                end
            })
            .unwrap_or(annex_b.len());
        if content_end > content_start {
            nals.push(&annex_b[content_start..content_end]);
        }
    }
    nals
}

/// For each NAL unit found in `annex_b`: emit it as-is (a "Single NAL Unit
/// Packet", RFC 6184 §5.6) if it fits within `mtu`, otherwise split it into
/// RFC 6184 §5.8 FU-A fragments. Returns the ordered list of RTP payload
/// byte-strings for the whole frame.
pub fn fragment_nal_units(annex_b: &[u8], mtu: usize) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    for nal in split_nal_units(annex_b) {
        if nal.is_empty() {
            continue;
        }
        if nal.len() <= mtu {
            packets.push(nal.to_vec());
            continue;
        }

        let nal_header = nal[0];
        let fu_indicator = (nal_header & 0xE0) | FU_A_TYPE;
        let original_type = nal_header & 0x1F;
        let payload = &nal[1..];
        let chunk_size = mtu - 2; // FU indicator + FU header bytes
        let chunks: Vec<&[u8]> = payload.chunks(chunk_size.max(1)).collect();
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut fu_header = original_type;
            if i == 0 {
                fu_header |= 0x80; // Start bit
            }
            if i == last {
                fu_header |= 0x40; // End bit
            }
            let mut packet = Vec::with_capacity(2 + chunk.len());
            packet.push(fu_indicator);
            packet.push(fu_header);
            packet.extend_from_slice(chunk);
            packets.push(packet);
        }
    }
    packets
}

/// Inverse of `fragment_nal_units`: walks a frame's ordered RTP payloads
/// (both plain Single-NAL packets and FU-A fragment sequences) and
/// reconstructs the Annex-B bytestream, re-adding 4-byte start codes and
/// reassembling FU-A fragments back into whole NAL units (with the
/// original NAL header byte reconstructed from the FU indicator/header per
/// RFC 6184 §5.8).
pub fn reassemble_nal_units(rtp_payloads: &[Vec<u8>]) -> Vec<u8> {
    const START_CODE: [u8; 4] = [0, 0, 0, 1];
    let mut out = Vec::new();
    let mut fu_accum: Option<Vec<u8>> = None;

    for payload in rtp_payloads {
        if payload.is_empty() {
            continue;
        }
        let nal_type = payload[0] & 0x1F;
        if nal_type == FU_A_TYPE {
            if payload.len() < 2 {
                continue;
            }
            let fu_indicator = payload[0];
            let fu_header = payload[1];
            let is_start = fu_header & 0x80 != 0;
            let is_end = fu_header & 0x40 != 0;
            let original_type = fu_header & 0x1F;
            let chunk = &payload[2..];

            if is_start {
                let nal_header = (fu_indicator & 0xE0) | original_type;
                let mut buf = Vec::with_capacity(1 + chunk.len());
                buf.push(nal_header);
                buf.extend_from_slice(chunk);
                fu_accum = Some(buf);
            } else if let Some(buf) = fu_accum.as_mut() {
                buf.extend_from_slice(chunk);
            } else {
                // A continuation/end fragment arrived with no preceding
                // start fragment (lost packet) -- nothing sane to
                // reassemble from, drop it rather than emit a corrupt NAL.
                continue;
            }

            if is_end && let Some(buf) = fu_accum.take() {
                out.extend_from_slice(&START_CODE);
                out.extend_from_slice(&buf);
            }
        } else {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(payload);
        }
    }
    out
}

#[cfg(test)]
#[path = "../tests/unit/video_rtp.rs"]
mod tests;
