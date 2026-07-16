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

All three route their delete-then-reinsert through `Db::replace_all_in_transaction`,
which wraps it in one transaction committed at the end (via `unchecked_transaction`,
not the usual `&mut self`-taking `transaction()`, since every `save` caller only ever
holds `&Db`). Each of the three used to run its delete-then-reinsert as separate
autocommit statements — up to ~200 individual synchronous disk writes (a full fsync
each, by default) per save, all on the render thread — before being consolidated
into one transaction this way.

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
bool when a feature's "on" state naturally needs a value anyway. `ldap_server`'s
presence specifically is what enables the Directory tab in `ui` (`None`/empty shows a
"configure this in Settings" prompt instead of a search box).

`crash_reporting_enabled` (`AppConfig`) is on by default, unlike every other opt-in
toggle in this struct: there's no privacy cost to weigh since the crash report it
gates (`deelip_config::crashes_dir()`) is purely local and never uploaded or
transmitted anywhere, and a crash report is only useful if it was already enabled
*before* the crash happened. It's read once at startup to install the panic hook, so
it's restart-required like every other logging-adjacent setting.

## Design decisions & invariants

**`accounts` table schema migration** (`db.rs::migrate_accounts_columns`): the table
is created once via `CREATE TABLE IF NOT EXISTS`, which does nothing on a database
that already has the table from before a column was added. Every column added to
`SCHEMA` after the table might already exist also gets an entry in `COLUMNS`, and
`migrate_accounts_columns` checks `PRAGMA table_info(accounts)` once up front to see
which of those already exist, issuing an idempotent `ALTER TABLE accounts ADD
COLUMN ...` only for the ones actually missing — SQLite has no
`ADD COLUMN IF NOT EXISTS` of its own, this is the idiom that stands in for one. An
earlier version of this instead unconditionally attempted every `ALTER TABLE` and
string-matched SQLite's "duplicate column name" error to ignore the ones that
already existed; that meant this many round-trips on *every* startup, not just a
fresh database, and depended on SQLite's exact error wording, which isn't a stable
API to lean on. The `PRAGMA` check fixes both: only genuinely-missing columns get an
`ALTER TABLE` at all, and nothing depends on error-message text. **Any future new
`accounts` column needs an entry in both places**: `SCHEMA`'s `CREATE TABLE` (for a
genuinely fresh database) and `COLUMNS` in `migrate_accounts_columns` (for an
existing one) — forgetting the second one means every existing install's
`SELECT`/`INSERT` in `account.rs` starts failing with "no such column" the moment
they upgrade, even though a fresh install would work fine.

**Config directory creation before opening the database** (`Db::open_default`):
`rusqlite::Connection::open` creates the database *file* if it's missing, but never
its parent directory. On a genuinely fresh profile — no prior DeeLip run, and
nothing else has yet created `~/.config/deelip`/`%APPDATA%\deelip` — that used to
fail outright with "unable to open database file" before any window could ever
appear. This runs as the very first thing `main()` does, before logging is even set
up, so the failure was silent on a console-subsystem Windows build (`main.rs` has no
`windows_subsystem = "windows"`). `open_default` now `create_dir_all`s the parent
directory first, unconditionally, before ever calling `Connection::open`.

**Shared network-timeout constants** (`timeouts.rs`): previously each independently
defined (and independently named/valued, in two cases) across `crates/sip-core` and
`crates/ui`. Consolidated here since both crates already depend on `deelip-config` (a
true leaf crate, safe to depend on from anywhere, so consolidating shared constants
here adds no new dependency-direction risk). `crates/nat`'s own STUN/TURN timeouts
deliberately stay local instead of moving here too: `nat` has no other `deelip-*`
dependency today, and pulling one in just for a couple of numbers isn't worth the new
coupling for that otherwise standalone, WebRTC-only crate.

**Dial-plan rule engine** (`dialplan.rs`): `DialPlanRule` matching/replacement uses
the `regex` crate directly rather than a hand-rolled pattern language — it's already
present in the workspace's dependency tree transitively, and a real match/replace
engine is exactly what `regex` already is, so there's no reason to reinvent one.

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
