use super::*;

#[test]
fn dtls_record_range_is_classified_as_dtls() {
    // RFC 5764 §5.1.2: first byte 20-63 (inclusive) is a DTLS record.
    for b in 20u8..=63 {
        assert!(is_dtls_packet(&[b, 0, 0]), "byte {b} should classify as DTLS");
    }
}

#[test]
fn boundary_bytes_outside_the_dtls_range_are_rejected() {
    assert!(!is_dtls_packet(&[19, 0, 0]), "19 is just below the DTLS range");
    assert!(!is_dtls_packet(&[64, 0, 0]), "64 is just above the DTLS range");
}

#[test]
fn rtp_and_rtcp_first_bytes_are_not_dtls() {
    // A real RTP/RTCP header's first byte is `V=2,P,X,CC` -- version bits
    // 10 in the top two bits put the byte at 0x80 or higher, always well
    // outside the 20-63 DTLS range. This is the actual disambiguation the
    // shared-socket recv loop depends on (`tasks.rs`'s `recv_loop` checks
    // `is_zrtp_packet`/`is_dtls_packet` before falling through to RTP
    // decode).
    assert!(!is_dtls_packet(&[0x80, 0, 0]), "plain RTP header byte");
    assert!(!is_dtls_packet(&[0x81, 0, 0]), "RTP header byte with marker bit set");
    // RTCP sender/receiver report first bytes (V=2 too).
    assert!(!is_dtls_packet(&[0xC8, 0, 0]), "RTCP SR-shaped first byte");
    assert!(!is_dtls_packet(&[0xCA, 0, 0]), "RTCP SDES-shaped first byte");
}

#[test]
fn empty_buffer_is_not_dtls() {
    assert!(!is_dtls_packet(&[]));
}
