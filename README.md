<p align="center">
  <img src="assets/icon.png" width="200" alt="DeeLip logo">
</p>

<h1 align="center">DeeLip</h1>

<p align="center">
  A lightweight SIP softphone for Linux, Windows, and macOS — encrypted calls, video,<br>
  and a clean native desktop UI. No Electron, no bloat.
</p>

## Features

- Full SIP calling — registration, hold/resume, attended transfer, 3-way local conferencing
- ZRTP end-to-end encryption (RFC 6189, implemented from scratch) alongside SDES-SRTP
- Video calling, negotiated additively over the same encrypted RTP session as audio
- Wide codec support — G.711, G.722, G.729, GSM, iLBC, Opus — with acoustic echo cancellation and AGC
- Real NAT traversal — STUN, TURN relay fallback, and full ICE
- SIP presence, voicemail (MWI) notifications, and threaded SIP MESSAGE chat
- Do-not-disturb, call forwarding, a configurable dial plan, and LDAP directory search
- Self-updating, with a SHA-256-verified download before it swaps in

## Get DeeLip

See **[smyrnis.github.io/DeeLip](https://smyrnis.github.io/DeeLip/)** for downloads, install
instructions, and full docs.

## License

MIT — see [LICENSE](LICENSE).
