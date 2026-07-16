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

Flags: `--version=TAG` pins a specific release instead of the latest; `--prefix=PATH`
and `--system` (installs to `/usr/local` instead of `~/.local`) control the `.tar.gz`
fallback path only — `.deb`/`.rpm` installs always go system-wide via the package
manager. Full reference: the script's own `--help`, or
[the source](https://github.com/Smyrnis/DeeLip/blob/main/scripts/install.sh).

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

- `scripts/health-check.sh --fix` — verify (and repair) your install; see
  [Troubleshooting](/troubleshooting/).
- `scripts/uninstall.sh` — remove DeeLip cleanly; add `--purge` to also delete your
  accounts/history/recordings (kept by default).
- Still stuck? See [Contact](/contact/).
