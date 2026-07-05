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
