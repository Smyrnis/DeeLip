# media-engine (`crates/media-engine`)

Owns everything downstream of SIP signaling for one call's actual media: capturing
and playing audio via `cpal`, encoding/decoding across seven audio codecs, RTP/SRTP
framing, jitter buffering, echo cancellation/AGC/VAD, call recording, and — as an
independent pipeline — H.264 video capture/encode/RTP/decode. `sip-core` negotiates
*what* codec, keys, and ports a call will use (see `docs/crates/sip-core.md`); this crate is
what actually moves the audio/video bytes once that negotiation is done.

## Architecture

Module map:
- `audio.rs` — cpal capture/playback streams, gain storage (`SharedGain`).
- `codec.rs` — one wrapper type per codec (Opus, G.722, G.729, GSM, iLBC, G.711
  µ-law/A-law), all exposing the same `encode(&[i16]) -> Vec<u8>` /
  `decode(&[u8]) -> Vec<i16>` shape regardless of the underlying library.
- `aec.rs` / `agc.rs` — acoustic echo cancellation and automatic gain control.
- `dtmf.rs` — RFC 2833 telephone-event encoding and inband dual-tone synthesis.
- `rtp.rs` — the RTP packet type and a stateful per-stream `RtpSender`.
- `recording.rs` — WAV/MP3 call-recording writers.
- `engine.rs` — `MediaEngine`, the orchestrator tying all of the above together
  into live send/recv tasks for one call (optionally two, for a 3-way conference).
- `video_capture.rs` / `video_codec.rs` / `video_rtp.rs` / `video_engine.rs` — an
  independent video pipeline (see its own section below).
- `zrtp_session.rs` — drives `sip-core`'s ZRTP protocol engine against a real RTP
  socket; see the dedicated section below.

**Audio data flow**: cpal's capture callback pushes 20ms PCM frames through a
channel into `MediaEngine`'s send task, which runs them through mute → echo
cancellation → AGC → user input-gain → VAD gate, then encodes and sends as
RTP/SRTP. The recv task does the mirror image: decrypt/decode off the wire (or
synthesize comfort noise for an RFC 3389 SID packet) into a shared jitter buffer
(`PlaybackTx`, an `Arc<Mutex<VecDeque<i16>>>`), which cpal's output callback drains
every callback tick, applying output gain on the way out.

**Conferencing**: a second RTP leg (`ConferenceLeg`) runs alongside leg 1 inside the
same `MediaEngine`, each with its own codec/encryption state; the send task encodes
captured audio for both legs independently, and decoded audio from both legs is
mixed (`mix_frames`) before local playback/recording.

**ZRTP**: `engine.rs`'s recv task owns a `ZrtpRuntime` (see the ZRTP section below)
and reacts to its outcomes — sending handshake bytes, swapping in fresh SRTP
contexts once the key agreement completes, and surfacing the SAS string.

**Video pipeline**: a completely separate `VideoEngine` (not part of `MediaEngine`)
with its own send/recv tasks: capture (or a synthetic frame source) → H.264 encode
→ RFC 6184 RTP fragmentation → SRTP → socket, and the reverse on receive.

## Design decisions & invariants

**Codec enum-dispatch** (`engine.rs`'s `AudioEncoder`/`AudioDecoder`): one value
replaces what used to be five separate `Option<XEncoder>` locals plus a 7-arm
`match` at every call site. G.722 and G.729's encoder/decoder structs are boxed
inside their enum variants — both are large lookup-table-heavy state machines, and
boxing keeps the enum's own stack size small regardless of which codec a call
actually negotiated.

**Codec implementations, one quirk each**:
- **G.722** operates natively at 16kHz, but this pipeline is fixed at 8kHz
  throughout (mic/speaker/jitter buffer/AEC/mixing/recording) — rather than thread
  a second sample rate through the whole engine, `G722Encoder`/`Decoder` resample at
  the codec boundary using `audio-codec`'s own stateful polyphase resampler (kept
  alive across calls, not reconstructed per-frame, so there's no discontinuity at
  each 20ms boundary). This buys SDP/RTP interop with phones/PBXes that require
  G.722 — it does not make DeeLip's own captured voice objectively clearer, since
  the source audio is 8kHz either way.
- **G.729** is native 8kHz (no resampling needed) via `audio-codec`'s pure-Rust
  `g729-sys` — not an FFI wrapper around the ITU reference C code.
