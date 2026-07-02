//! DTMF telephone-event encoding (RFC 2833 / RFC 4733).
//! Payload type 101 is the IANA-registered dynamic PT for telephone-event.

use crate::rtp::RtpPacket;

pub const DTMF_PAYLOAD_TYPE: u8 = 101;

// ── Digit → event code ────────────────────────────────────────────────────────

/// Map a DTMF character to its RFC 2833 event code (0-15).
pub fn char_to_event(c: char) -> Option<u8> {
    match c {
        '0'..='9' => Some(c as u8 - b'0'),
        '*' => Some(10),
        '#' => Some(11),
        'A' | 'a' => Some(12),
        'B' | 'b' => Some(13),
        'C' | 'c' => Some(14),
        'D' | 'd' => Some(15),
        _ => None,
    }
}

// ── Payload encoding ──────────────────────────────────────────────────────────

/// Encode a 4-byte telephone-event payload.
/// - `event`: digit code (0-15)
/// - `end`:   true on the final (end) packets
/// - `volume`: loudness (0 = loudest, 63 = softest; typically 10)
/// - `duration`: in timestamp units (8000 Hz → 160 per 20 ms)
pub fn encode_dtmf_payload(event: u8, end: bool, volume: u8, duration: u16) -> Vec<u8> {
    let e_vol = if end { 0x80 | (volume & 0x3F) } else { volume & 0x3F };
    vec![event, e_vol, (duration >> 8) as u8, duration as u8]
}

// ── Burst builder ─────────────────────────────────────────────────────────────

/// Build a complete RFC 2833 DTMF burst (5 RTP packets) for one digit.
///
/// Protocol requirement: all packets for the same event share the same RTP
/// timestamp.  The sequence numbers advance monotonically.  Three end packets
/// (E bit set) are sent at the close of the event.
///
/// Returns the encoded wire bytes for each of the 5 packets.
pub fn build_dtmf_burst(
    event:   u8,
    ssrc:    u32,
    seq:     &mut u16,
    base_ts: u32,
    dtmf_pt: u8,
) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(5);

    // 1. Start packet — marker=true, E=false, duration=160 (~20 ms)
    let mut start = RtpPacket::new(dtmf_pt, *seq, base_ts, ssrc,
        encode_dtmf_payload(event, false, 10, 160));
    start.marker = true;
    out.push(start.encode());
    *seq = seq.wrapping_add(1);

    // 2. Middle packet — marker=false, E=false, duration=320 (~40 ms)
    let mid = RtpPacket::new(dtmf_pt, *seq, base_ts, ssrc,
        encode_dtmf_payload(event, false, 10, 320));
    out.push(mid.encode());
    *seq = seq.wrapping_add(1);

    // 3–5. End packets — E=true, same timestamp, duration=480 (~60 ms)
    for _ in 0..3 {
        let end = RtpPacket::new(dtmf_pt, *seq, base_ts, ssrc,
            encode_dtmf_payload(event, true, 10, 480));
        out.push(end.encode());
        *seq = seq.wrapping_add(1);
    }

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_mapping() {
        assert_eq!(char_to_event('0'), Some(0));
        assert_eq!(char_to_event('9'), Some(9));
        assert_eq!(char_to_event('*'), Some(10));
        assert_eq!(char_to_event('#'), Some(11));
        assert_eq!(char_to_event('A'), Some(12));
        assert_eq!(char_to_event('D'), Some(15));
        assert_eq!(char_to_event('X'), None);
    }

    #[test]
    fn payload_encoding() {
        // Start packet for digit '5': event=5, E=false, vol=10, dur=160
        let p = encode_dtmf_payload(5, false, 10, 160);
        assert_eq!(p, vec![5, 10, 0, 160]);

        // End packet: E bit should be in the MSB of byte 1
        let p = encode_dtmf_payload(5, true, 10, 480);
        assert_eq!(p[1] & 0x80, 0x80);
        assert_eq!(p[1] & 0x3F, 10); // volume preserved
    }

    #[test]
    fn burst_has_five_packets() {
        let mut seq = 100u16;
        let pkts = build_dtmf_burst(5, 0xDEAD, &mut seq, 1000, DTMF_PAYLOAD_TYPE);
        assert_eq!(pkts.len(), 5);
        assert_eq!(seq, 105); // seq advanced by 5
    }

    #[test]
    fn burst_timestamps_are_same() {
        use crate::rtp::RtpPacket;
        let mut seq = 0u16;
        let pkts = build_dtmf_burst(1, 0, &mut seq, 9999, DTMF_PAYLOAD_TYPE);
        // All 5 packets must share base_ts=9999
        for raw in &pkts {
            let pkt = RtpPacket::decode(raw).unwrap();
            assert_eq!(pkt.timestamp, 9999);
        }
        // First packet must have marker bit
        let first = RtpPacket::decode(&pkts[0]).unwrap();
        assert!(first.marker);
        // Last 3 packets must have end bit
        for raw in &pkts[2..] {
            let pkt = RtpPacket::decode(raw).unwrap();
            assert_eq!(pkt.payload[1] & 0x80, 0x80);
        }
    }
}
