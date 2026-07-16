use super::*;

/// Exact `payload_type()`/`rtpmap()`/`fmtp()` values per `AudioCodec`
/// variant -- `sdp.rs`'s own tests exercise these indirectly through
/// `build_offer`/`parse_sdp` round-trips, but don't pin every variant's
/// precise rtpmap/fmtp string, which is what actually goes out on the wire
/// and what interop with a given far end depends on.
#[test]
fn pcmu_static_assignment() {
    assert_eq!(AudioCodec::Pcmu.payload_type(), 0);
    assert_eq!(AudioCodec::Pcmu.rtpmap(), "PCMU/8000");
    assert_eq!(AudioCodec::Pcmu.fmtp(), None);
}

#[test]
fn pcma_static_assignment() {
    assert_eq!(AudioCodec::Pcma.payload_type(), 8);
    assert_eq!(AudioCodec::Pcma.rtpmap(), "PCMA/8000");
    assert_eq!(AudioCodec::Pcma.fmtp(), None);
}

#[test]
fn gsm_static_assignment() {
    assert_eq!(AudioCodec::Gsm.payload_type(), 3);
    assert_eq!(AudioCodec::Gsm.rtpmap(), "GSM/8000");
    assert_eq!(AudioCodec::Gsm.fmtp(), None);
}

/// G.722's RTP clock is spec-mandated as 8000 (RFC 3551) despite the codec
/// operating at 16kHz internally -- a well-known historical quirk that's
/// easy to "fix" by mistake.
#[test]
fn g722_uses_8000_clock_not_16000() {
    assert_eq!(AudioCodec::G722.payload_type(), 9);
    assert_eq!(AudioCodec::G722.rtpmap(), "G722/8000");
    assert_eq!(AudioCodec::G722.fmtp(), None);
}

#[test]
fn opus_dynamic_pt_and_fec_fmtp() {
    assert_eq!(AudioCodec::Opus.payload_type(), OPUS_PAYLOAD_TYPE);
    assert_eq!(AudioCodec::Opus.rtpmap(), "opus/48000/2");
    assert_eq!(AudioCodec::Opus.fmtp(), Some(format!("a=fmtp:{OPUS_PAYLOAD_TYPE} useinbandfec=1\r\n")));
}

#[test]
fn ilbc_dynamic_pt_and_20ms_mode_fmtp() {
    assert_eq!(AudioCodec::Ilbc.payload_type(), ILBC_PAYLOAD_TYPE);
    assert_eq!(AudioCodec::Ilbc.rtpmap(), "iLBC/8000");
    assert_eq!(AudioCodec::Ilbc.fmtp(), Some(format!("a=fmtp:{ILBC_PAYLOAD_TYPE} mode=20\r\n")));
}

#[test]
fn g729_static_pt_and_annexb_no_fmtp() {
    assert_eq!(AudioCodec::G729.payload_type(), 18);
    assert_eq!(AudioCodec::G729.rtpmap(), "G729/8000");
    assert_eq!(AudioCodec::G729.fmtp(), Some("a=fmtp:18 annexb=no\r\n".to_string()));
}

#[test]
fn l16_dynamic_pt_and_no_fmtp() {
    assert_eq!(AudioCodec::L16.payload_type(), L16_PAYLOAD_TYPE);
    assert_eq!(AudioCodec::L16.rtpmap(), "L16/8000");
    assert_eq!(AudioCodec::L16.fmtp(), None);
}

#[test]
fn all_codecs_has_no_duplicate_payload_types() {
    let mut seen = std::collections::HashSet::new();
    for codec in ALL_CODECS {
        assert!(seen.insert(codec.payload_type()), "duplicate payload type for {codec:?}");
    }
    assert_eq!(seen.len(), ALL_CODECS.len());
}

#[test]
fn all_codecs_covers_every_currently_known_variant_exactly_once() {
    let count = |c: AudioCodec| ALL_CODECS.iter().filter(|&&x| x == c).count();
    for codec in [
        AudioCodec::Pcmu,
        AudioCodec::Pcma,
        AudioCodec::Opus,
        AudioCodec::G722,
        AudioCodec::Gsm,
        AudioCodec::Ilbc,
        AudioCodec::G729,
        AudioCodec::L16,
    ] {
        assert_eq!(count(codec), 1, "{codec:?} must appear exactly once in ALL_CODECS");
    }
}

#[test]
fn rtpmap_strings_are_all_distinct() {
    let mut seen = std::collections::HashSet::new();
    for codec in ALL_CODECS {
        assert!(seen.insert(codec.rtpmap()), "duplicate rtpmap string for {codec:?}");
    }
}