- **GSM 06.10** has no usable pure-Rust crate (the one published, `oxideav-gsm`, has
  every version yanked), so `gsm-sys` vendors and compiles the classic reference
  implementation (Jutta Degener/Carsten Bormann, 1992–2009 — the same code
  Asterisk/FFmpeg/SoX have used for decades) from C source at build time. Its raw
  `extern "C"` binding needs an explicit `unsafe impl Send` for `GsmEncoder`/
  `GsmDecoder`: `gsm_sys::Gsm` is a raw pointer so it isn't `Send` by default, but
  each instance is exclusively owned (created in `new()`, freed in `Drop`, never
  shared across threads concurrently) and libgsm's per-instance state is entirely
  self-contained, so moving one into a spawned task is sound.
- **iLBC**'s 20ms mode (304 bits/38 bytes per frame) matches DeeLip's fixed 20ms RTP
  framing directly. `oxideav-ilbc` exposes a generic streaming encoder/decoder trait
  pair built for a broader multi-codec framework; `IlbcEncoder`/`Decoder` just hide
  that machinery behind the same simple per-frame shape every other codec here uses.
- **RTP clock/timestamp increment** (`ts_increment_for`/`clock_hz_for`): Opus's RTP
  clock is always signaled as 48000 Hz per RFC 7587 regardless of the audio's actual
  8kHz sample rate here; everything else runs at a matching 8000 Hz RTP clock.

**Echo cancellation** (`aec.rs`): bridges the mismatch between DeeLip's fixed
160-sample (20ms @ 8kHz) RTP framing and `fdaf-aec`'s power-of-two FFT size
requirement (256, i.e. 128-sample hops) — `LCM(160, 128) = 640`, so every 4 RTP
frames align exactly with 5 AEC hops with no long-run drift, just a small fixed
buffering delay. Runs non-realtime, inside `MediaEngine`'s send task rather than the
cpal capture callback, since it does FFT + allocation work per frame. The step size
(0.05) was tuned empirically: a standalone repeated-tone test showed values above
~0.1 causing a large transient energy blow-up (e.g. 0.5 spiked to ~150x the input
energy before eventually recovering) before converging; 0.05 converges smoothly with
no overshoot.

**AGC** (`agc.rs`): a simple feedback loop nudging a linear gain multiplier toward a
~-14 dBFS RMS target every frame, with a hard clip-safety ceiling (0.5x–8x) and a
silence floor (below which gain is left alone, so a pause in speech doesn't blow
gain up toward the ceiling dividing by a near-zero denominator).

**Call recording** (`recording.rs`): stereo WAV (lossless, via `hound`) or MP3
(lossy, via `mp3lame-encoder`), left channel = near-end mic, right = the
already-mixed far-end audio for that same frame. **A real SIGSEGV was found and
fixed here**: `Mp3Writer::write_frame`/`finalize` call `encode_to_vec`/
`flush_to_vec`, which write directly into the target `Vec<u8>`'s *spare capacity* —
an unreserved `Vec` has none, and the underlying LAME C encoder does not reliably
bound its writes to that (empty) capacity, corrupting the heap. Both call sites now
`reserve(max_required_buffer_size(...))` first (a no-op after the first call, since
`clear()` preserves capacity) — **do not remove these reserves**, they are the fix,
not an optimization.

**`MediaEngine::stop`'s abort-then-await shape**: async specifically so callers can
wait for the send/recv tasks to actually finish, not just be scheduled for
cancellation — `abort()` alone doesn't guarantee a task (and whatever it holds,
e.g. a TURN relay `Conn`) is really gone by the time `stop()` returns. This matters
because a caller can immediately reuse the *same* relay `Conn` in a brand new
engine (conference-merge does exactly this): if the old task's `recv_from` is still
alive even momentarily, it races the new engine's recv task for the same incoming
packets and can silently steal them, starving the new one. Awaiting here closes that
window. This is also why `recorder`'s finalize happens synchronously in `stop()`
itself rather than inside the (aborted) send task — abort cancellation would
otherwise race the WAV/MP3 finalize and could leave a truncated file.

**Stats are local-only** (`LegStats`/`CallStatsSnapshot`): there's no RTCP in this
codebase, so loss/jitter reflect what *we* observe receiving, not what the remote
reports observing from us — the usual scope for a softphone's stats panel without a
full RTCP implementation.

