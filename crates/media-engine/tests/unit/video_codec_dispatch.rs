use super::*;

/// Same shape of dispatch-wiring test as `codec_dispatch.rs`'s audio
/// equivalent: `VideoEncoder`/`VideoDecoder` construction must produce the
/// matching enum variant, and `.encode()`/`.decode()` must actually route
/// through it. There's only one `VideoCodec` variant today (`H264`), but
/// this is exactly the seam a second one (e.g. VP8, per this module's own
/// doc comment) would need to slot into correctly, and a mismatched match
/// arm there would silently misroute every video frame.
#[test]
fn video_encoder_new_produces_the_matching_variant() {
    let enc = VideoEncoder::new(VideoCodec::H264, 500_000).unwrap();
    match enc {
        VideoEncoder::H264(_) => {}
    }
}

#[test]
fn video_decoder_new_produces_the_matching_variant() {
    let dec = VideoDecoder::new(VideoCodec::H264).unwrap();
    match dec {
        VideoDecoder::H264(_) => {}
    }
}

#[test]
fn encode_then_decode_round_trips_through_dispatch() {
    let mut enc = VideoEncoder::new(VideoCodec::H264, 500_000).unwrap();
    let mut dec = VideoDecoder::new(VideoCodec::H264).unwrap();

    // Real decoders commonly buffer/reorder internally (see
    // `video_codec.rs`'s own tests) -- feed a short sequence and accept a
    // frame appearing anywhere in it.
    let mut got_frame = false;
    for _ in 0..3 {
        let frame = Yuv420Frame::solid_color(64, 64, 128, 128, 128);
        let bitstream = enc.encode(&frame).unwrap();
        assert!(!bitstream.is_empty(), "encoder must produce a non-empty NAL bitstream");
        if let Some(decoded) = dec.decode(&bitstream).unwrap() {
            assert_eq!(decoded.width, 64);
            assert_eq!(decoded.height, 64);
            got_frame = true;
        }
    }
    assert!(got_frame, "decoder should emit at least one frame across a short sequence");
}
