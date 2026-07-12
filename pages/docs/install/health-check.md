# Health check

`scripts/health-check.sh` verifies the pieces [`install.sh`](/docs/install/install) may have
placed are still intact, and — with `--fix` — repairs whichever ones aren't. Useful when a user
(or some cleanup tool) has deleted a file DeeLip depends on existing outside its own control.

```sh
./scripts/health-check.sh
```

Same one-liner form as `install.sh`, if you don't have a local checkout:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/health-check.sh)" -- --fix
```

It checks four things:

1. **The `deelip` binary** — is it on `PATH`?
2. **The desktop-launcher entry** — does `deelip.desktop` exist (either the package-manager
   location under `/usr/share/applications` or your `--prefix`), and does its `Exec=` line
   resolve to a real, executable binary?
3. **The app icon** — does `deelip.png` exist under `hicolor/256x256/apps` in either location?
4. **The autostart entry** (only if you've enabled autostart in Settings) — same `Exec=`
   resolution check as the desktop entry.

It deliberately does **not** check `~/.config/deelip/` (the SQLite database, recordings, logs,
crash reports) — every table there is created with `CREATE TABLE IF NOT EXISTS` on load, so
that data is already self-healing and there's nothing for this script to fix.

## Flags

```
scripts/health-check.sh [--prefix=PATH] [--system] [--fix] [--version=TAG]
```

- `--prefix=PATH` — where to look for a `.tar.gz`-fallback install (default: same as
  `install.sh`'s — `~/.local`, or `/usr/local` with `--system`).
- `--system` — look under `/usr/local` instead of `~/.local`.
- `--fix` — attempt to repair anything found broken. Without it, the script only reports
  problems and changes nothing.
- `--version=TAG` — when repairing a `.tar.gz` install, fetch this release instead of the
  latest.

## What `--fix` actually does

If DeeLip was installed via a package manager, repair means reinstalling that package
(`apt-get install --reinstall`, `dnf`/`yum reinstall`, or `zypper install --force`) — the
package manager owns those files, so it's the one that fixes them. If it was the `.tar.gz`
fallback, `--fix` re-downloads the release tarball and restores only the specific files found
missing. Either way, if the autostart entry's `Exec=` pointed at a binary that no longer exists,
`--fix` repoints it at whichever binary path was just repaired, then re-runs every check to
confirm the repair actually worked.
