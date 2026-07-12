# The interface

A tour of DeeLip's desktop app — a native window (no Electron), fast, and built
around a single clean light theme.

## Main tabs

- **Dialer** — a keypad for placing calls, plus the live in-call screen (mute,
  hold, transfer, keypad for DTMF, recording toggle) once you're connected.
- **History** — every past call, searchable, with missed calls flagged.
- **Contacts** — your address book, plus LDAP directory search if your organization
  has one configured.
- **Messages** — threaded SIP instant-message conversations, opened as their own
  window so you can keep chatting while doing something else.

## Settings

A tabbed dialog — every section is sized to be usable without scrolling wherever
possible, so you're not hunting through one long page.

- **General** — notification and ringtone toggles, whether the incoming-call popup
  appears at a random screen position, what a contact's double-click does (call,
  message, or edit), single-call mode (one call at a time instead of allowing a
  second while one's active), start-minimized, logging to a file, local-only crash
  reporting, and launching DeeLip on login.
- **Account** — your SIP identity/identities and all the per-account calling,
  codec, and security options covered in [Calling & security](/docs/guide/calling-security)
  and [Audio & video quality](/docs/guide/audio-video).
- **Audio** / **Video** — device selection and quality controls, see
  [Audio & video quality](/docs/guide/audio-video).
- **Network** — STUN/TURN/ICE and port settings, see
  [Working behind your router (NAT)](/docs/guide/nat).
- **Directory** — connect to an LDAP directory (server, port, base DN, bind DN and
  password, TLS, and a custom search filter) so contacts from a company directory
  show up alongside your own address book.
- **Hotkeys** — an enable/disable switch, three bindable global shortcuts (Answer,
  Hangup, Mute — `Ctrl+Alt+A`/`H`/`M` by default) that work even when DeeLip isn't
  focused, and a toggle for responding to a headset's hardware answer/hangup button.
- **Advanced** — how often DeeLip checks for updates and whether it updates itself
  automatically (see [Staying up to date](/docs/guide/updates)), the blocklist
  manager (see [Calling & security](/docs/guide/calling-security)), exporting call history
  to CSV, and importing/exporting contacts as CSV or vCard.

## Beyond the main window

- **System tray** — hide DeeLip to the tray and get it back with a click; a badge
  shows your missed-call count.
- **Global hotkeys** — bind Answer/Hangup/Mute (and a hardware headset button) so
  you can control a call without the window focused.
- **Desktop notifications** — an incoming call notification with Accept/Reject
  buttons built in, even if DeeLip isn't the focused window.
- **Pop-out windows** — Settings, Messages, Transfer, and the DTMF keypad each open
  as their own real OS window, so you can move them wherever you like instead of
  them being stuck inside the main window.

## Look and feel

One consistent color palette (sourced from JetBrains' IntelliJ Light theme) and
JetBrains Mono throughout, rather than a mix of ad hoc styling. Color is used
consistently to mean something: a status color always signals call state (active,
ringing/hold, or a destructive action), never just decoration.

---
Curious how any of this actually works under the hood? The engineering notes live
in [`docs/crates/ui.md`](https://github.com/Smyrnis/DeeLip/blob/main/docs/crates/ui.md)
on GitHub.
