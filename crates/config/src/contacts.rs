use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::db::{bool_to_sql, sql_int_to_bool};
use crate::Db;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Contact {
    pub name:    String,
    pub sip_uri: String,
    /// Subscribe to this contact's SIP presence (RFC 3856), shown as a
    /// colored dot in the Contacts tab. Off by default -- opt-in, like the
    /// other watch/enable toggles in this config.
    #[serde(default)]
    pub watch_presence: bool,
    /// Which account's identity subscribes on this contact's behalf,
    /// identified by username (stable across account reordering, unlike an
    /// index). `None` defaults to the first enabled account.
    #[serde(default)]
    pub presence_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContactBook {
    pub contacts: Vec<Contact>,
}

impl ContactBook {
    pub fn load(db: &Db) -> anyhow::Result<Self> {
        let mut stmt = db.conn.prepare(
            "SELECT name, sip_uri, watch_presence, presence_account FROM contacts ORDER BY id",
        )?;
        let contacts = stmt
            .query_map([], |row| {
                Ok(Contact {
                    name: row.get(0)?,
                    sip_uri: row.get(1)?,
                    watch_presence: sql_int_to_bool(row.get(2)?),
                    presence_account: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading contacts from database")?;
        Ok(ContactBook { contacts })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.conn.execute("DELETE FROM contacts", []).context("Clearing contacts table")?;
        for c in &self.contacts {
            db.conn.execute(
                "INSERT INTO contacts (name, sip_uri, watch_presence, presence_account) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![c.name, c.sip_uri, bool_to_sql(c.watch_presence), c.presence_account],
            ).with_context(|| format!("Inserting contact {}", c.name))?;
        }
        Ok(())
    }

    /// Contacts whose name or URI contains `query` (case-insensitive), paired
    /// with their index in `self.contacts` so callers can edit/delete them.
    pub fn search<'a>(&'a self, query: &str) -> Vec<(usize, &'a Contact)> {
        let q = query.to_lowercase();
        self.contacts
            .iter()
            .enumerate()
            .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q) || c.sip_uri.to_lowercase().contains(&q))
            .collect()
    }
}
