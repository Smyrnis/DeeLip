---
name: verify
description: Launch and drive DeeLip (egui desktop SIP softphone) under Xvfb to verify a change actually works, not just that it builds/tests clean.
---

# Verifying DeeLip end-to-end

DeeLip is a GUI app (egui/eframe), so its real surface is pixels, not a CLI
or API. This environment has no window manager, no display, and no audio
hardware guarantees other than what's on the host -- but Xvfb + xdotool +
scrot works fine for driving it headlessly. This has been done successfully
multiple times across sessions (redirect-call feature, video calling,
DTLS-SRTP).

## Build

`cargo build -p deelip` (debug is fine and fast; release only if you
specifically need it).

## Seeding accounts without clicking through Settings

Config lives in SQLite (`deelip_config::Db`, respects `$XDG_CONFIG_HOME` via
the `dirs` crate -- point it at a scratch dir per instance to isolate them).
For anything beyond the default single-account seed, write a **throwaway
example binary** in `crates/config/examples/` that does:

```rust
use deelip_config::{AppConfig, Db, SipAccount};
let db = Db::open_default()?; // honors XDG_CONFIG_HOME from the env
let mut cfg = AppConfig::default();
cfg.local_sip_port = <port>;
cfg.accounts = vec![SipAccount { local_account: true, /* ...overrides... */ ..Default::default() }];
cfg.save(&db)?;
```

Run it with `XDG_CONFIG_HOME=<scratch dir> cargo run --example <name> -p deelip-config -- <args>`,
then **delete the example file** once done verifying (matches this
project's established throwaway-tool convention -- never leave scratch
`examples/` binaries committed).

For a two-instance same-machine call, `local_account: true` lets each
instance dial the other directly by `<ip>:<port>` (e.g. `127.0.0.1:15080`)
with no registrar/proxy needed at all -- see
`crates/sip-core/src/call/lifecycle/outgoing.rs::resolve_local_call_target`.
Set `auto_answer_enabled: true, auto_answer_secs: 1` on the callee's account
so you don't need to click Accept.

## Launching two instances under Xvfb

```bash
Xvfb :97 -screen 0 1280x800x24 &
DISPLAY=:97 XDG_CONFIG_HOME=<scratch>/cfg_a RUST_LOG="deelip=debug,deelip_sip=debug,deelip_media=debug,deelip_nat=info" \
  ./target/debug/deelip > <scratch>/a.log 2>&1 &
DISPLAY=:97 XDG_CONFIG_HOME=<scratch>/cfg_b RUST_LOG=... ./target/debug/deelip > <scratch>/b.log 2>&1 &
sleep 3
DISPLAY=:97 xdotool search --name "DeeLip"   # both windows have the SAME title
```

Map window IDs to instances via `xdotool getwindowpid <id>` against the
PIDs you captured, not the window title. Both windows default to the same
`0,0` position -- `xdotool windowmove <id> <x> 0` to separate them before
screenshotting side-by-side with one `scrot`.

## Driving it -- no window manager, so the obvious xdotool calls don't work

- `xdotool windowactivate`/`getactivewindow` **fail** ("windowmanager
  claims not to support _NET_ACTIVE_WINDOW") -- expected, ignore the
  warning text, don't try to work around it.
- `xdotool click --window <id> 1` **does not click where you think** --
  it does NOT click at the widget's on-screen position, it's unreliable
  without a prior mouse warp. Always do
  `xdotool mousemove --window <id> <x> <y>` (coords relative to that
  window's own top-left, read off a screenshot) **then** a separate
  `xdotool click 1` (no `--window`).
- `xdotool type "text"` (no `--window` needed) goes to whatever has real X
  input focus, which a real mouse click (via the mousemove+click sequence
  above) on a text field correctly sets even with no WM -- confirmed
  working for the dialer's number-entry field.
- Pressing Return while the dial field has focus submits the call
  (`ctx_key_enter` check in `views/dialer/idle.rs`) -- no need to click a
  separate Call button if you just typed the target and want to dial.

## What to check after a call connects

- Screenshot both windows together -- look for "connected" status + a
  running call timer on both sides.
- `grep` both instances' log files for feature-specific tracing lines
  (e.g. this project's media-encryption paths all log a
  `debug!("<PROTOCOL>: switching to SRTP-encrypted recv/send")` on success)
  and for the absence of any `WARN`/`ERROR` lines your change could plausibly
  cause. Pre-existing unrelated noise to ignore: STUN-failed-using-local-IP
  warning (no real STUN server in a scratch test), and a
  `libayatana-appindicator` deprecation warning from the tray icon.
- If you hang up and re-screenshot: this project's `hang_up()` (`call/lifecycle/teardown.rs`)
  does **not** log its outgoing BYE at all (only the *incoming* BYE handler
  logs `debug!("← BYE ...")`), so an absence of a "→ BYE" log line is
  normal, not evidence anything failed on the sending side. Verified during
  the DTLS-SRTP session: hanging up on one instance reset its own UI to
  idle but the other side kept showing "connected" for minutes afterward
  with no `← BYE` ever logged on its side either -- looked like a real,
  pre-existing teardown bug in this same-host two-process test setup,
  unrelated to whatever feature was being verified. Worth a fresh look if
  it recurs, but don't let it block verifying an unrelated change.

## Cleanup

Always: `kill <app pids>`, `pkill -f "Xvfb :97"`, delete the scratch
`XDG_CONFIG_HOME` dirs, and delete any throwaway `crates/*/examples/*.rs`
seed binary you added -- this project's own memory/conventions expect
verification scratch state to leave no trace.
