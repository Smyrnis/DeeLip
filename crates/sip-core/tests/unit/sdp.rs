use super::*;

#[test]
fn offer_prefers_opus() {
    let sdp = build_offer("192.0.2.1", 40000, None, &ALL_CODECS, None, false);
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert_eq!(parsed.codec, AudioCodec::Opus);
    assert_eq!(parsed.payload_type, OPUS_PAYLOAD_TYPE);
    assert_eq!(parsed.dtmf_type, Some(101));
    assert!(!parsed.is_sendonly);
    assert!(parsed.srtp.is_none());
}

#[test]
fn answer_honors_selected_codec() {
    for codec in [
        AudioCodec::Pcmu,
        AudioCodec::Pcma,
        AudioCodec::Opus,
        AudioCodec::G722,
    ] {
        let sdp = build_answer("192.0.2.2", 40002, codec, None, None, false);
        let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
        assert_eq!(parsed.codec, codec);
        assert_eq!(parsed.payload_type, codec.payload_type());
    }
}

#[test]
fn offer_includes_g722_with_correct_clock_quirk() {
    let sdp = build_offer("192.0.2.1", 40000, None, &ALL_CODECS, None, false);
    assert!(
        sdp.contains("a=rtpmap:9 G722/8000"),
        "G722's RTP clock must be signalled as 8000 per RFC 3551, not 16000"
    );
    // An answerer selecting G722 (e.g. one without Opus support) must parse correctly.
    let g722_only = "v=0\r\n\
                      o=- 1 1 IN IP4 198.51.100.1\r\n\
                      s=-\r\n\
                      c=IN IP4 198.51.100.1\r\n\
                      t=0 0\r\n\
                      m=audio 30000 RTP/AVP 9 101\r\n\
                      a=rtpmap:9 G722/8000\r\n\
                      a=rtpmap:101 telephone-event/8000\r\n\
                      a=sendrecv\r\n";
    let parsed = parse_sdp(g722_only, &ALL_CODECS).unwrap();
    assert_eq!(parsed.codec, AudioCodec::G722);
    assert_eq!(parsed.payload_type, 9);
}

#[test]
fn offer_with_srtp_uses_savp_and_carries_crypto() {
    let srtp = SrtpParams::generate();
    let sdp = build_offer("192.0.2.1", 40000, Some(&srtp), &ALL_CODECS, None, false);
    assert!(sdp.contains("RTP/SAVP"));
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert_eq!(parsed.codec, AudioCodec::Opus);
    assert_eq!(parsed.srtp, Some(srtp));
}

#[test]
fn srtp_crypto_line_roundtrip() {
    let params = SrtpParams::generate();
    let line = params.to_crypto_line(1);
    let parsed = SrtpParams::parse_crypto_line(&line).unwrap();
    assert_eq!(parsed, params);
}

#[test]
fn parse_falls_back_when_opus_unsupported() {
    // Remote offer without opus at all -- PCMA should win as it's first in the list.
    let sdp = "v=0\r\n\
               o=- 1 1 IN IP4 198.51.100.1\r\n\
               s=-\r\n\
               c=IN IP4 198.51.100.1\r\n\
               t=0 0\r\n\
               m=audio 30000 RTP/AVP 8 0 101\r\n\
               a=rtpmap:8 PCMA/8000\r\n\
               a=rtpmap:0 PCMU/8000\r\n\
               a=rtpmap:101 telephone-event/8000\r\n\
               a=sendrecv\r\n";
    let parsed = parse_sdp(sdp, &ALL_CODECS).unwrap();
    assert_eq!(parsed.codec, AudioCodec::Pcma);
    assert_eq!(parsed.payload_type, 8);
}

#[test]
fn hold_offer_is_sendonly() {
    let sdp = build_hold_offer("192.0.2.3", 40004, AudioCodec::Opus, None);
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert!(parsed.is_sendonly);
    assert_eq!(parsed.codec, AudioCodec::Opus);
}

#[test]
fn build_offer_honors_restricted_and_reordered_codec_list() {
    let codecs = [AudioCodec::Pcma, AudioCodec::Pcmu];
    let sdp = build_offer("192.0.2.1", 40000, None, &codecs, None, false);
    assert!(sdp.contains(&format!(
        "m=audio 40000 RTP/AVP {} {} 101",
        AudioCodec::Pcma.payload_type(),
        AudioCodec::Pcmu.payload_type()
    )));
    assert!(
        !sdp.contains("opus"),
        "Opus must not be offered when excluded from the codec list"
    );
    assert!(
        !sdp.contains("G722"),
        "G722 must not be offered when excluded from the codec list"
    );
    let parsed = parse_sdp(&sdp, &codecs).unwrap();
    assert_eq!(
        parsed.codec,
        AudioCodec::Pcma,
        "first entry in the configured list should win"
    );
}

