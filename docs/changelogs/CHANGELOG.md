# Changelog

All notable changes to DeeLip are documented in this file. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project doesn't yet
follow semantic versioning strictly (still pre-1.0, no tagged releases), but will once
the first one ships.

Entries are curated by user-visible effect, not a 1:1 mirror of commit history — see
`git log` for the exact commit-level record.

## [Unreleased]

Everything so far — no version has been tagged/released yet.

### Added
- Core SIP softphone: registration, calling, hold/resume, attended transfer, 3-way
  local conference, call recording (WAV/MP3) with CSV/vCard contact import-export.
- Audio: G.711 (A-law/µ-law), G.722, G.729, GSM, iLBC, and Opus codecs; configurable
  DTMF mode (RFC 2833 or SIP INFO); acoustic echo cancellation and AGC; VAD-driven
  RFC 3389 comfort noise.
- Video calling: H.264 capture/encode/decode over SRTP-protected RTP, negotiated
  additively alongside the audio leg.
- NAT traversal: STUN, TURN relay fallback, and full ICE.
- ZRTP (RFC 6189) end-to-end encryption with SAS verification, alongside SDES-SRTP.
- SIP presence (PIDF) subscriptions, voicemail (MWI) notifications, and SIP MESSAGE-based
  instant messaging with a threaded Messages window.
- Call-handling rules: do-not-disturb, call forwarding (always/busy/no-answer), a
  configurable dial plan with prefix rules.
- Directory search over LDAP.
- Global hotkeys, system tray with missed-call badge and quick actions, interactive
  call notifications, desktop autostart.
- Self-update: release-checking and checksummed (SHA-256) binary replacement.
- Linux packaging: .deb, .rpm, .AppImage, and a portable .tar.gz, plus a distro-detecting
  `install.sh`.
- Multi-language infrastructure (JSON locale files, English shipped so far).
- IntelliJ-inspired UI (Darcula-derived, later switched to a light-only palette) with
  JetBrains Mono throughout, built on `egui`/`eframe`.

### Changed
- Config/contacts/history/messages storage moved from TOML+JSON files to a single
  SQLite database.
- SDP/codec/STUN/TURN/ICE negotiation moved out of the `ui` crate into `sip-core`,
  where the rest of call signaling already lived.
- Several large files split into per-concern modules/directories as they grew:
  `sip-core`'s `client.rs` and call lifecycle, `ui`'s `lib.rs`/`settings.rs`/`dialer.rs`,
  `CallStatus` display logic consolidated out of three separate copies.
- Settings and Messages became genuine separate OS windows (`Deferred` egui viewports)
  instead of in-canvas overlays, fixing a real "can't move the Settings window" bug and
  a redraw-throttling slowdown while it was open.

### Fixed
- ACK/redirect branch handling, a stuck status line, a Via-echo bug, list-row hover
  collisions, a History lag, a Settings-window lag, and a call-recording/hold
  interaction bug.
- Live calls now fail cleanly on transport loss instead of hanging; SIP INFO messages
  are answered correctly.
- A SIGSEGV in MP3 call recording caused by calling the LAME encoder wrapper without
  reserving its output buffer first.

### Documentation
- Introduced a `docs/` book, one file per crate, replacing an earlier set of ad hoc
  per-topic write-ups; this changelog.
