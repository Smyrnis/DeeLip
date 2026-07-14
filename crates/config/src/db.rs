//! SQLite-backed storage for everything DeeLip used to keep in
//! `config.toml`/`contacts.json`/`history.json` -- one `deelip.db` file
//! instead of three hand-written flat files. `AppConfig`/`ContactBook`/
//! `CallHistory` (defined in `lib.rs`) stay the same in-memory structs;
//! only their `load`/`save` methods go through here now.

use std::path::PathBuf;

use anyhow::Context;
use rusqlite::Connection;

use crate::{AppConfig, CallHistory, ContactBook, deelip_dir};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS accounts (
    id                        INTEGER PRIMARY KEY AUTOINCREMENT,
    sort_order                INTEGER NOT NULL,
    username                  TEXT NOT NULL,
    password                  TEXT NOT NULL,
    server                    TEXT NOT NULL,
    port                      INTEGER NOT NULL,
    display_name              TEXT,
    transport                 TEXT NOT NULL,
    enabled                   INTEGER NOT NULL,
    tls_insecure_skip_verify  INTEGER NOT NULL,
    no_answer_forward         TEXT,
    no_answer_timeout_secs    INTEGER NOT NULL,
    dnd                       INTEGER NOT NULL,
    forward_always            TEXT,
    forward_on_busy           TEXT,
    codec_order               TEXT NOT NULL,
    dtmf_mode                 TEXT NOT NULL,
    auto_answer_enabled       INTEGER NOT NULL,
    auto_answer_secs          INTEGER NOT NULL,
    mailbox                   TEXT,
    account_name              TEXT,
    sip_proxy                 TEXT,
    domain                    TEXT,
    auth_username             TEXT,
    dialing_prefix            TEXT,
    hide_caller_id            INTEGER NOT NULL DEFAULT 0,
    register_expires          INTEGER NOT NULL DEFAULT 3600,
    keepalive_secs            INTEGER,
    media_encryption          TEXT NOT NULL DEFAULT 'match_transport',
    public_address            TEXT,
    ice_enabled               INTEGER,
    force_incoming_codec      TEXT,
    vad_enabled               INTEGER NOT NULL DEFAULT 0,
    publish_presence          INTEGER NOT NULL DEFAULT 0,
    allow_ip_rewrite          INTEGER NOT NULL DEFAULT 0,
    dial_plan                 TEXT NOT NULL DEFAULT '[]',
    session_timers_enabled    INTEGER NOT NULL DEFAULT 1,
    auto_answer_control_button INTEGER NOT NULL DEFAULT 0,
    deny_incoming_control_button INTEGER NOT NULL DEFAULT 0,
    local_account              INTEGER NOT NULL DEFAULT 0,
    video_enabled              INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS contacts (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    name              TEXT NOT NULL,
    sip_uri           TEXT NOT NULL,
    watch_presence    INTEGER NOT NULL,
    presence_account  TEXT
);
CREATE TABLE IF NOT EXISTS call_history (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_uri     TEXT NOT NULL,
    direction      TEXT NOT NULL,
    timestamp      INTEGER NOT NULL,
    duration_secs  INTEGER NOT NULL,
    status         TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    peer_uri   TEXT NOT NULL,
    direction  TEXT NOT NULL,
    body       TEXT NOT NULL,
    timestamp  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS zrtp_cache (
    local_zid   TEXT NOT NULL,
    remote_zid  TEXT NOT NULL,
    rs1         BLOB NOT NULL,
    rs2         BLOB NOT NULL,
    verified    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (local_zid, remote_zid)
);
"#;

/// Handle to `~/.config/deelip/deelip.db`.
pub struct Db {
    pub(crate) conn: Connection,
}

impl Db {
    /// Opens (creating if necessary) the single DeeLip database, ensures the
    /// schema exists, and -- only the very first time this file is created --
    /// imports any existing legacy `config.toml`/`contacts.json`/
    /// `history.json` into it (left untouched on disk afterward), or seeds a
    /// single default account if there's no legacy data to import either.
    pub fn open_default() -> anyhow::Result<Self> {
        let path = default_db_path()?;
        let is_fresh = !path.exists();

        // `rusqlite::Connection::open` creates the *file* if missing but
        // never its parent directory -- on a genuinely fresh profile (no
        // prior DeeLip run, and nothing else has created `~/.config/deelip`
        // / `%APPDATA%\deelip` yet), this fails outright with "unable to
        // open database file" before any window can ever appear. This is
        // the very first thing `main()` does, before logging is even set
        // up, so that failure was otherwise silent on a console-subsystem
        // Windows build (main.rs has no `windows_subsystem = "windows"`).
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("Creating config dir {}", parent.display()))?;
        }
        let conn = Connection::open(&path).with_context(|| format!("Opening database at {}", path.display()))?;
        conn.execute_batch(SCHEMA).context("Creating database schema")?;
        let db = Db { conn };
        db.migrate_accounts_columns().context("Migrating accounts table columns")?;

        if is_fresh {
            db.migrate_legacy_or_seed_default()?;
        }
        Ok(db)
    }

    /// Idempotent `ALTER TABLE ADD COLUMN` for anything `SCHEMA` expects but
    /// an existing `accounts` table predates -- adding a column here always
    /// needs a matching entry in `SCHEMA` too. Full picture: `docs/crates/config.md`.
    ///
    /// Checks `PRAGMA table_info` once up front and only issues `ALTER
    /// TABLE` for columns that are actually missing, rather than
    /// unconditionally attempting all of them and string-matching
    /// "duplicate column name" errors to ignore the ones that already
    /// exist -- on every single startup (not just a fresh database), that
    /// used to mean this many round-trips regardless. Also more robust
    /// than the error-text match it replaces (SQLite's exact wording isn't
    /// a stable API to depend on); a real existence check needs no
    /// separately-maintained version number a future column addition
    /// could forget to bump either.
    fn migrate_accounts_columns(&self) -> anyhow::Result<()> {
        const COLUMNS: &[&str] = &[
            "account_name   TEXT",
            "sip_proxy      TEXT",
            "domain         TEXT",
            "auth_username  TEXT",
            "dialing_prefix TEXT",
            "hide_caller_id INTEGER NOT NULL DEFAULT 0",
            "register_expires INTEGER NOT NULL DEFAULT 3600",
            "keepalive_secs INTEGER",
            "media_encryption TEXT NOT NULL DEFAULT 'match_transport'",
            "public_address TEXT",
            "ice_enabled INTEGER",
            "force_incoming_codec TEXT",
            "vad_enabled INTEGER NOT NULL DEFAULT 0",
            "publish_presence INTEGER NOT NULL DEFAULT 0",
            "allow_ip_rewrite INTEGER NOT NULL DEFAULT 0",
            "dial_plan TEXT NOT NULL DEFAULT '[]'",
            "session_timers_enabled INTEGER NOT NULL DEFAULT 1",
            "auto_answer_control_button INTEGER NOT NULL DEFAULT 0",
            "deny_incoming_control_button INTEGER NOT NULL DEFAULT 0",
            "local_account INTEGER NOT NULL DEFAULT 0",
            "video_enabled INTEGER NOT NULL DEFAULT 0",
        ];
        let mut existing = std::collections::HashSet::new();
        let mut stmt = self.conn.prepare("PRAGMA table_info(accounts)")?;
        for name in stmt.query_map([], |row| row.get::<_, String>(1))? {
            existing.insert(name?);
        }

        for col in COLUMNS {
            let col_name = col.split_whitespace().next().unwrap_or(col);
            if existing.contains(col_name) {
                continue;
            }
            self.conn.execute(&format!("ALTER TABLE accounts ADD COLUMN {col}"), [])?;
        }
        Ok(())
    }

    fn migrate_legacy_or_seed_default(&self) -> anyhow::Result<()> {
        let dir = deelip_dir()?;
        let mut migrated_anything = false;

        let legacy_config = dir.join("config.toml");
        if let Ok(raw) = std::fs::read_to_string(&legacy_config)
            && let Ok(cfg) = toml::from_str::<AppConfig>(&raw)
        {
            cfg.save(self)?;
            migrated_anything = true;
        }

        let legacy_contacts = dir.join("contacts.json");
        if let Ok(raw) = std::fs::read_to_string(&legacy_contacts)
            && let Ok(book) = serde_json::from_str::<ContactBook>(&raw)
        {
            book.save(self)?;
            migrated_anything = true;
        }

        let legacy_history = dir.join("history.json");
        if let Ok(raw) = std::fs::read_to_string(&legacy_history)
            && let Ok(hist) = serde_json::from_str::<CallHistory>(&raw)
        {
            hist.save(self)?;
            migrated_anything = true;
        }

        if !migrated_anything {
            AppConfig::default().save(self)?;
        }
        Ok(())
    }

    pub(crate) fn get_setting(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0)).ok()
    }

    pub(crate) fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub(crate) fn delete_setting(&self, key: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Set-or-clear an `Option<String>` setting in one call -- `None` removes
    /// the row entirely so `get_setting` correctly returns `None` back.
    pub(crate) fn set_setting_opt(&self, key: &str, value: &Option<String>) -> anyhow::Result<()> {
        match value {
            Some(v) => self.set_setting(key, v),
            None => self.delete_setting(key),
        }
    }

    /// Shared by `CallHistory::save`/`ContactBook::save`/`MessageLog::save`:
    /// wraps `DELETE FROM <table>` + `insert_all`'s row-by-row inserts in
    /// one transaction, committed once at the end. Each of those three used
    /// to run its own delete-then-reinsert as separate autocommit
    /// statements -- up to ~200 individual synchronous disk writes (a full
    /// fsync each, by default) per save, all on the render thread. `table`
    /// is always a hardcoded literal from one of the three call sites,
    /// never user input, so interpolating it into the DELETE is safe here.
    /// Uses `unchecked_transaction` (not the usual `&mut self`-taking
    /// `transaction()`) since every `save()` caller only ever holds `&Db`.
    pub(crate) fn replace_all_in_transaction(
        &self, table: &str, insert_all: impl FnOnce(&rusqlite::Transaction) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction().with_context(|| format!("Starting transaction for {table}"))?;
        tx.execute(&format!("DELETE FROM {table}"), []).with_context(|| format!("Clearing {table} table"))?;
        insert_all(&tx)?;
        tx.commit().with_context(|| format!("Committing {table} transaction"))?;
        Ok(())
    }
}

/// Returns `~/.config/deelip/deelip.db`.
pub fn default_db_path() -> anyhow::Result<PathBuf> {
    Ok(deelip_dir()?.join("deelip.db"))
}

// ── Shared scalar<->SQL conversions ──────────────────────────────────────────
// Used by both `account.rs` (`SipAccount`) and `contacts.rs` (`Contact`).

pub(crate) fn bool_to_sql(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}
pub(crate) fn sql_to_bool(s: &str) -> bool {
    s == "1"
}
/// Same as `sql_to_bool`, but for the `accounts`/`contacts` tables' `INTEGER`
/// columns -- SQLite's INTEGER column affinity silently converts a bound
/// numeric-looking TEXT value ("1"/"0") into actual INTEGER storage, so
/// those columns must be read back as `i64`, unlike `settings.value` (a
/// genuinely `TEXT`-affinity column, where "1"/"0" round-trips as text).
pub(crate) fn sql_int_to_bool(i: i64) -> bool {
    i != 0
}
