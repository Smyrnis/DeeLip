use super::*;

#[test]
fn encode_decode_round_trip() {
    let pkt = RtpPacket::new(8, 100, 1600, 0xdead_beef, vec![1, 2, 3, 4]);
    let decoded = RtpPacket::decode(&pkt.encode()).expect("valid packet");
    assert_eq!(decoded.payload_type, 8);
    assert_eq!(decoded.sequence, 100);
    assert_eq!(decoded.timestamp, 1600);
    assert_eq!(decoded.ssrc, 0xdead_beef);
    assert!(!decoded.marker);
    assert_eq!(decoded.payload, vec![1, 2, 3, 4]);
}

#[test]
fn marker_bit_round_trips() {
    let mut pkt = RtpPacket::new(0, 0, 0, 0, vec![]);
    pkt.marker = true;
    let decoded = RtpPacket::decode(&pkt.encode()).unwrap();
    assert!(decoded.marker);
}

#[test]
fn payload_type_is_masked_to_7_bits() {
    let pkt = RtpPacket::new(0xFF, 0, 0, 0, vec![]);
    let decoded = RtpPacket::decode(&pkt.encode()).unwrap();
    assert_eq!(decoded.payload_type, 0x7F);
}

#[test]
fn decode_rejects_too_short_data() {
    assert!(RtpPacket::decode(&[0u8; 11]).is_none());
}

#[test]
fn decode_rejects_wrong_version() {
    let mut bytes = RtpPacket::new(0, 0, 0, 0, vec![1]).encode();
    bytes[0] = 0x00; // clears the version bits -> version 0, not RTP_VERSION (2)
    assert!(RtpPacket::decode(&bytes).is_none());
}

/// PT=8, sequence=1, timestamp=10, ssrc=1 -- shared by the CSRC/
/// extension-header decode tests below, with just byte 0 (V/P/X/CC) varying.
fn header_with_byte0(byte0: u8) -> Vec<u8> {
    vec![byte0, 8, 0, 1, 0, 0, 0, 10, 0, 0, 0, 1]
}

#[test]
fn decode_skips_csrc_list() {
    let mut bytes = header_with_byte0((RTP_VERSION << 6) | 2); // V=2, CC=2
    bytes.extend_from_slice(&[0u8; 4]); // csrc 1
    bytes.extend_from_slice(&[0u8; 4]); // csrc 2
    bytes.extend_from_slice(&[9, 9, 9]); // payload

    let decoded = RtpPacket::decode(&bytes).expect("valid packet with csrc list");
    assert_eq!(decoded.payload, vec![9, 9, 9]);
}

#[test]
fn decode_skips_extension_header() {
    let mut bytes = header_with_byte0((RTP_VERSION << 6) | 0x10); // V=2, X=1, CC=0
    bytes.extend_from_slice(&[0xBE, 0xEF, 0, 1]); // profile-specific + ext_len (1 word)
    bytes.extend_from_slice(&[0u8; 4]); // the one extension word
    bytes.extend_from_slice(&[7, 7]); // payload

    let decoded = RtpPacket::decode(&bytes).expect("valid packet with extension header");
    assert_eq!(decoded.payload, vec![7, 7]);
}

#[test]
fn decode_rejects_extension_header_claiming_more_data_than_present() {
    let mut bytes = header_with_byte0((RTP_VERSION << 6) | 0x10); // X=1
    bytes.extend_from_slice(&[0, 0, 0, 200]); // claims 200 extension words but none follow
    assert!(RtpPacket::decode(&bytes).is_none());
}

#[test]
fn sender_advances_sequence_and_timestamp() {
    let mut sender = RtpSender::new(8, 160);
    let first = sender.next_packet(vec![]);
    let second = sender.next_packet(vec![]);
    assert_eq!(second.sequence, first.sequence.wrapping_add(1));
    assert_eq!(second.timestamp, first.timestamp.wrapping_add(160));
}

#[test]
fn sender_sequence_wraps_at_u16_max() {
    let mut sender = RtpSender::new(8, 160);
    sender.sequence = u16::MAX;
    let pkt = sender.next_packet(vec![]);
    assert_eq!(pkt.sequence, u16::MAX);
    assert_eq!(sender.sequence, 0);
}

#[test]
fn next_packet_with_pt_overrides_payload_type_but_keeps_shared_state() {
    let mut sender = RtpSender::new(8, 160);
    let pkt = sender.next_packet_with_pt(13, vec![]);
    assert_eq!(pkt.payload_type, 13);
    assert_eq!(sender.sequence, 1);
}

#[test]
fn skip_tick_advances_timestamp_without_touching_sequence() {
    let mut sender = RtpSender::new(8, 160);
    let before_seq = sender.sequence;
    sender.skip_tick();
    assert_eq!(sender.timestamp, 160);
    assert_eq!(sender.sequence, before_seq);
}
