---
title: FAQ
pageClass: faq-page
---

# FAQ

### What is DeeLip?

A lightweight SIP softphone for Linux, Windows, and macOS — full SIP calling, ZRTP end-to-end
encrypted audio/video, wide codec support, real NAT traversal, and a clean native desktop UI,
without Electron.

### Is it free?

Yes — DeeLip is open source. Get it from [Downloads](/downloads/) or build it yourself from the
[source on GitHub](https://github.com/Smyrnis/DeeLip).

### Which distros/platforms and architectures are supported?

On Linux, prebuilt packages cover Debian/Ubuntu (`.deb`), Fedora/RHEL (`.rpm`), and openSUSE
(`.rpm`), plus a portable `.tar.gz` for anything else (Arch, Alpine, ...). Windows (`.msi`) and
macOS (`.dmg`) installers are also published, though they're newer and less battle-tested than
the Linux packages. Only `x86_64` builds are published for any platform today —
`scripts/install.sh` exits with an error on other architectures on Linux. See
[Downloads](/downloads/).

### Is there a `curl | bash` one-liner?

Yes:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/install.sh)"
```

`bash -c "$(curl ...)"` rather than a plain `curl ... | bash` pipe is deliberate — piping leaves
no real stdin for a prompt to read, which matters for `scripts/uninstall.sh --purge`'s
confirmation prompt (pass `-y`/`--yes`, or use this same `bash -c` form, to answer it
non-interactively). See [Downloads](/downloads/) for the equivalent `git clone` + run form, if
you'd rather have a local checkout.

### Where does DeeLip store my data?

Everything — accounts, contacts, call history, recordings, logs, crash reports — lives in one
SQLite database under your OS's standard config directory plus a `deelip` folder: `~/.config/deelip/`
on Linux, `%APPDATA%\deelip\` on Windows, `~/Library/Application Support/deelip/` on macOS. Every
table is created with `CREATE TABLE IF NOT EXISTS` on load, so that directory is self-healing;
uninstalling normally leaves it untouched (see [Uninstall](/docs/install/uninstall)'s `--purge`
flag if you want it gone too, on Linux).

### Does it work behind NAT / on a real-world network, not just a LAN?

Yes — DeeLip implements STUN, TURN relay fallback, and full ICE for RTP media traversal. See the
[NAT docs](/docs/guide/nat) for how the pieces fit together.

### Is the encryption real end-to-end, or just transport encryption?

Real ZRTP (RFC 6189), implemented from scratch with SAS (Short Authentication String)
verification, alongside SDES-SRTP as an alternative. Media stays encrypted peer-to-peer; see the
[Calling & security](/docs/guide/calling-security) and [Audio & video quality](/docs/guide/audio-video)
docs for the protocol and RTP-level details.

### It's not working — where do I start?

Run `scripts/health-check.sh` — see [Troubleshooting](/troubleshooting/).

### I have a question that isn't answered here.

See [Contact](/contact/).
