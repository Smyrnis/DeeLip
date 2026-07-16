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
- `engine.rs` — `MediaEngine`, the orchestrator wiring up one call's live send/recv
  tasks (optionally two, for a 3-way conference); the tasks themselves live in
  `tasks.rs` (see below).
- `tasks.rs` — the actual `recv_loop`/`send_loop` bodies (plus their shared helpers
  `drain_leg`/`mix_frames`/`write_recording`), split out of `engine.rs` purely for
  file size, same precedent as `codec_dispatch.rs` below — not a behavior change.
- `codec_dispatch.rs` / `video_codec_dispatch.rs` — per-codec `AudioEncoder`/
  `AudioDecoder` and `VideoEncoder`/`VideoDecoder` enum-dispatch, split out of
  `engine.rs` for the same file-size reason.
- `video_capture.rs` / `video_codec.rs` / `video_rtp.rs` / `video_engine.rs` — an
  independent video pipeline (see its own section below).
- `zrtp_session.rs` — drives `sip-core`'s ZRTP protocol engine against a real RTP
  socket; see the dedicated section below.
- `dtls_demux.rs` / `dtls_srtp_session.rs` — DTLS-SRTP (RFC 5763/5764) handshake and
  SRTP key export, driven against the same shared RTP socket; see the dedicated
  section below.

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

**Capture channel backpressure** (`audio.rs`'s `CaptureRx`/`CAPTURE_QUEUE_FRAMES`):
bounded, unlike the DTMF/ZRTP channels elsewhere in this crate (those stay
unbounded since they're fed at human/protocol-handshake rates that can never
realistically fill one) -- this one is fed by the realtime capture callback
every 20ms regardless of whether the consumer (the send task, which can stall
on a congested/blocked network `send_to`) is keeping up. `CAPTURE_QUEUE_FRAMES`
(50) matches the jitter/playback buffers' own 1s cap elsewhere in this crate.
The realtime callback uses `try_send`, so a full queue just drops the newest
frame (an audio glitch under sustained congestion) instead of growing without
bound or blocking the realtime thread.

**Realtime capture callback, allocation-free per frame** (`audio.rs`'s
`build_input_i16`/`build_input_f32`): once a 160-sample frame fills, the full
frame is moved into the channel via `mem::replace(&mut buf, Vec::with_capacity(FRAME_SAMPLES))`
rather than `buf.clone()` + `buf.clear()` -- the replace leaves a fresh
pre-allocated `Vec` in `buf` for the next frame with no copy, where the
clone+clear pair did a real allocation+memcpy on this realtime audio thread.

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

**Manual record toggle** (`MediaEngine::set_recording`): independent of whatever
`RecordingOptions::enabled` the engine was started with. Turning on lazily opens
a fresh `RecordingWriter`, so a manually-started recording only captures audio
from that point forward, not the part of the call already missed. Turning off
finalizes and drops the writer immediately rather than waiting for `stop()`,
matching the "record only what was asked for" intent. A failure to open the
file is logged and simply leaves recording off, the same handling as the
auto-record path at `start()`.

**`MediaEngine::stop`'s abort-then-await shape**: async specifically so callers can
wait for the send/recv tasks to actually finish, not just be scheduled for
cancellation — `abort()` alone doesn't guarantee a task (and whatever it holds,
e.g. a TURN relay `Conn`) is really gone by the time `stop()` returns. This matters
because a caller can immediately reuse the *same* relay `Conn` in a brand new
engine (conference-merge does exactly this): if the old task's `recv_from` is still
alive even momentarily, it races the new engine's recv task for the same incoming
packets and can silently steal them, starving the new one. Awaiting here closes that
window. This is also why `recorder`'s finalize happens in `stop()` itself
rather than inside the (aborted) send task — abort cancellation would
otherwise race the WAV/MP3 finalize and could leave a truncated file. Taking
the writer here (not inside the send task) makes finalization deterministic
regardless of the abort race, but the finalize() call itself is blocking disk
I/O, and `stop()` is commonly awaited via `rt.block_on` directly on the
UI/render thread (hangup/hold/swap) — so the finalize is dispatched onto
`spawn_blocking` rather than run inline, keeping this UI-thread-visible
`stop()` fast even on a slow or antivirus-intercepted disk. Recording is
already best-effort (see `AppConfig::recording_enabled`'s doc comment), so a
finalize that completes a moment after `stop()` itself returns is an
acceptable tradeoff — unlike the task-await above, which must stay
synchronous.

**Stats are local-only** (`LegStats`/`CallStatsSnapshot`): there's no RTCP in this
codebase, so loss/jitter reflect what *we* observe receiving, not what the remote
reports observing from us — the usual scope for a softphone's stats panel without a
full RTCP implementation.

**`tasks::encrypt_and_send`**: collapses the encrypt-log-send-count sequence
that used to be copy-pasted 5 times across the send task into one helper.
Borrows `ctx.encrypt_rtp`'s output (a cheap refcounted `bytes::Bytes`) rather
than `.to_vec()`-ing it, so this is also one fewer allocation+copy per
encrypted packet than the code it replaces. One deliberate, minor behavior
change from sharing this one path: DTMF packets now update `stats` on a
successful send too (previously only voice/comfort-noise packets did) — DTMF
is real data on the wire, and counting it makes the leg stats more accurate,
not less, so this isn't an oversight to "fix" back.

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

**`VideoEngine::start`'s encoder/decoder construction** (leg 1 and, if present,
leg 2): built once, up front, before either send/recv task spawns, and moved
in — mirroring `MediaEngine::start`'s own audio encoder/decoder construction.
Unlike the code this replaced, which built its own encoder/decoder from
inside `send_loop`/`recv_loop` on first entry and warned-and-returned on
failure, a construction failure now fails `start()` itself synchronously via
`?`, matching how audio already behaves — a deliberate behavior change, not
just a construction-timing rename.

