# config (`crates/config`)

`deelip-config` owns every piece of state DeeLip persists between runs and every
other crate reads settings from: `AppConfig` (global + per-account settings),
`ContactBook`, `CallHistory`, `MessageLog`, dial-plan matching, and the per-OS
autostart integration (`autostart.rs` — XDG on Linux, a registry `Run` key on
Windows, a `LaunchAgent` plist on macOS). Every other crate in the workspace
(`sip-core`, `media-engine`, `ui`) depends on this one for its config types; this
crate depends on none of them.

## Architecture

All persistent state lives in one SQLite database, `~/.config/deelip/deelip.db`
(`db.rs`), opened once at startup via `Db::open_default()`. There is no `.toml`/
`.json` config file anymore — an earlier version of DeeLip stored `config.toml`/
`contacts.json`/`history.json` directly; `Db::open_default` still migrates any of
those found on disk into the database the very first time it's created (see
`migrate_legacy_or_seed_default`), then leaves the old files untouched afterward
(read but never deleted).

Five tables: `settings` (a flat key-value store — most of `AppConfig`'s fields live
here, one row per field, read/written via `Db::get_setting`/`set_setting`),
`accounts` (one row per `SipAccount`, this crate's one genuinely relational table —
see below), `contacts`, `call_history`, `messages`. `zrtp_cache` also lives in this
same database file but is owned and read/written entirely by
`media-engine::zrtp_session` (see `docs/crates/media-engine.md`) — this crate only creates
the table, never touches its rows.

Each of `account.rs`/`contacts.rs`/`history.rs`/`messages.rs` follows the same
`load(db: &Db) -> Result<Self>` / `save(&self, db: &Db) -> Result<()>` shape: `load`
reads by column *name* (`row.get("column_name")`, not a positional index) — an
earlier round of cleanup replaced positional `row.get(0)`/`row.get(1)`/... reads
after finding they're a silent-data-corruption risk: a `SELECT` column reorder would
shift every downstream index without `rusqlite` raising any error, since it happily
returns whatever type-compatible value sits at that position. Named lookups turn
that same mistake into a loud, immediate `Err` instead. `save` does a full
`DELETE` + re-`INSERT` of every row rather than a diffed update, which is simple and
fine at this data's scale (a handful of accounts, a few hundred history/message rows
each capped in `push`).

Config *enum* fields (`TransportProtocol`, `DtmfMode`, `MediaEncryption`,
`RecordingFormat`, `Language`, `UpdateCheckFrequency`, `DefaultListAction`) all follow
one pattern, worth matching exactly when adding a new one: a
`#[derive(..., Default, PartialEq, Eq)] enum` with `#[default]` on its default
variant, a pair of free functions `..._to_str`/`..._from_str` (not `Display`/`FromStr`
impls — kept as plain functions so call sites read `to_str(x)` symmetrically on both
the read and write side), a field on `AppConfig`/`SipAccount`, and explicit
load/save lines in `AppConfig::load`/`save` (`accounts` fields go through the
`accounts` table's named columns; everything else through `settings` via
`get`/`set_setting`).

An `Option<T>` field is frequently used as its own on/off toggle rather than pairing
a `bool` alongside it — `no_answer_forward`, `forward_always`, `forward_on_busy`,
`mailbox`, `ldap_server`, `keepalive_secs`, `ice_enabled` (per-account override) all
work this way: `None`/empty means "off, use the default behavior," `Some(_)` both
enables the feature and supplies its value in one field. Prefer this over a separate
bool when a feature's "on" state naturally needs a value anyway.

## Design decisions & invariants

**`accounts` table schema migration** (`db.rs::migrate_accounts_columns`): the table
is created once via `CREATE TABLE IF NOT EXISTS`, which does nothing on a database
that already has the table from before a column was added. Every column added to
`SCHEMA` after the table might already exist also gets an idempotent
`ALTER TABLE accounts ADD COLUMN ...` in `migrate_accounts_columns`, with SQLite's
"duplicate column name" error (its way of saying "already has it") swallowed and
anything else propagated — SQLite has no `ADD COLUMN IF NOT EXISTS` of its own, this
is the idiom that stands in for one. **Any future new `accounts` column needs an
entry in both places**: `SCHEMA`'s `CREATE TABLE` (for a genuinely fresh database)
and `COLUMNS` in `migrate_accounts_columns` (for an existing one) — forgetting the
second one means every existing install's `SELECT`/`INSERT` in `account.rs` starts
failing with "no such column" the moment they upgrade, even though a fresh install
would work fine.

**`local_account` (`SipAccount`)**: a serverless, direct-IP calling mode — place and
receive direct SIP calls to/from a bare IP with no registrar at all. No
`REGISTER` is ever sent; `server`/`password`/`auth_username` go unused since there's
nothing to authenticate to, while `username`/`display_name` still serve as this
account's caller-ID identity. Forced to UDP regardless of the account's own
`transport` setting: TCP/TLS need a real persistent connection to a live peer at
socket-creation time, which doesn't exist without a fixed server to connect to (see
`deelip_sip::client::SipStack::connect_local`). Outgoing calls resolve their
destination straight from the dialed target (an IP or hostname, with an optional
`:port`) instead of through an outbound proxy.

**ZRTP identity** (`AppConfig::zrtp_zid_bytes`): one 12-byte ZID per DeeLip
*installation* (not per-account), generated randomly on first use and persisted from
then on as a hex string in `zrtp_zid` — every account's ZRTP calls share it. See
`docs/crates/sip-core.md` for what the ZID is actually used for in the protocol itself.

## Known limitations / open items

- `video_enabled` (`SipAccount`) currently only controls whether a video leg gets
  *negotiated* in SDP; camera capture/encode/decode is driven entirely by
  `media-engine`/`ui`, not this crate — see `docs/crates/media-engine.md`'s video section
  for that side's actual current state (video calling is fully wired end to end as
  of this writing, this field just isn't literally a Settings checkbox in `ui` yet
  independent of `media-engine`'s own camera-picker UI).
- `Language` has exactly one variant (`En`) today — the load/save plumbing and
  `assets/locales/*.json` lookup path both exist and are exercised without a second
  translated locale to maintain yet. See `docs/crates/i18n.md` for the loading side of this
  in `ui`.
- `AudioConfig::sample_rate`/`frame_size_ms` are stored fields with no effect —
  audio is always captured/played at 8 kHz in 20ms frames regardless of their value.
