use super::*;

#[test]
fn sanitize_filename_replaces_unsafe_chars() {
    assert_eq!(sanitize_filename("abc123._-"), "abc123._-");
    assert_eq!(sanitize_filename("call@id:1/2"), "call_id_1_2");
}

#[test]
fn recording_options_default_is_disabled_wav() {
    let opts = RecordingOptions::default();
    assert!(!opts.enabled);
    assert_eq!(opts.format, RecordingFormat::Wav);
    assert!(opts.dir_override.is_none());
}

fn temp_recordings_dir(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("deelip-recording-test-{name}-{}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn wav_round_trip_writes_readable_interleaved_samples() {
    let dir = temp_recordings_dir("wav");
    let mut writer = RecordingWriter::create("call-1", Some(&dir), RecordingFormat::Wav).unwrap();
    writer.write_frame(&[1, 2, 3], &[10, 20, 30]).unwrap();
    writer.write_frame(&[4, 5], &[40, 50]).unwrap();
    writer.finalize().unwrap();

    let entry = std::fs::read_dir(&dir).unwrap().next().expect("one recording file").unwrap();
    let mut reader = hound::WavReader::open(entry.path()).unwrap();
    assert_eq!(reader.spec().channels, 2);
    let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
    assert_eq!(samples, vec![1, 10, 2, 20, 3, 30, 4, 40, 5, 50]);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wav_write_frame_pads_missing_far_end_samples_with_zero() {
    let dir = temp_recordings_dir("wav-short-far");
    let mut writer = RecordingWriter::create("call-2", Some(&dir), RecordingFormat::Wav).unwrap();
    writer.write_frame(&[7, 8, 9], &[70]).unwrap();
    writer.finalize().unwrap();

    let entry = std::fs::read_dir(&dir).unwrap().next().expect("one recording file").unwrap();
    let mut reader = hound::WavReader::open(entry.path()).unwrap();
    let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
    assert_eq!(samples, vec![7, 70, 8, 0, 9, 0]);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mp3_round_trip_produces_a_non_empty_file() {
    let dir = temp_recordings_dir("mp3");
    let mut writer = RecordingWriter::create("call-3", Some(&dir), RecordingFormat::Mp3).unwrap();
    for _ in 0..5 {
        writer.write_frame(&[100i16; 160], &[50i16; 160]).unwrap();
    }
    writer.finalize().unwrap();

    let entry = std::fs::read_dir(&dir).unwrap().next().expect("one recording file").unwrap();
    assert!(entry.metadata().unwrap().len() > 0);

    std::fs::remove_dir_all(&dir).ok();
}