**RTP fragmentation/reassembly** (`video_rtp.rs`): NAL-unit splitting relies on
encoder-side emulation prevention (which every spec-compliant H.264 encoder,
including `openh264`, applies) guaranteeing real NAL payload never contains a raw
start-code byte sequence — so a plain byte scan for `00 00 01` unambiguously finds
only real start codes, no bit-level parsing needed. The codec-dispatching entry
points (`fragment_video_frame`/`reassemble_video_frame`) have nothing to share
for a hypothetical VP8 arm beyond routing to it: VP8's own RTP framing (RFC 7741)
has no NAL-unit/start-code concept at all, so there's no common logic to factor
out of `fragment_nal_units`/`reassemble_nal_units` themselves.

**Codec dispatch generalized** (`video_codec_dispatch.rs`): `VideoEncoder`/
`VideoDecoder` enum-dispatch, mirroring `codec_dispatch.rs`'s `AudioEncoder`/
`AudioDecoder` pattern one-for-one. Only `VideoCodec::H264` exists as a real variant
today — this is prep work so a second video codec (VP8 was the one considered; it
and G.723.1 were explicitly attempted and deferred by the user, blocked on missing
system libs/sudo access and no acceptable pure-Rust crate at the time) slots in the
same low-friction way new audio codecs already do, not evidence a second codec is
implemented.

## DTLS-SRTP session driving (`dtls_demux.rs` / `dtls_srtp_session.rs`)

DeeLip's third media-encryption path (alongside SDES-SRTP and ZRTP) — see
`docs/crates/sip-core.md` for the SDP `a=fingerprint`/`a=setup` negotiation half;
this is the half that actually runs the handshake and hands `engine.rs`/`tasks.rs`
real SRTP keys.

- **`dtls_demux.rs`**: `webrtc_dtls::conn::DTLSConn` wants to *own* a `Conn` and run
  its own internal read loop — the opposite shape from ZRTP's byte-in/event-out
  `handle_incoming` (see below). Since only `tasks.rs`'s `recv_loop` can own the
  real socket's `recv_from`, `DemuxConn` wraps the same shared socket plus an inbound
  channel that `recv_loop` feeds whenever it classifies an incoming packet as a DTLS
  record (RFC 5764 §5.1.2's byte-range check, `is_dtls_packet`, alongside the
  existing `is_zrtp_packet` classification). Implements `webrtc-util 0.11`'s `Conn`
  trait — a third, isolated `webrtc-util` version alongside the `0.7` one
  `webrtc-srtp` pulls in transitively and the `0.17` one `RtpSocket::Relay`/ICE use
  elsewhere; Cargo allows this since nothing needs to unify them.
