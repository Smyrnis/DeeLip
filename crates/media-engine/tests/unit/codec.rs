use super::*;

fn ulaw_error_pct(original: i16, decoded: i16) -> f32 {
    let err = (original as i32 - decoded as i32).abs() as f32;
    let mag = original.unsigned_abs() as f32;
    if mag < 1.0 {
        err
    } else {
        err / mag * 100.0
    }
}

#[test]
fn ulaw_roundtrip() {
    for &sample in &[0i16, 100, 1000, 10000, -100, -1000, -10000] {
        let decoded = ulaw_to_pcm(pcm_to_ulaw(sample));
        let err_pct = ulaw_error_pct(sample, decoded);
        assert!(
            err_pct < 5.0,
            "μ-law roundtrip: sample={sample}, decoded={decoded}, err={err_pct:.1}%"
        );
    }
    // At full scale, clipping adds error; up to 2% is within G.711 spec
    let clip_decoded = ulaw_to_pcm(pcm_to_ulaw(i16::MAX));
    assert!((i16::MAX as i32 - clip_decoded as i32).abs() < 1000);
}

#[test]
fn alaw_roundtrip() {
    for &sample in &[0i16, 100, 1000, 10000, -100, -1000, -10000] {
        let decoded = alaw_to_pcm(pcm_to_alaw(sample));
        let err = (sample as i32 - decoded as i32).abs();
        let mag = sample.unsigned_abs() as i32;
        let err_pct = if mag > 0 { err * 100 / mag } else { err };
        assert!(
            err_pct < 10,
            "A-law roundtrip: sample={sample}, decoded={decoded}, err={err}"
        );
    }
}

#[test]
fn ulaw_known_values() {
    // μ-law silence (0) encodes to 0xFF
    assert_eq!(pcm_to_ulaw(0), 0xFF);
}

#[test]
fn opus_roundtrip() {
    let mut encoder = OpusEncoder::new().unwrap();
    let mut decoder = OpusDecoder::new().unwrap();

    // One 20ms frame (160 samples @ 8kHz) of a synthetic tone.
    let frame: Vec<i16> = (0..crate::audio::FRAME_SAMPLES)
        .map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16)
        .collect();

    let encoded = encoder.encode(&frame);
    assert!(
        !encoded.is_empty(),
        "Opus should produce a non-empty packet"
    );
    assert!(encoded.len() <= OPUS_MAX_PACKET);

    let decoded = decoder.decode(&encoded);
    assert_eq!(decoded.len(), crate::audio::FRAME_SAMPLES);
}

#[test]
fn g722_roundtrip() {
    let mut encoder = G722Encoder::new();
    let mut decoder = G722Decoder::new();

    // One 20ms frame (160 samples @ 8kHz) of a synthetic tone.
    let frame: Vec<i16> = (0..crate::audio::FRAME_SAMPLES)
        .map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16)
        .collect();

    let encoded = encoder.encode(&frame);
    assert!(
        !encoded.is_empty(),
        "G722 should produce a non-empty packet"
    );

    let decoded = decoder.decode(&encoded);
    assert!(
        !decoded.is_empty(),
        "G722 should decode back to a non-empty PCM frame"
    );
    // The 8k->16k->8k resample round trip isn't guaranteed to preserve
    // the exact sample count frame-for-frame (polyphase filter delay) --
    // just stay in the right ballpark of the original frame size.
    let expected = crate::audio::FRAME_SAMPLES;
    assert!(
        decoded.len() > expected / 2 && decoded.len() < expected * 2,
        "decoded length {} far from expected ~{expected}",
        decoded.len(),
    );
}

fn test_tone() -> Vec<i16> {
    (0..crate::audio::FRAME_SAMPLES)
        .map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16)
        .collect()
}

#[test]
fn gsm_roundtrip() {
    let mut encoder = GsmEncoder::new();
    let mut decoder = GsmDecoder::new();

    let encoded = encoder.encode(&test_tone());
    assert_eq!(
        encoded.len(),
        33,
        "GSM full-rate frames are always 33 bytes"
    );

    let decoded = decoder.decode(&encoded);
    assert_eq!(decoded.len(), crate::audio::FRAME_SAMPLES);
}

#[test]
fn gsm_decode_rejects_wrong_length() {
    let mut decoder = GsmDecoder::new();
    assert!(decoder.decode(&[0u8; 10]).is_empty());
}

#[test]
fn ilbc_roundtrip() {
    let mut encoder = IlbcEncoder::new().unwrap();
    let mut decoder = IlbcDecoder::new().unwrap();

    let encoded = encoder.encode(&test_tone());
    assert_eq!(encoded.len(), 38, "iLBC 20ms frames are always 38 bytes");

    let decoded = decoder.decode(&encoded);
    assert_eq!(decoded.len(), crate::audio::FRAME_SAMPLES);
}
