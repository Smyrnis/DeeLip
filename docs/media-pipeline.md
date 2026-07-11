# Media pipeline: VAD/comfort noise and the video engine

Sources: `crates/media-engine/src/{vad,video_capture,video_engine}.rs`.

## Voice activity detection / comfort noise (`vad.rs`)

Energy-threshold voice activity detection driving RFC 3389 comfort-noise
generation: during a detected silence, the send task stops encoding and
sending continuous audio and instead sends an occasional Comfort Noise/SID
packet carrying a noise-level byte -- the same non-realtime placement as the
AGC/AEC stages it sits alongside in `MediaEngine`'s send task.

Simplification worth being explicit about: this only sends a fresh SID
packet at silence onset and then periodically (`CN_REPEAT_FRAMES`) while
silence continues -- it does not attempt to synthesize *continuous* comfort
noise on the receive side spanning the gaps between SID packets (see
`generate_comfort_noise` in `vad.rs` and its call site in `engine.rs`),
which would need a background filler tied to the jitter buffer's own
playout clock rather than packet arrival. Between SID updates the receive
side falls back to plain silence (the jitter buffer's existing
zero-pad-on-underrun behavior), not a continuous background hiss.

## Video capture (`video_capture.rs`)

Camera capture + RGB→I420 conversion, feeding `video_codec::Yuv420Frame`
(which `H264Encoder` already consumes). Not wired into any live call path
yet -- video negotiation is a later phase (see `video_engine.rs` below).

**Not live-verified against real camera hardware**: this development
environment has no camera device at all (no `/dev/video*`, no `uvcvideo`
kernel module), so while `nokhwa`'s device-enumeration/open/capture calls
are real and structurally complete, only the pixel-format conversion
(`rgb8_to_yuv420`) could be verified end-to-end here, using synthetic RGB
buffers instead of real sensor output. Treat the capture loop itself as
unverified until run somewhere with actual camera hardware.

`start_capture` opens the camera and starts capturing on a dedicated OS
thread -- `nokhwa`'s `Camera::frame()` is a blocking pull call, not
tokio-async, the same non-async-capture shape `cpal` audio already has in
this crate, needing the same thread-based bridge as `audio.rs`. `nokhwa::Camera`
is not `Send` (its internal backend trait object isn't), so it can't be
constructed and then moved into the spawned thread -- it has to be created
*on* that thread instead, with a one-shot channel reporting the open/
stream-start result back so the function still gets to "fail fast,
synchronously" despite that.

## Standalone video RTP engine (`video_engine.rs`)

Capture-frame → H.264 encode → RTP send, and RTP recv → H.264 decode →
latest-decoded-frame -- its own independent construct, deliberately *not*
part of `MediaEngine`.

**Why standalone**: `MediaEngine`'s `ConferenceLeg` (a second RTP leg for
3-way audio conferencing) looked like an obvious template at first, but its
send/recv tasks are wired into audio-only jitter-buffer/mixing machinery
("mix leg 1 and leg 2 into the same speaker") that a video leg has no
equivalent of -- there's no "mix two videos into one display." Only
`ConferenceLeg`'s socket/SRTP-context *setup* shape is reusable, which this
module borrows (see `RtpSocket`, shared with `engine.rs`).

Not yet wired into any live call (`MediaEngine`/`ui`) -- that's a future
phase, once this piece is proven correct in isolation (see this module's
own tests: a real two-instance UDP round trip using synthetic frames, since
this development environment has no camera hardware to test real capture
with).

**Disclosed simplification**: the recv side does no RTP
reordering/jitter-buffering -- fragments are reassembled in arrival order
only. Real out-of-order delivery would corrupt a frame until the next
keyframe. Acceptable for proving the pipeline works; worth revisiting before
this is real-world-facing.