- **`dtls_srtp_session.rs::run_dtls_handshake`**: reconstructs a
  `webrtc_dtls::crypto::Certificate` from the same DER bytes `sip-core`'s
  `generate_dtls_cert()` produced, runs the handshake over `DemuxConn`, then exports
  SRTP keying material (`EXTRACTOR-dtls_srtp` label, RFC 5764 §4.2) once connected.
  `client_auth: ClientAuthType::RequireAnyClientCert` is required on the config even
  though DeeLip only ever needs one-way SDP-fingerprint verification per call leg —
  without it the server/`is_client: false` side never requests the client's
  certificate at all and `peer_certificates` comes back empty (confirmed by this
  module's own two-socket test failing with exactly that symptom before the setting
  was added). `insecure_skip_verify: true` is intentional: these are self-signed
  certs authenticated out-of-band via SDP, not a CA chain — the real
  MITM-prevention check is the explicit post-handshake comparison of the peer's
  actual certificate against `DtlsSrtpParams::expected_remote_fingerprint`
  (`DtlsSrtpOutcome::FingerprintMismatch` if they don't match).
- **`engine.rs`/`tasks.rs`**: `MediaEngine::start` spawns `run_dtls_handshake` as a
  background task when `dtls_srtp: Some(...)`, mirroring the ZRTP/`ZrtpRuntime`
  shape, but unlike ZRTP's initial Hello (sent synchronously before the recv
  task exists) the DTLS handshake has to run *concurrently* with `recv_loop`,
  since `recv_loop` is what feeds `DemuxConn` its inbound bytes in the first
  place — it can't run to completion before `recv_loop` starts the way ZRTP's
  Hello can. `dtls_encrypt_tx`/`rx` are always constructed (mirroring
  `zrtp_encrypt_tx`/`rx`) so the send task's `tokio::select!` arm type-checks
  unconditionally; it simply never fires when `dtls_srtp` was `None`. `tasks.rs`'s
  `recv_loop` owns a `DtlsRecvState`, reacts to `DtlsSrtpOutcome` (swap in the
  exported SRTP keys on `Secure`; tear down this call's media entirely via
  `stop_tx` on `FingerprintMismatch`, since a mismatch means the peer's actual
  certificate didn't match what SDP advertised — an active-attack indicator, not
  an ordinary negotiation failure. An ordinary `Failed`, e.g. a network hiccup
  during the handshake, instead just logs and falls back to unencrypted media,
  exactly like ZRTP's own `Failed` precedent).

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

- **Video has been confirmed working end-to-end through a real call, but not
  against real camera hardware.** This development environment has no camera
  device at all (no `/dev/video*`, no `uvcvideo` kernel module), so
  `nokhwa`'s device-enumeration/open/capture calls remain structurally
  untested here. What *has* been verified: two real, separate DeeLip
  processes (Local Account/serverless, one placing the call, one answering),
  each with a synthetic frame substituted for a real camera, completed a real
  SDP video negotiation and both sides rendered an actual decoded frame from
  the other party in their in-call video panel — a genuine encode → RTP →
  SRTP → decode → texture round trip through the real call-setup path, not
  just the isolated `video_engine.rs` unit tests. Real camera hardware still
  needs testing on a machine that has one.
- **No RTP reordering/jitter-buffering on the video recv side** — fragments are
  reassembled in arrival order only; real out-of-order delivery would corrupt a
  frame until the next keyframe. The same live 2-process test above surfaced a
  concrete, real instance of this: the callee side decoded exactly one frame
  successfully, then failed to decode every subsequent frame from the caller
  for the rest of the call, while the reverse direction (caller decoding the
  callee's stream) had zero failures — consistent with real packet reordering/
  loss corrupting the callee's reassembly in a way the caller's didn't hit,
  though the asymmetry wasn't root-caused further. Previously only a
  theoretical concern; now observed in practice. Worth revisiting before this
  is real-world-facing — not yet a scoped item in `ROADMAP.md`.
- **Conferencing now carries video** — merging two calls fans the local camera's
  single encoded stream out to both remote legs (one `H264Encoder`, two RTP sends,
  each leg decoded independently — mirrors `MediaEngine`'s "encode once, decode per
  leg" audio shape). If only one of the two merged calls negotiated video, that
  leg's video is kept and the other simply has none, rather than dropping video
  for both. See `video_engine.rs`'s `VideoConferenceLeg`. Leg 2 gets its own
  independent `RtpSender` — a fresh random SSRC and its own sequence/timestamp
  counters, a distinct RTP session from the receiving party's point of view —
  fed the same encoded fragments as leg 1 every tick, since the encode itself is
  shared (one camera, one encoder). Both legs use leg 1's own `payload_type`/
  `codec` for that shared bitstream, unchanged from today's actual behavior (a
  single global payload type for both legs); a conference where the two legs
  negotiate genuinely different video codecs isn't supported by this
  shared-single-encoder architecture, regardless of this limitation — out of
  scope here, not a bug.
- **MP3 recording's buffer-reservation requirement** (see above) is a correctness
  invariant, not a style choice — regressing it reintroduces a real SIGSEGV.
