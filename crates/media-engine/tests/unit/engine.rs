use super::*;
use deelip_sip::SrtpParams;

#[test]
fn jitter_tracker_counts_missing_sequence_numbers_as_loss() {
    let mut tracker = JitterTracker::default();
    let mut stats = LegStats::default();
    let pkt0 = RtpPacket::new(0, 100, 1600, 1, vec![]);
    let pkt1 = RtpPacket::new(0, 103, 1760, 1, vec![]); // 101, 102 missing
    tracker.observe(&mut stats, &pkt0, 8000.0);
    tracker.observe(&mut stats, &pkt1, 8000.0);
    assert_eq!(stats.packets_lost, 2);
}

#[test]
fn jitter_tracker_ignores_huge_gaps_as_reordering_noise() {
    let mut tracker = JitterTracker::default();
    let mut stats = LegStats::default();
    let pkt0 = RtpPacket::new(0, 100, 1600, 1, vec![]);
    let pkt1 = RtpPacket::new(0, 50_000, 1760, 1, vec![]);
    tracker.observe(&mut stats, &pkt0, 8000.0);
    tracker.observe(&mut stats, &pkt1, 8000.0);
    assert_eq!(stats.packets_lost, 0);
}

#[test]
fn jitter_tracker_reports_zero_jitter_for_perfectly_paced_packets() {
    let mut tracker = JitterTracker::default();
    let mut stats = LegStats::default();
    // Three packets 20ms apart in both RTP-timestamp and (as far as this
    // synchronous test can approximate) wall-clock terms.
    for (seq, ts) in [(1u16, 1600u32), (2, 1760), (3, 1920)] {
        let pkt = RtpPacket::new(0, seq, ts, 1, vec![]);
        tracker.observe(&mut stats, &pkt, 8000.0);
    }
    assert!(stats.jitter_ms < 5.0, "jitter should stay small for evenly-paced packets, got {}", stats.jitter_ms);
}

#[test]
fn srtp_roundtrip_preserves_rtp_payload() {
    let params = SrtpParams::generate();
    let mut enc_ctx =
        SrtpContext::new(&params.key, &params.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None).unwrap();
    let mut dec_ctx = SrtpContext::new(
        &params.key,
        &params.salt,
        ProtectionProfile::Aes128CmHmacSha1_80,
        Some(srtp_replay_protection(64)),
        None,
    )
    .unwrap();

    let raw = RtpPacket::new(0, 1, 160, 0xDEAD_BEEF, vec![1, 2, 3, 4, 5]).encode();

    let encrypted = enc_ctx.encrypt_rtp(&raw).unwrap();
    assert!(encrypted.len() > raw.len(), "SRTP appends an auth tag");

    let decrypted = dec_ctx.decrypt_rtp(&encrypted).unwrap();
    assert_eq!(&decrypted[..], &raw[..]);

    let decoded = RtpPacket::decode(&decrypted).unwrap();
    assert_eq!(decoded.payload, vec![1, 2, 3, 4, 5]);
}

#[test]
fn srtp_decrypt_rejects_wrong_key() {
    let params_a = SrtpParams::generate();
    let params_b = SrtpParams::generate();
    let mut enc_ctx =
        SrtpContext::new(&params_a.key, &params_a.salt, ProtectionProfile::Aes128CmHmacSha1_80, None, None).unwrap();
    let mut dec_ctx = SrtpContext::new(
        &params_b.key,
        &params_b.salt,
        ProtectionProfile::Aes128CmHmacSha1_80,
        Some(srtp_replay_protection(64)),
        None,
    )
    .unwrap();

    let raw = RtpPacket::new(0, 1, 160, 0xDEAD_BEEF, vec![1, 2, 3]).encode();
    let encrypted = enc_ctx.encrypt_rtp(&raw).unwrap();
    assert!(dec_ctx.decrypt_rtp(&encrypted).is_err());
}

#[test]
fn mix_frames_sums_and_clamps() {
    let a = vec![100i16, -100, i16::MAX, i16::MIN];
    let b = vec![50i16, -50, i16::MAX, i16::MIN];
    let mixed = mix_frames(&a, &b);
    // Each leg halved (integer truncation) before summing:
    // 100/2 + 50/2 = 75; -100/2 + -50/2 = -75;
    // MAX/2 + MAX/2 = 32766 (truncation loses 1); MIN/2 + MIN/2 = MIN exactly.
    assert_eq!(mixed, vec![75, -75, 32766, i16::MIN]);
}
