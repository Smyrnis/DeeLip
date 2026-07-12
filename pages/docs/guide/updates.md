# Staying up to date

If DeeLip's own install directory is writable by you, DeeLip can update itself —
in practice that's a portable `.tar.gz` install to the default `~/.local` prefix.
Package-manager installs, and any install placed in a root-owned system directory,
work differently — see below.

## How it works

1. On startup, and periodically after that based on your Settings (Always, Daily,
   Weekly, or Never — set in Settings > Advanced), DeeLip checks the project's
   GitHub Releases for a newer version.
2. If one's available, you get a small popup with **Update Now**, **Skip**, or
   **Later** — nothing happens automatically without your say-so, unless you've
   turned on auto-update in Settings. Choosing "Skip" remembers that version, so you
   won't be prompted about it again.
3. When you approve, DeeLip downloads the new release, verifies its checksum, and
   swaps the new binary in. Nothing is applied until you restart the app — an
   in-progress call is never interrupted by an update. You can also trigger a check
   manually with the "Check now" button in Settings > Advanced.

## Package-manager installs (.deb/.rpm)

If you installed DeeLip via `apt`/`dnf`/`yum`/`zypper`, or into a system-wide prefix
it can't write to on its own, self-update is intentionally disabled — updating
those is your package manager's job, not DeeLip's. You'll still get notified that a
new version exists, with a link to the release, but you'll update it the same way
you installed it. See [Downloads](/downloads/).

## Integrity

Every downloaded release is checked against a published SHA-256 checksum before
it's installed, catching a corrupted download or a tampered file that doesn't match
what was published.

---
Curious how any of this actually works under the hood? The engineering notes live
in [`docs/crates/updater.md`](https://github.com/Smyrnis/DeeLip/blob/main/docs/crates/updater.md)
on GitHub.
