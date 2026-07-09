use super::*;
use crate::video_codec::H264Encoder;
use nokhwa::utils::CameraIndex;

#[test]
fn list_cameras_does_not_panic_with_no_hardware() {
    // This development sandbox has no camera device at all (no
    // `/dev/video*`, no `uvcvideo` kernel module) -- the meaningful
    // assertion here is that enumeration degrades to an empty list
    // instead of panicking, not that it finds anything.
    let cameras = list_cameras();
    assert!(cameras.is_empty(), "no camera hardware exists in this sandbox");
}

fn solid_rgb(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(width as usize * height as usize * 3);
    for _ in 0..(width * height) {
        buf.extend_from_slice(&[r, g, b]);
    }
    buf
}

#[test]
fn rgb8_to_yuv420_produces_correct_dimensions() {
    let rgb = solid_rgb(64, 48, 128, 128, 128);
    let frame = rgb8_to_yuv420(&rgb, 64, 48).unwrap();
    assert_eq!(frame.width, 64);
    assert_eq!(frame.height, 48);
    assert_eq!(frame.y.len(), 64 * 48);
    assert_eq!(frame.u.len(), (64 / 2) * (48 / 2));
    assert_eq!(frame.v.len(), (64 / 2) * (48 / 2));
}

#[test]
fn rgb8_to_yuv420_known_color_lands_in_expected_range() {
    // Pure red: high luma-ish Y, but chroma clearly below neutral (128) on
    // U and above neutral on V (BT.601-family YCbCr).
    let red = solid_rgb(32, 32, 255, 0, 0);
    let red_frame = rgb8_to_yuv420(&red, 32, 32).unwrap();
    assert!(red_frame.u[0] < 128, "red should have below-neutral U (Cb)");
    assert!(red_frame.v[0] > 128, "red should have above-neutral V (Cr)");

    // Pure blue: below-neutral V, above-neutral U, and noticeably darker
    // luma than red (blue contributes least to perceptual luma).
    let blue = solid_rgb(32, 32, 0, 0, 255);
    let blue_frame = rgb8_to_yuv420(&blue, 32, 32).unwrap();
    assert!(blue_frame.u[0] > 128, "blue should have above-neutral U (Cb)");
    assert!(blue_frame.v[0] < 128, "blue should have below-neutral V (Cr)");
    assert!(
        blue_frame.y[0] < red_frame.y[0],
        "blue's luma contribution is lower than red's"
    );
}

#[test]
fn rgb8_to_yuv420_rejects_malformed_input() {
    let too_short = vec![0u8; 10];
    assert!(rgb8_to_yuv420(&too_short, 64, 48).is_err());
    let odd_dims = solid_rgb(63, 48, 0, 0, 0);
    assert!(rgb8_to_yuv420(&odd_dims, 63, 48).is_err());
}

#[test]
fn start_capture_on_nonexistent_camera_returns_a_clean_error() {
    // This sandbox has zero camera devices -- index 0 deterministically
    // doesn't exist, so this is a real (not hypothetical) negative-path check.
    let result = start_capture(CameraIndex::Index(0), 640, 480, 30);
    assert!(result.is_err(), "opening a camera with no hardware present must fail cleanly, not panic or hang");
}

#[test]
fn converted_frame_is_accepted_by_the_h264_encoder() {
    let rgb = solid_rgb(64, 64, 100, 150, 200);
    let frame = rgb8_to_yuv420(&rgb, 64, 64).unwrap();
    let mut encoder = H264Encoder::new(500_000).unwrap();
    let bitstream = encoder.encode(&frame).unwrap();
    assert!(
        !bitstream.is_empty(),
        "a frame produced by rgb8_to_yuv420 must be genuinely encodable, not just internally self-consistent"
    );
}
