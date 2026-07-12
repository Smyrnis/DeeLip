# Your data & privacy

DeeLip doesn't have an account, a cloud sync, or any telemetry. Here's exactly what
it stores and where.

## One file, on your machine

Everything — your SIP accounts, contacts, call history, messages, dial-plan rules,
and settings — lives in a single SQLite database at `~/.config/deelip/deelip.db`.
Nothing is uploaded anywhere. The only network traffic DeeLip generates is the SIP
signaling and call media you'd expect from a softphone: talking to your SIP
provider and whoever you're calling.

A few other things can also land on disk under `~/.config/deelip/`, all local-only:

- **`deelip.log`** — if you turn on "log to file" in Settings > General.
- **`recordings/`** — call recordings, if you've enabled recording in Settings >
  Audio (or wherever you've pointed it to instead).
- **`crashes/`** — crash reports, on by default. These stay on your machine; DeeLip
  never uploads them anywhere, "no telemetry" applies here too.

If you're upgrading from a very old version that used separate `config.toml`/
`contacts.json`/`history.json` files, those are read once and migrated into the
database automatically the first time you run a newer DeeLip — the old files are
left in place afterward, untouched.

## What's in there

- **Accounts** — one or more SIP identities, each with its own server, credentials,
  and per-account preferences (codec order, transport, video on/off, and more).
- **Contacts, call history, and messages** — your address book, a log of past calls,
  and SIP message threads.
- **Dial plan** — rules for how a dialed number gets rewritten before it's sent (e.g.
  stripping a prefix, adding a country code).
- **Settings** — everything configurable from the Settings window, from ringtone
  choice to autostart.

## Local Account (no server at all)

You don't need a SIP provider to use DeeLip. A "Local Account" places and receives
calls directly to/from an IP address, with no registration step — see
[Calling & security](/docs/guide/calling-security) for what that looks like in practice.

## Removing your data

Uninstalling DeeLip (`scripts/uninstall.sh`) leaves `~/.config/deelip/` alone by
default — your accounts and history stay put in case you reinstall. Pass `--purge`
if you want that directory deleted too. See [Uninstall](/docs/install/uninstall).

---
Curious how any of this actually works under the hood? The engineering notes live
in [`docs/crates/config.md`](https://github.com/Smyrnis/DeeLip/blob/main/docs/crates/config.md)
on GitHub.