### Video pipeline

**Why `VideoEngine` is standalone, not part of `MediaEngine`**: `ConferenceLeg`
(`MediaEngine`'s second audio leg) looked like an obvious template at first, but its
send/recv tasks are wired into audio-only jitter-buffer/mixing machinery ("mix leg 1
and leg 2 into the same speaker") that video has no equivalent of — there's no "mix
two videos into one display." Only the socket/SRTP-context *setup* shape is
reusable (`RtpSocket`, shared with `engine.rs`).

**Video RTP timestamping** (`video_engine.rs`): H.264's RTP clock is always 90kHz
(RFC 6184) regardless of capture/encode frame rate. A single video frame usually
fragments into several RTP packets that must all share one timestamp, then jump the
clock once per frame — which doesn't fit `RtpSender::next_packet`'s "one call = one
packet = one timestamp step" model. The fix: construct the sender with
`ts_increment: 0` so repeated `next_packet` calls within one frame only advance
`sequence`, then bump `timestamp` manually, once, after the frame's last fragment.

**Camera capture** (`video_capture.rs`): `nokhwa::Camera::frame()` is a blocking
pull call, not tokio-async — the same shape `cpal` audio already has in this crate,
needing the same thread-based bridge as `audio.rs`. `nokhwa::Camera` itself isn't
`Send` (its internal backend trait object isn't), so it can't be constructed and
then moved into the spawned thread — it has to be created *on* that thread instead,
with a one-shot channel reporting the open/stream-start result back so
`start_capture` still gets to fail fast, synchronously, despite that.
`CaptureHandle` holds only the single freshest captured frame, not a queue: a raw
video frame is large enough that letting a slow consumer fall behind and
accumulate a backlog is worse for a live call than simply dropping stale frames.

**RTP fragmentation/reassembly** (`video_rtp.rs`): NAL-unit splitting relies on
encoder-side emulation prevention (which every spec-compliant H.264 encoder,
including `openh264`, applies) guaranteeing real NAL payload never contains a raw
start-code byte sequence — so a plain byte scan for `00 00 01` unambiguously finds
only real start codes, no bit-level parsing needed.

## ZRTP session driving (`zrtp_session.rs`)

`SqliteSecretStore` implements `sip-core`'s `SharedSecretStore` trait against the
same SQLite database the rest of DeeLip's config uses (schema owned by
`deelip_config::db`, which always creates the `zrtp_cache` table before any call
could reach this code) — opens its own connection rather than threading a live `Db`
handle from the `ui` crate into this RTP-loop task.

`ZrtpRuntime` wraps `sip-core`'s `ZrtpEngine` for one call's RTP socket:
retransmitting our own last-sent handshake message on packet loss (a flat
`RESEND_INTERVAL`-apart retry up to `MAX_ATTEMPTS`, not RFC 6189's own
exponential-backoff schedule — simpler, and this implementation's own tests are the
only thing that have ever exercised it), persisting retained secrets, and
translating engine events into what `engine.rs`'s RTP loop needs to act on: send
bytes, swap in fresh SRTP keys, or surface the SAS. See `docs/crates/sip-core.md` for the
ZRTP protocol/wire/crypto half (message format, hash-chain reveal sequence,
verification/provenance status).

## Known limitations / open items

- **Video has never been confirmed working against real camera hardware** — this
  development environment has no camera device at all (no `/dev/video*`, no
  `uvcvideo` kernel module), so while `nokhwa`'s device-enumeration/open/capture
  calls are structurally complete, only the pixel-format conversion
  (`rgb8_to_yuv420`) has been verified end-to-end here, using synthetic RGB buffers.
- **No RTP reordering/jitter-buffering on the video recv side** — fragments are
  reassembled in arrival order only; real out-of-order delivery would corrupt a
  frame until the next keyframe. Acceptable for proving the pipeline works, worth
  revisiting before this is real-world-facing (tracked in the current
  `ARCHITECTURE_GAPS.md`'s video phase).
- **Conferencing stays audio-only** — merging two calls drops any video leg either
  one had (see `ARCHITECTURE_GAPS.md`'s video phase for the two design options being
  weighed to change this).
- **MP3 recording's buffer-reservation requirement** (see above) is a correctness
  invariant, not a style choice — regressing it reintroduces a real SIGSEGV.
