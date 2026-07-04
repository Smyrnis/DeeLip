//! SQLite-backed storage for everything DeeLip used to keep in
//! `config.toml`/`contacts.json`/`history.json` -- one `deelip.db` file
//! instead of three hand-written flat files. `AppConfig`/`ContactBook`/
//! `CallHistory` (defined in `lib.rs`) stay the same in-memory structs;
//! only their `load`/`save` methods go through here now.

use std::path::PathBuf;

use anyhow::Context;
use rusqlite::Connection;

use crate::{deelip_dir, AppConfig, CallHistory, ContactBook};

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
    mailbox                   TEXT
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

        let conn = Connection::open(&path)
            .with_context(|| format!("Opening database at {}", path.display()))?;
        conn.execute_batch(SCHEMA).context("Creating database schema")?;
        let db = Db { conn };

        if is_fresh {
            db.migrate_legacy_or_seed_default()?;
        }
        Ok(db)
    }

    fn migrate_legacy_or_seed_default(&self) -> anyhow::Result<()> {
        let dir = deelip_dir()?;
        let mut migrated_anything = false;

        let legacy_config = dir.join("config.toml");
        if let Ok(raw) = std::fs::read_to_string(&legacy_config) {
            if let Ok(cfg) = toml::from_str::<AppConfig>(&raw) {
                cfg.save(self)?;
                migrated_anything = true;
            }
        }

        let legacy_contacts = dir.join("contacts.json");
        if let Ok(raw) = std::fs::read_to_string(&legacy_contacts) {
            if let Ok(book) = serde_json::from_str::<ContactBook>(&raw) {
                book.save(self)?;
                migrated_anything = true;
            }
        }

        let legacy_history = dir.join("history.json");
        if let Ok(raw) = std::fs::read_to_string(&legacy_history) {
            if let Ok(hist) = serde_json::from_str::<CallHistory>(&raw) {
                hist.save(self)?;
                migrated_anything = true;
            }
        }

        if !migrated_anything {
            AppConfig::default().save(self)?;
        }
        Ok(())
    }

    pub(crate) fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0))
            .ok()
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
            None    => self.delete_setting(key),
        }
    }
}

/// Returns `~/.config/deelip/deelip.db`.
pub fn default_db_path() -> anyhow::Result<PathBuf> {
    Ok(deelip_dir()?.join("deelip.db"))
}

// ── Shared scalar<->SQL conversions ──────────────────────────────────────────
// Used by both `account.rs` (`SipAccount`) and `contacts.rs` (`Contact`).

pub(crate) fn bool_to_sql(b: bool) -> &'static str { if b { "1" } else { "0" } }
pub(crate) fn sql_to_bool(s: &str) -> bool { s == "1" }
/// Same as `sql_to_bool`, but for the `accounts`/`contacts` tables' `INTEGER`
/// columns -- SQLite's INTEGER column affinity silently converts a bound
/// numeric-looking TEXT value ("1"/"0") into actual INTEGER storage, so
/// those columns must be read back as `i64`, unlike `settings.value` (a
/// genuinely `TEXT`-affinity column, where "1"/"0" round-trips as text).
pub(crate) fn sql_int_to_bool(i: i64) -> bool { i != 0 }
