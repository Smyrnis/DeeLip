use super::*;

/// Every `AudioCodec` variant, so the table tests below can't silently skip
/// one if a new variant is ever added without updating this list (the
/// per-variant tests use an explicit match instead of `matches!` so the
/// compiler flags missing arms too).
const ALL_CODECS: &[AudioCodec] = &[
    AudioCodec::Opus,
    AudioCodec::G722,
    AudioCodec::Gsm,
    AudioCodec::Ilbc,
    AudioCodec::G729,
    AudioCodec::Pcma,
    AudioCodec::Pcmu,
    AudioCodec::L16,
];

#[test]
fn ts_increment_for_matches_each_codec_rtp_clock() {
    for &codec in ALL_CODECS {
        let expected = if codec == AudioCodec::Opus { 960 } else { 160 };
        assert_eq!(ts_increment_for(codec), expected, "wrong ts increment for {codec:?}");
    }
}

#[test]
fn clock_hz_for_matches_each_codec_rtp_clock() {
    for &codec in ALL_CODECS {
        let expected = if codec == AudioCodec::Opus { 48000.0 } else { 8000.0 };
        assert_eq!(clock_hz_for(codec), expected, "wrong clock rate for {codec:?}");
    }
}

/// The construction+dispatch wiring itself: `AudioEncoder::new`/
/// `AudioDecoder::new` must produce the matching enum variant, not just
/// *a* variant that happens to also encode/decode without panicking. This
/// is exactly the class of bug this session already found once for real
/// (a codec silently unreachable via its string form) -- a mismatched
/// match arm here would misroute every encode/decode call for that codec
/// to a different codec's implementation while still "working" in the
/// sense of not crashing.
#[test]
fn audio_encoder_new_produces_the_matching_variant() {
    for &codec in ALL_CODECS {
        let enc = AudioEncoder::new(codec, "").unwrap();
        let got = match &enc {
            AudioEncoder::Opus(_) => AudioCodec::Opus,
            AudioEncoder::G722(_) => AudioCodec::G722,
            AudioEncoder::Gsm(_) => AudioCodec::Gsm,
            AudioEncoder::Ilbc(_) => AudioCodec::Ilbc,
            AudioEncoder::G729(_) => AudioCodec::G729,
            AudioEncoder::Pcma => AudioCodec::Pcma,
            AudioEncoder::Pcmu => AudioCodec::Pcmu,
            AudioEncoder::L16 => AudioCodec::L16,
        };
        assert_eq!(got, codec, "AudioEncoder::new({codec:?}) produced the wrong variant");
    }
}

#[test]
fn audio_decoder_new_produces_the_matching_variant() {
    for &codec in ALL_CODECS {
        let dec = AudioDecoder::new(codec, "").unwrap();
        let got = match &dec {
            AudioDecoder::Opus(_) => AudioCodec::Opus,
            AudioDecoder::G722(_) => AudioCodec::G722,
            AudioDecoder::Gsm(_) => AudioCodec::Gsm,
            AudioDecoder::Ilbc(_) => AudioCodec::Ilbc,
            AudioDecoder::G729(_) => AudioCodec::G729,
            AudioDecoder::Pcma => AudioCodec::Pcma,
            AudioDecoder::Pcmu => AudioCodec::Pcmu,
            AudioDecoder::L16 => AudioCodec::L16,
        };
        assert_eq!(got, codec, "AudioDecoder::new({codec:?}) produced the wrong variant");
    }
}

/// Confirms `.encode()`/`.decode()` actually route through the constructed
/// variant end to end (not just that construction picked the right enum
/// tag) -- a round trip through each codec's real dispatch path with a
/// non-trivial frame.
#[test]
fn encode_then_decode_round_trips_through_dispatch_for_every_codec() {
    let frame: Vec<i16> = (0..crate::audio::FRAME_SAMPLES).map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16).collect();
    for &codec in ALL_CODECS {
        let mut enc = AudioEncoder::new(codec, "").unwrap();
        let mut dec = AudioDecoder::new(codec, "").unwrap();
        let encoded = enc.encode(&frame);
        assert!(!encoded.is_empty(), "{codec:?} encoder produced an empty packet");
        let decoded = dec.decode(&encoded);
        assert_eq!(decoded.len(), crate::audio::FRAME_SAMPLES, "{codec:?} decoder returned the wrong sample count");
    }
}
