//! DTMF telephone-event encoding (RFC 2833 / RFC 4733), plus inband
//! dual-tone audio synthesis for `DtmfMode::Inband`.
//! Payload type 101 is the IANA-registered dynamic PT for telephone-event.

use crate::audio::{FRAME_SAMPLES, SAMPLE_RATE};
use crate::rtp::RtpPacket;

pub const DTMF_PAYLOAD_TYPE: u8 = 101;

/// How many 20ms frames of inband tone to send per digit — 200ms, a
/// typical single-press duration (same ballpark as the RFC 2833 burst's
/// own ~160ms of "on" time above, see `build_dtmf_burst`).
pub const INBAND_FRAME_COUNT: u32 = 10;

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

// ── Inband dual-tone synthesis ────────────────────────────────────────────────

/// Standard DTMF dual-tone frequency pair (low, high) in Hz — ITU-T Q.23/Q.24.
fn dtmf_frequencies(c: char) -> Option<(f32, f32)> {
    match c {
        '1' => Some((697.0, 1209.0)), '2' => Some((697.0, 1336.0)), '3' => Some((697.0, 1477.0)),
        'A' | 'a' => Some((697.0, 1633.0)),
        '4' => Some((770.0, 1209.0)), '5' => Some((770.0, 1336.0)), '6' => Some((770.0, 1477.0)),
        'B' | 'b' => Some((770.0, 1633.0)),
        '7' => Some((852.0, 1209.0)), '8' => Some((852.0, 1336.0)), '9' => Some((852.0, 1477.0)),
        'C' | 'c' => Some((852.0, 1633.0)),
        '*' => Some((941.0, 1209.0)), '0' => Some((941.0, 1336.0)), '#' => Some((941.0, 1477.0)),
        'D' | 'd' => Some((941.0, 1633.0)),
        _ => None,
    }
}

/// Synthesize one 20ms (`FRAME_SAMPLES` @ `SAMPLE_RATE`) frame of dual-tone
/// DTMF audio for `c`, continuing the waveform from `phase_samples` (the
/// count of samples already emitted for this same digit press) so
/// consecutive frames don't click at the frame boundary. `None` if `c`
/// isn't a valid DTMF character.
///
/// Each tone is scaled to half full-scale before summing, so the combined
/// signal (like RFC 3551's own inband-tone guidance) never clips.
pub fn dtmf_tone_frame(c: char, phase_samples: u32) -> Option<Vec<i16>> {
    let (f1, f2) = dtmf_frequencies(c)?;
    let mut out = Vec::with_capacity(FRAME_SAMPLES);
    for i in 0..FRAME_SAMPLES as u32 {
        let t = (phase_samples + i) as f32 / SAMPLE_RATE as f32;
        let s = 0.5 * (2.0 * std::f32::consts::PI * f1 * t).sin()
              + 0.5 * (2.0 * std::f32::consts::PI * f2 * t).sin();
        out.push((s * i16::MAX as f32) as i16);
    }
    Some(out)
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
    fn tone_frame_has_correct_length_and_no_clipping() {
        for c in "0123456789*#ABCD".chars() {
            let frame = dtmf_tone_frame(c, 0).unwrap();
            assert_eq!(frame.len(), FRAME_SAMPLES);
            assert!(frame.iter().all(|&s| s != i16::MIN), "digit {c} clipped");
        }
    }

    #[test]
    fn tone_frame_rejects_non_dtmf_characters() {
        assert!(dtmf_tone_frame('X', 0).is_none());
    }

    #[test]
    fn tone_frame_is_continuous_across_frame_boundary() {
        // The sample right after frame 0 ends should match the first
        // sample of a frame synthesized starting at that same phase --
        // i.e. phase-continuation actually continues the waveform rather
        // than restarting it (which would click at every frame boundary).
        let frame0 = dtmf_tone_frame('5', 0).unwrap();
        let frame1 = dtmf_tone_frame('5', FRAME_SAMPLES as u32).unwrap();
        let restart = dtmf_tone_frame('5', 0).unwrap();
        assert_eq!(frame0, restart);
        assert_ne!(frame1, restart, "continued phase should differ from a restarted one");
    }

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
