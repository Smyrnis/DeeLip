# Documentation

What DeeLip actually does, explained for anyone evaluating or using it — not an
engineering reference. If you're looking for how something is implemented rather
than what it does, the in-depth technical notes for each part of the codebase live
in [`docs/crates/`](https://github.com/Smyrnis/DeeLip/tree/main/docs/crates) at the
repo root, linked directly on GitHub rather than published here (same as the
project's [`ARCHITECTURE_GAPS.md`](https://github.com/Smyrnis/DeeLip/blob/main/ARCHITECTURE_GAPS.md)).

For what changed and when, see the [Changelog](changelogs/CHANGELOG).

## What's covered

| Page | What it answers |
|---|---|
| [Calling & security](guide/calling-security) | What calling features DeeLip has, and how encrypted calls work. |
| [Audio & video quality](guide/audio-video) | Codec support, echo cancellation, recording, and video calling. |
| [Your data & privacy](guide/data-privacy) | What DeeLip stores, where, and how to remove it. |
| [Working behind your router (NAT)](guide/nat) | Why calls connect reliably even behind a home router. |
| [The interface](guide/interface) | A tour of the app's tabs, tray icon, hotkeys, and Settings. |
| [Staying up to date](guide/updates) | How self-update works, and what's different for package-manager installs. |
| [Language support](guide/language) | Current translation status. |

New here? Start with [Calling & security](guide/calling-security) and
[Audio & video quality](guide/audio-video) — together they cover what a call
actually is. [Your data & privacy](guide/data-privacy) and [The interface](guide/interface)
cover the rest of the day-to-day app.