#[test]
fn parse_sdp_skips_disabled_codec_in_remote_offer() {
    // Remote prefers Opus, but our account has Opus disabled -- PCMU
    // (also offered, lower in the remote's own preference) should be
    // picked instead of failing outright.
    let sdp = "v=0\r\n\
               o=- 1 1 IN IP4 198.51.100.1\r\n\
               s=-\r\n\
               c=IN IP4 198.51.100.1\r\n\
               t=0 0\r\n\
               m=audio 30000 RTP/AVP 111 0 101\r\n\
               a=rtpmap:111 opus/48000/2\r\n\
               a=rtpmap:0 PCMU/8000\r\n\
               a=rtpmap:101 telephone-event/8000\r\n\
               a=sendrecv\r\n";
    let allowed = [AudioCodec::Pcmu, AudioCodec::Pcma];
    let parsed = parse_sdp(sdp, &allowed).unwrap();
    assert_eq!(parsed.codec, AudioCodec::Pcmu);

    // Nothing mutually acceptable -- must return None, not guess.
    let allowed = [AudioCodec::Pcma];
    assert!(parse_sdp(sdp, &allowed).is_none());
}

#[test]
fn ice_attrs_round_trip_through_offer() {
    let ice = IceAttrs {
        ufrag: "abcd1234".into(),
        pwd: "s0mel0ngicepasswordvalue".into(),
        candidates: vec![
            "1 1 udp 2130706431 192.0.2.1 40000 typ host".into(),
            "2 1 udp 1694498815 203.0.113.5 40000 typ srflx raddr 192.0.2.1 rport 40000".into(),
        ],
    };
    let sdp = build_offer("192.0.2.1", 40000, None, &ALL_CODECS, Some(&ice), false);
    assert!(sdp.contains("a=ice-ufrag:abcd1234"));
    assert!(sdp.contains("a=ice-pwd:s0mel0ngicepasswordvalue"));
    assert!(sdp.contains("a=candidate:1 1 udp 2130706431 192.0.2.1 40000 typ host"));

    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert_eq!(parsed.ice_ufrag.as_deref(), Some("abcd1234"));
    assert_eq!(parsed.ice_pwd.as_deref(), Some("s0mel0ngicepasswordvalue"));
    assert_eq!(parsed.ice_candidates.len(), 2);
    assert_eq!(
        parsed.ice_candidates[0],
        "1 1 udp 2130706431 192.0.2.1 40000 typ host"
    );
}

#[test]
fn no_ice_attrs_leaves_ice_fields_empty() {
    let sdp = build_offer("192.0.2.1", 40000, None, &ALL_CODECS, None, false);
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert!(parsed.ice_ufrag.is_none());
    assert!(parsed.ice_pwd.is_none());
    assert!(parsed.ice_candidates.is_empty());
}

#[test]
fn vad_disabled_never_offers_comfort_noise() {
    let sdp = build_offer("192.0.2.1", 40000, None, &ALL_CODECS, None, false);
    assert!(!sdp.contains("CN/8000"));
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert!(parsed.cn_type.is_none());
}

#[test]
fn vad_enabled_offers_comfort_noise_alongside_narrowband_codecs() {
    let codecs = [AudioCodec::Pcmu, AudioCodec::Pcma];
    let sdp = build_offer("192.0.2.1", 40000, None, &codecs, None, true);
    assert!(sdp.contains(&format!("a=rtpmap:{CN_PAYLOAD_TYPE} CN/8000")));
    let parsed = parse_sdp(&sdp, &codecs).unwrap();
    assert_eq!(parsed.cn_type, Some(CN_PAYLOAD_TYPE));
}

#[test]
fn vad_enabled_answer_excludes_comfort_noise_for_opus() {
    // Opus's RTP clock (48000) doesn't match CN's static 8000 Hz assignment
    // -- see `CN_PAYLOAD_TYPE`'s doc comment -- so it must never be offered
    // alongside a negotiated Opus answer even with vad_enabled on.
    let sdp = build_answer("192.0.2.2", 40002, AudioCodec::Opus, None, None, true);
    assert!(!sdp.contains("CN/8000"));
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert!(parsed.cn_type.is_none());
}

#[test]
fn vad_enabled_answer_includes_comfort_noise_for_narrowband_codec() {
    let sdp = build_answer("192.0.2.2", 40002, AudioCodec::Pcmu, None, None, true);
    let parsed = parse_sdp(&sdp, &ALL_CODECS).unwrap();
    assert_eq!(parsed.cn_type, Some(CN_PAYLOAD_TYPE));
}
