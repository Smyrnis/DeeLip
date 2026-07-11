//! Camera capture + RGB→I420 conversion, feeding `video_codec::Yuv420Frame`
//! into `video_engine::VideoEngine` for a live call. Hardware-verification
//! status and the capture-thread design: `docs/crates/media-engine.md`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
};
use nokhwa::Camera;
use openh264::formats::{RgbSliceU8, YUVBuffer, YUVSource};

use crate::video_codec::Yuv420Frame;

/// Enumerate available cameras as `(index, human-readable name)` pairs, for
/// a future Settings camera picker. "No cameras found" (including any
/// enumeration error -- e.g. no backend available at all) is an expected,
/// unremarkable state, not a hard error: logged and reported as an empty
/// list rather than propagated.
pub fn list_cameras() -> Vec<(CameraIndex, String)> {
    match nokhwa::query(ApiBackend::Auto) {
        Ok(infos) => infos.into_iter().map(|info| (info.index().clone(), info.human_name())).collect(),
        Err(e) => {
            tracing::debug!("Camera enumeration unavailable: {e}");
            Vec::new()
        }
    }
}

/// Resolve a persisted camera name (as `list_cameras()` returns and
/// Settings stores) back to a `CameraIndex` -- mirrors `audio.rs`'s
/// find-cpal-device-by-name idiom. `None` if no currently enumerable
/// camera has that name (unplugged, or enumeration itself unavailable).
pub fn find_camera_by_name(name: &str) -> Option<CameraIndex> {
    list_cameras().into_iter().find(|(_, n)| n == name).map(|(idx, _)| idx)
}

/// Convert a packed RGB8 buffer (as `nokhwa`'s `Buffer::decode_image::<RgbFormat>()`
/// produces) into I420 -- reuses `openh264`'s own RGB→YUV420 converter
/// (`RgbSliceU8`/`YUVBuffer::from_rgb8_source`, the same color-space
/// conversion + 2x2 chroma subsampling it already needs internally for its
/// own still-image-adjacent helpers) rather than hand-rolling the math.
pub fn rgb8_to_yuv420(rgb: &[u8], width: u32, height: u32) -> anyhow::Result<Yuv420Frame> {
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        anyhow::bail!("rgb8_to_yuv420: width/height must be non-zero and even, got {width}x{height}");
    }
    let expected_len = width as usize * height as usize * 3;
    if rgb.len() != expected_len {
        anyhow::bail!(
            "rgb8_to_yuv420: buffer length {} doesn't match {width}x{height} RGB8 ({expected_len} expected)",
            rgb.len()
        );
    }

    let source = RgbSliceU8::new(rgb, (width as usize, height as usize));
    let yuv = YUVBuffer::from_rgb8_source(source);
    Ok(Yuv420Frame { width, height, y: yuv.y().to_vec(), u: yuv.u().to_vec(), v: yuv.v().to_vec() })
}

/// Handle to a running capture thread -- holds only the freshest captured
/// frame, not a queue (see `docs/crates/media-engine.md` for why).
pub struct CaptureHandle {
    latest_frame: Arc<Mutex<Option<Yuv420Frame>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle {
    /// The most recently captured frame, if any -- `None` until the first
    /// frame arrives, then always the latest one (never a backlog).
    pub fn latest_frame(&self) -> Option<Yuv420Frame> {
        self.latest_frame.lock().unwrap().clone()
    }

    /// The same "latest frame" slot `latest_frame()` reads, as a cheap
    /// `Arc` clone -- lets `video_engine::VideoEngine::start` poll this
    /// capture thread's output directly as its frame source, without
    /// needing to know `CaptureHandle` exists at all (it's generic over
    /// any `Arc<Mutex<Option<Yuv420Frame>>>`, which is also what a
    /// synthetic-frame test source looks like).
    pub fn frame_slot(&self) -> Arc<Mutex<Option<Yuv420Frame>>> {
        self.latest_frame.clone()
    }

    /// Signal the capture thread to stop. Does not block; call `join` (via
    /// `Drop`, which happens automatically) if you need to wait for the
    /// camera to actually be released.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Open `index` and start capturing on a dedicated OS thread. Fails fast
/// with a real error if the camera can't be opened -- the expected outcome
/// whenever no camera is plugged in/available, which is unconditionally
/// true in this development sandbox. See `docs/crates/media-engine.md` for why
/// this needs a thread (not tokio-async) and a one-shot readiness channel.
pub fn start_capture(index: CameraIndex, width: u32, height: u32, fps: u32) -> anyhow::Result<CaptureHandle> {
    let latest_frame = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();

    let thread_latest = latest_frame.clone();
    let thread_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
            Resolution::new(width, height),
            FrameFormat::MJPEG,
            fps,
        )));
        let mut camera = match Camera::new(index, requested) {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow::anyhow!("Opening camera: {e}")));
                return;
            }
        };
        if let Err(e) = camera.open_stream() {
            let _ = ready_tx.send(Err(anyhow::anyhow!("Starting camera stream: {e}")));
            return;
        }
        let _ = ready_tx.send(Ok(()));

        while !thread_stop.load(Ordering::Relaxed) {
            let buffer = match camera.frame() {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Camera frame capture failed: {e}");
                    continue;
                }
            };
            let resolution = buffer.resolution();
            let rgb = match buffer.decode_image::<RgbFormat>() {
                Ok(img) => img.into_raw(),
                Err(e) => {
                    tracing::warn!("Camera frame decode failed: {e}");
                    continue;
                }
            };
            match rgb8_to_yuv420(&rgb, resolution.width_x, resolution.height_y) {
                Ok(frame) => *thread_latest.lock().unwrap() = Some(frame),
                Err(e) => tracing::warn!("RGB->I420 conversion failed: {e}"),
            }
        }
        let _ = camera.stop_stream();
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(CaptureHandle { latest_frame, stop, thread: Some(thread) }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(anyhow::anyhow!("Camera capture thread exited unexpectedly during startup"))
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/video_capture.rs"]
mod tests;
