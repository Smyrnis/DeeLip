use super::*;

fn sample_hello() -> Hello {
    Hello {
        version: *b"1.10",
        client_id: *b"DeeLip          ",
        h3: [0x11; 32],
        zid: [0x22; 12],
        mitm_capable: false,
        hashes: vec![*b"S256"],
        ciphers: vec![*b"AES1"],
        auths: vec![*b"HS80"],
        key_agreements: vec![*b"EC25"],
        sas_types: vec![*b"B32 "],
        mac: [0x33; 8],
    }
}

fn sample_commit() -> Commit {
    Commit {
        h2: [0x44; 32],
        zid: [0x22; 12],
        hash: *b"S256",
        cipher: *b"AES1",
        auth: *b"HS80",
        key_agreement: *b"EC25",
        sas: *b"B32 ",
        hvi: [0x55; 32],
        mac: [0x66; 8],
    }
}

fn sample_dhpart() -> DhPart {
    DhPart {
        h1: [0x77; 32],
        rs1_id: [0x01; 8],
        rs2_id: [0x02; 8],
        aux_id: [0x03; 8],
        pbx_id: [0x04; 8],
        pv: vec![0xab; 64],
        mac: [0x88; 8],
    }
}

fn sample_confirm() -> Confirm {
    Confirm { confirm_mac: [0x99; 8], cfb_iv: [0xaa; 16], encrypted: vec![0xbb; 36] }
}

#[test]
fn hello_roundtrip() {
    let msg = Message::Hello(sample_hello());
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn commit_roundtrip() {
    let msg = Message::Commit(sample_commit());
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn dhpart1_roundtrip() {
    let msg = Message::DhPart1(sample_dhpart());
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn dhpart2_roundtrip() {
    let msg = Message::DhPart2(sample_dhpart());
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn confirm1_roundtrip() {
    let msg = Message::Confirm1(sample_confirm());
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn confirm2_roundtrip() {
    let msg = Message::Confirm2(sample_confirm());
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn conf2ack_roundtrip() {
    let msg = Message::Conf2Ack;
    assert_eq!(Message::decode(&msg.encode()).unwrap(), msg);
}

#[test]
fn bad_crc_is_rejected() {
    let mut bytes = Message::Hello(sample_hello()).encode();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    assert!(matches!(Message::decode(&bytes), Err(WireError::BadCrc)));
}

#[test]
fn bad_preamble_is_rejected() {
    let mut bytes = Message::Hello(sample_hello()).encode();
    bytes[0] = 0;
    bytes[1] = 0;
    assert!(matches!(Message::decode(&bytes), Err(WireError::BadPreamble)));
}

#[test]
fn unknown_message_type_is_rejected() {
    let mut bytes = Message::Conf2Ack.encode();
    bytes[4..12].copy_from_slice(&message_type_bytes("Bogus"));
    // Recompute the CRC so this fails on the *type* check, not the CRC check.
    let crc_at = bytes.len() - 4;
    let crc = CRC32.checksum(&bytes[..crc_at]);
    bytes[crc_at..].copy_from_slice(&crc.to_be_bytes());
    assert!(matches!(Message::decode(&bytes), Err(WireError::UnknownMessageType(_))));
}

#[test]
fn packet_roundtrip() {
    let packet = Packet { sequence: 42, ssrc: 0xdead_beef, message: Message::Hello(sample_hello()) };
    let bytes = packet.encode();
    assert!(is_zrtp_packet(&bytes));
    assert_eq!(Packet::decode(&bytes).unwrap(), packet);
}

#[test]
fn packet_rejects_bad_magic_cookie() {
    let mut bytes = Packet { sequence: 1, ssrc: 2, message: Message::Conf2Ack }.encode();
    bytes[4] = 0;
    assert!(!is_zrtp_packet(&bytes));
    assert!(matches!(Packet::decode(&bytes), Err(WireError::BadMagicCookie)));
}

#[test]
fn packet_too_short_is_rejected() {
    assert!(matches!(Packet::decode(&[0u8; 8]), Err(WireError::TooShort)));
}

#[test]
fn is_zrtp_packet_false_for_plain_rtp() {
    // A plausible plain RTP header: version=2 in the top bits, PT=0, etc.
    let rtp_like = [0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0, 0, 0, 0];
    assert!(!is_zrtp_packet(&rtp_like));
}
