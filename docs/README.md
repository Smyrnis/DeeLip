# DeeLip docs

DeeLip is a Linux SIP softphone (MicroSIP-inspired), built as a Cargo workspace of six
crates. This is the index into how each one works — the authoritative "how does this
app work" reference, replacing the design-rationale comments that used to live
scattered across the source (see each crate's own doc for a "Design decisions &
invariants" section covering exactly that content, moved here so it can't drift out of
sync with two copies).

For what changed and when, see [`CHANGELOG.md`](changelogs/CHANGELOG.md).
For the current backlog of in-progress architecture/cleanup work, see
[`ARCHITECTURE_GAPS.md`](../ARCHITECTURE_GAPS.md) at the repo root.

## Workspace map

| Crate | Doc | What it owns |
|---|---|---|
| `sip-core` | [`sip-core.md`](crates/sip-core.md) | The SIP user-agent: registration, call signaling (INVITE/BYE/re-INVITE), SDP/ICE negotiation, presence/MWI/messaging subscriptions, and the ZRTP protocol/handshake implementation. |
| `media-engine` | [`media-engine.md`](crates/media-engine.md) | Everything downstream of signaling for one call's actual media: audio capture/AEC/AGC/VAD/codecs/RTP/SRTP, call recording, video capture/encode/decode, and driving ZRTP against a real RTP socket. |
| `config` | [`config.md`](crates/config.md) | All persisted state — `AppConfig`/`SipAccount`, contacts, history, messages, dial plan, autostart — backed by one SQLite database. |
| `nat` | [`nat.md`](crates/nat.md) | NAT traversal for RTP media: STUN, TURN relay fallback, and full ICE. |
| `ui` | [`ui.md`](crates/ui.md) | The `eframe`/`egui` desktop UI — `DeelipApp`, the frame/render loop, views (Dialer/History/Contacts/Messages/Directory/Settings), and platform integration (tray, hotkeys, notifications, ringtones). |
| `updater` | [`updater.md`](crates/updater.md) | Self-update: checks GitHub Releases, verifies a SHA-256 checksum, and swaps in the new binary. |

Cutting across the UI layer: [`i18n.md`](crates/i18n.md) documents `ui/src/strings.rs`'s
locale/lookup infrastructure.

## Reading order

New to the codebase? `sip-core.md` and `media-engine.md` together cover what a call
actually is and how its media flows — read those first. `config.md` covers the data
every other crate reads from. `ui.md` ties it all together into the actual
application. `nat.md`, `updater.md`, and `i18n.md` are self-contained side reads,
useful whenever you're actually touching that piece.
