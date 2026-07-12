# Audio & video quality

Once a call is connected, this is what actually carries your voice (and video) back
and forth — and what makes it sound clear instead of choppy or echoey.

## Audio

- **Wide codec support** — G.711, G.722, G.729, GSM, iLBC, and Opus. DeeLip picks a
  codec both sides support automatically, so it interoperates with pretty much any
  SIP provider or PBX. In Settings > Account you can reorder your preferred codecs,
  disable ones you don't want offered, or force a specific codec for incoming calls.
- **Echo cancellation** — removes the sound of your own speaker bleeding back into
  your microphone, tuned to converge quickly without the loud transient "blow-up"
  cheaper implementations sometimes have. Toggle it in Settings > Audio.
- **Automatic gain control (AGC)** — keeps your outgoing volume steady whether you're
  speaking softly or loudly, without pumping the volume up during silence. Toggle it
  in Settings > Audio, next to echo cancellation.
- **Comfort noise** — during silence, a low-level synthetic noise floor is sent
  instead of dead air, so the call doesn't sound like it dropped.
- **3-way conferencing** — bridge two calls into one local conference; both legs are
  mixed together for you and for each other party.

Settings > Audio also has separate pickers for your input device, output device, and
ringtone device, a custom ringtone file (with its own volume slider) if you don't
want the default, and a recording toggle — see below.

## Video

Video calling uses H.264, negotiated automatically alongside audio when both sides
support a camera. It rides the same encrypted call as audio — see
[Calling & security](/docs/guide/calling-security) for how that encryption works.

Pick your camera in Settings > Video, which lists every detected device with a
refresh button (useful if you plug one in after opening Settings); if none are
found, it tells you so instead of leaving the list blank.

## Recording

Record a call to WAV (lossless) or MP3, with your microphone and the other party's
audio on separate stereo channels so you can tell the two apart afterward. Turn
recording on, pick the format, and optionally choose a custom save folder in
Settings > Audio — otherwise recordings go to the default recordings folder (see
[Your data & privacy](/docs/guide/data-privacy)).

## A note on video and conferencing

Video calling works for regular two-party calls. Local 3-way conferencing currently
stays audio-only — if either leg had video, it's dropped when the calls are bridged.

---
Curious how any of this actually works under the hood? The engineering notes live
in [`docs/crates/media-engine.md`](https://github.com/Smyrnis/DeeLip/blob/main/docs/crates/media-engine.md)
on GitHub.
