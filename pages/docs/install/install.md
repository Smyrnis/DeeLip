# Install

> On Windows or macOS, this page doesn't apply — see [Downloads](/downloads/) for the `.msi`/`.dmg`
> installer instead. `install.sh` is a Linux-only script.

`scripts/install.sh` installs a prebuilt DeeLip package for your distro straight from GitHub
Releases — no local Rust toolchain or compiling required. Run it straight from GitHub, no
checkout needed:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/install.sh)"
```

`bash -c "$(curl ...)"` rather than a plain `curl ... | bash` pipe is deliberate — it leaves your
real stdin free, which matters for `uninstall.sh --purge`'s confirmation prompt (see
[Uninstall](/docs/install/uninstall)). Pass flags after a `--`:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/install.sh)" -- --version=v0.2.0
```

Prefer a local checkout instead?

```sh
git clone https://github.com/Smyrnis/DeeLip.git
cd DeeLip
./scripts/install.sh
```

<div class="pill-row">
  <span class="pill">apt → .deb</span>
  <span class="pill">dnf / yum → .rpm</span>
  <span class="pill">zypper → .rpm</span>
  <span class="pill">other → .tar.gz</span>
</div>

The script detects your package manager and picks the matching release asset:

| Family | Detected via | Package | Install method |
|---|---|---|---|
| Debian/Ubuntu | `apt` | `.deb` | `apt-get install` |
| Fedora/RHEL | `dnf`/`yum` | `.rpm` | `dnf`/`yum install` |
| openSUSE | `zypper` | `.rpm` | `zypper install` |
| Arch, Alpine, anything else | — | `.tar.gz` | unpacked into `--prefix` |

Only `x86_64` is published — `install.sh` exits early with an error on any other architecture.

## Flags

```
scripts/install.sh [--version=TAG] [--prefix=PATH] [--system]
```

- `--version=TAG` — install a specific release tag instead of the latest.
- `--prefix=PATH` — install prefix for the `.tar.gz` fallback only (default: `~/.local`).
- `--system` — for the `.tar.gz` fallback, install to `/usr/local` instead of `~/.local`.
  Ignored for `.deb`/`.rpm` — those always install system-wide via the package manager.

The `.deb`/`.rpm`/`.zypper` paths call `sudo` for you when needed. The `.tar.gz` fallback places
the binary, desktop-launcher entry, and icon under `--prefix`, then refreshes the desktop/icon
caches if `update-desktop-database`/`gtk-update-icon-cache` are available.

If `$PREFIX/bin` isn't already on your `PATH`, the script tells you at the end.

### Env

- `DEELIP_SCRIPTS_REF` — when run via the `curl`/`bash -c` one-liner (no local checkout to find
  `scripts/helpers/lib.sh` next to), this picks which branch or tag to fetch it from. Defaults to
  `main`; set it to a release tag if you fetched that tag's `install.sh` specifically rather than
  `main`'s.

## Next steps

Something not working? [`scripts/health-check.sh`](/docs/install/health-check) verifies every
piece `install.sh` placed is intact, and can repair it. To remove DeeLip, see
[Uninstall](/docs/install/uninstall).
