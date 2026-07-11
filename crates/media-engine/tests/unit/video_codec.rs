use super::*;

#[test]
fn encode_decode_round_trip_dimensions_match() {
    let mut encoder = H264Encoder::new(500_000).unwrap();
    let mut decoder = H264Decoder::new().unwrap();

    // Real decoders commonly buffer/reorder internally, so a single
    // encode+decode call isn't guaranteed to emit a picture immediately --
    // feed a short sequence and accept a frame appearing anywhere in it,
    // matching how a real streaming decode loop would be driven.
    let mut got_frame = false;
    for _ in 0..3 {
        let frame = Yuv420Frame::solid_color(640, 480, 128, 128, 128);
        let bitstream = encoder.encode(&frame).unwrap();
        assert!(!bitstream.is_empty(), "encoder must produce a non-empty NAL bitstream");
        if let Some(decoded) = decoder.decode(&bitstream).unwrap() {
            assert_eq!(decoded.width, 640);
            assert_eq!(decoded.height, 480);
            got_frame = true;
        }
    }
    assert!(got_frame, "decoder should emit at least one frame across a short sequence");
}

#[test]
fn decoder_keeps_working_across_multiple_frames() {
    let mut encoder = H264Encoder::new(300_000).unwrap();
    let mut decoder = H264Decoder::new().unwrap();

    let mut decoded_count = 0;
    for i in 0..10u8 {
        // Vary the luma slightly per frame so this isn't just re-encoding
        // an identical picture ten times over.
        let frame = Yuv420Frame::solid_color(64, 64, 100u8.wrapping_add(i * 10), 128, 128);
        let bitstream = encoder.encode(&frame).unwrap();
        if let Some(decoded) = decoder.decode(&bitstream).unwrap() {
            assert_eq!(decoded.width, 64);
            assert_eq!(decoded.height, 64);
            decoded_count += 1;
        }
    }
    assert!(decoded_count > 0, "decoder should emit frames across a 10-frame sequence");
}
