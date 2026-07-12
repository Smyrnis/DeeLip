---
title: Downloads
---

# Downloads

<div class="card-grid">
  <div class="card">
    <h3>Prebuilt packages</h3>
    <p>The fastest path — pick your distro's package from the latest GitHub Release.</p>
    <p><a href="https://github.com/Smyrnis/DeeLip/releases/latest" target="_blank" rel="noopener">Go to the latest release →</a></p>
  </div>
  <div class="card">
    <h3>One-line install</h3>
    <p>Run the install script straight from GitHub — no local checkout needed. It detects your
    package manager for you.</p>
  </div>
</div>

## Linux

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/install.sh)"
```

<div class="pill-row">
  <span class="pill">Debian / Ubuntu — .deb via apt</span>
  <span class="pill">Fedora / RHEL — .rpm via dnf/yum</span>
  <span class="pill">openSUSE — .rpm via zypper</span>
  <span class="pill">Arch / Alpine / other — .tar.gz</span>
</div>

`install.sh` detects your package manager, downloads the matching asset from the latest GitHub
Release, and installs it — `apt-get`/`dnf`/`yum`/`zypper` on supported distros, or a portable
`.tar.gz` unpacked into `~/.local` (or `/usr/local` with `--system`) everywhere else. Only
`x86_64` builds are published today.

Prefer a local checkout instead (e.g. to read the source first, or to contribute)?

```sh
git clone https://github.com/Smyrnis/DeeLip.git
cd DeeLip
./scripts/install.sh
```

Full flag reference, including `--version=TAG` to pin a specific release: see the
[Install docs](/docs/install/install).

## Windows

Download the `.msi` from the [latest release](https://github.com/Smyrnis/DeeLip/releases/latest)
and run it — a standard installer with a Start Menu shortcut and an optional "add to PATH" step.
The Windows build is newer than the Linux packages above and hasn't seen as much real-world
testing yet; please [report an issue](https://github.com/Smyrnis/DeeLip/issues/new) if something
doesn't work.

## macOS

Download the `.dmg` from the [latest release](https://github.com/Smyrnis/DeeLip/releases/latest),
open it, and drag `DeeLip.app` into `Applications`. The build isn't code-signed or notarized yet,
so Gatekeeper will likely block the first launch — right-click (or Control-click) the app and
choose **Open** instead of double-clicking to get past that once. Like Windows, this build is
newer than the Linux packages and hasn't seen as much real-world testing; please
[report an issue](https://github.com/Smyrnis/DeeLip/issues/new) if something doesn't work.

## Manual download

Prefer to grab the file yourself? Every [release](https://github.com/Smyrnis/DeeLip/releases)
publishes a `.deb`, `.rpm`, and `.tar.gz` for Linux, a `.msi` for Windows, and a `.dmg` for macOS —
download the one matching your platform.

## After installing (Linux)

- [`scripts/health-check.sh`](/docs/install/health-check) — verify (and `--fix`) your install
- [`scripts/uninstall.sh`](/docs/install/uninstall) — remove DeeLip cleanly
- Still stuck? See [Troubleshooting](/troubleshooting/) or [Contact](/contact/).
