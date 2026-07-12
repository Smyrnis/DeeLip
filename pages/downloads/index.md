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

## Install via script

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

## Manual download

Prefer to grab the file yourself? Every [release](https://github.com/Smyrnis/DeeLip/releases)
publishes a `.deb`, `.rpm`, and `.tar.gz` — download the one matching your distro and install it
with your normal package manager, or unpack the tarball anywhere on your `PATH`.

## After installing

- [`scripts/health-check.sh`](/docs/install/health-check) — verify (and `--fix`) your install
- [`scripts/uninstall.sh`](/docs/install/uninstall) — remove DeeLip cleanly
- Still stuck? See [Troubleshooting](/troubleshooting/) or [Contact](/contact/).
