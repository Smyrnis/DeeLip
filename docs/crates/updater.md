# updater (`crates/updater`)

Self-update: checks the project's GitHub Releases for a newer DeeLip version and,
when the running binary is user-writable, replaces it in place. Used by `ui`'s
Settings/startup update-check flow; this crate has no UI of its own, just the
check/download/verify/swap mechanics.

## Architecture

- **`check_latest(current)`** — fetches `https://api.github.com/repos/{REPO}/releases/latest`,
  parses it, and returns a `ReleaseInfo` if the release's tag is strictly newer
  (semver) than `current`. Also opportunistically locates and fetches this release's
  `SHA256SUMS.txt` asset (see below) to populate `ReleaseInfo::tar_gz_sha256` ahead of
  time, so `download_and_replace` doesn't need a second network round-trip later just
  to verify.
- **`newer_version(tag, current)`** — the pure semver comparison (`v`-prefix stripped)
  behind `check_latest`'s "is this actually newer" decision.
- **`can_self_replace()`** — whether the running binary's directory is writable by
  this process, i.e. whether an in-place update is even possible for this install.
- **`download_and_replace(release)`** — downloads the release's `.tar.gz` asset,
  verifies its checksum via `verify_checksum`, extracts the `deelip` binary via
  `install_from_archive`, and atomically swaps it in.
- **`parse_sha256sums(body, filename)`** — pure parsing of `sha256sum`-format text
  (both its plain and `*`-prefixed binary-mode line formats), split out from the
  network fetch specifically so it's unit-testable against a hand-built string.
- **`install_from_archive(archive_path, current_exe)`** — the pure extract-and-swap
  half of `download_and_replace`, split out so it's testable against a hand-built
  local archive/target instead of a real download and the real running executable.

## Design decisions & invariants

- **System package installs (.deb/.rpm) are never self-updated.** `can_self_replace()`
  checks whether the running binary's *directory* is writable — true for a
  `~/.local/bin`-style user install (what `install.sh`'s tar.gz fallback produces),
  false for `/usr/bin` under dpkg/rpm's ownership. Overwriting a package-managed
  binary directly would desync the package database from what's actually on disk, so
  those installs are only ever offered a link to the release page instead, same as
  `install.sh` itself defers to the system package manager rather than fighting it.
  Only the directory's write permission matters, not the exe file's own: Linux refuses
  to open a currently-executing binary for writing (`ETXTBSY`), but
  `download_and_replace` never does that anyway — it stages the new binary alongside
  the old one and `rename()`s over it, a directory operation that works fine on a
  running executable.
- **Replacing a running executable is safe on Linux without stopping anything first.**
  Unlinking/renaming over the file backing an already-running process leaves that
  process executing fine off its old (now-unlinked) inode until it next exits — so
  `download_and_replace` can run while DeeLip is live, and the *next* launch is what
  picks up the new binary. Callers are expected to prompt the user to restart rather
  than doing it automatically (an in-progress call would otherwise be dropped).
- **Checksum verification, not signing — a deliberately named boundary.** This crate
  added SHA-256 checksum verification (`verify_checksum`, `SHA256SUMS.txt` published by
  `.github/workflows/package.yml` alongside every release) after an earlier audit
  found the update mechanism had *no* integrity verification at all — TLS protected
  only the transport, not the downloaded content. What shipped catches corruption in
  transit and a tampered asset that doesn't also carry a matching tampered checksum.
  It does **not** protect against a fully compromised release pipeline, where an
  attacker controlling CI could tamper with both the binary and its checksum file
  together — that would need GPG/Sigstore signing with a key held outside CI's own
  reach, out of scope for a one-person project without that infrastructure.
- **A missing checksum warns, never blocks.** `ReleaseInfo::tar_gz_sha256` is `None`
  when a release predates the checksum step or its `SHA256SUMS.txt` didn't contain a
  matching line; `verify_checksum` treats `None` as "nothing to verify against" and
  proceeds with a `tracing::warn!`, rather than refusing to update — so an older
  already-published release can't get permanently stuck unable to self-update.
- **A non-semver tag reports "nothing to do," not an error.** Both `check_latest` (via
  `newer_version`) and any hand-pushed tag that doesn't parse as semver are treated as
  "no update available" rather than a failure — a stray non-version tag shouldn't nag
  the user with an error.

## Known limitations / open items

None beyond the signing boundary already named above — this crate's scope has stayed
exactly what it set out to be (integrity-checked self-update for user-writable
installs) since the checksum-verification audit that shaped its current design.
