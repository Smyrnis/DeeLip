---
title: Troubleshooting
pageClass: troubleshooting-page
---

# Troubleshooting

## Start here: health check

```sh
./scripts/health-check.sh --fix
```

No local checkout handy? Same thing as a one-liner:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/health-check.sh)" -- --fix
```

This checks four things an install can lose without you noticing — the `deelip` binary on
`PATH`, the desktop-launcher entry, the app icon, and (if enabled) the autostart entry — and
`--fix` repairs whichever ones are broken. It never touches your accounts, contacts, history, or
recordings; that data is self-healing on its own (every table is created with
`CREATE TABLE IF NOT EXISTS`).

If health-check reports everything healthy but DeeLip still isn't behaving, it's a runtime issue
rather than an install issue — see below.

## Calls won't connect / registration fails

Check your SIP account settings first (server, port, transport). If registration succeeds but
calls fail to connect specifically behind a router/firewall, that's a NAT traversal problem —
DeeLip uses STUN (on by default), full ICE, and TURN relay fallback (both configurable in
Settings > Network) to work around this. One known limitation: STUN support is IPv4-only today.

## Audio problems (choppy, no audio, echo)

DeeLip auto-negotiates a codec both sides support (G.711/G.722/G.729/GSM/iLBC/Opus), with
echo cancellation and automatic gain control available in Settings > Audio. If calls connect
but audio is missing or distorted, double-check your system's default input/output devices —
DeeLip uses whatever ALSA/PulseAudio reports as default.

## Video problems

Video negotiation rides on the same encrypted RTP session as audio (H.264, negotiated
additively). Two known gaps worth knowing about if video misbehaves: there's no jitter-buffering
on the receive side yet (real out-of-order packet delivery can corrupt a frame until the next
keyframe), and local conferencing currently stays audio-only even if both legs had video.

## Encrypted calls / SAS verification

DeeLip implements ZRTP (RFC 6189) from scratch, with SAS (Short Authentication String)
verification, alongside SDES-SRTP as a simpler alternative. Encryption mode is set per
account in Settings > Account (Match Transport / Disabled / Enabled / ZRTP) — if a call
won't establish encryption, confirm both sides are using a compatible mode.

## Still stuck?

See [Contact](/contact/) — include what `scripts/health-check.sh` reported, and whether the
problem is registration, call setup, audio, or video.
