use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::Db;
use crate::db::{bool_to_sql, sql_int_to_bool};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Contact {
    pub name: String,
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
        let mut stmt =
            db.conn.prepare("SELECT name, sip_uri, watch_presence, presence_account FROM contacts ORDER BY id")?;
        let contacts = stmt
            .query_map([], |row| {
                Ok(Contact {
                    name: row.get("name")?,
                    sip_uri: row.get("sip_uri")?,
                    watch_presence: sql_int_to_bool(row.get("watch_presence")?),
                    presence_account: row.get("presence_account")?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading contacts from database")?;
        Ok(ContactBook { contacts })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.replace_all_in_transaction("contacts", |tx| {
            for c in &self.contacts {
                tx.execute(
                    "INSERT INTO contacts (name, sip_uri, watch_presence, presence_account) \
                 VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![c.name, c.sip_uri, bool_to_sql(c.watch_presence), c.presence_account],
                )
                .with_context(|| format!("Inserting contact {}", c.name))?;
            }
            Ok(())
        })
    }

    /// The saved contact whose `sip_uri` matches `uri`, if any -- used to
    /// resolve a raw call-history/message URI to a display name. Compares
    /// `normalize_uri_for_match`'d forms rather than exact bytes, so a
    /// contact saved as `sip:600@127.0.0.1` still matches an incoming URI
    /// that only differs by case, an explicit default port, or a trailing
    /// `;param`.
    pub fn find_by_uri(&self, uri: &str) -> Option<&Contact> {
        let target = normalize_uri_for_match(uri);
        self.contacts.iter().find(|c| normalize_uri_for_match(&c.sip_uri) == target)
    }

    /// Contacts whose name or URI contains `query` (case-insensitive), paired
    /// with their index in `self.contacts` so callers can edit/delete them.
    pub fn search<'a>(&'a self, query: &str) -> Vec<(usize, &'a Contact)> {
        let q = query.to_lowercase();
        self.contacts
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                q.is_empty() || c.name.to_lowercase().contains(&q) || c.sip_uri.to_lowercase().contains(&q)
            })
            .collect()
    }
}

/// Normalize a SIP URI for `find_by_uri`'s comparison: lowercase, strip the
/// `sip:`/`sips:` scheme, drop everything from the first `;` onward
/// (tags/params), and drop an explicit default `:5060` port -- so two URIs
/// that only differ in case, params, or an explicit-vs-implied default port
/// are still recognized as the same contact.
fn normalize_uri_for_match(uri: &str) -> String {
    let lower = uri.trim().to_ascii_lowercase();
    let stripped = lower.strip_prefix("sip:").or_else(|| lower.strip_prefix("sips:")).unwrap_or(&lower);
    let before_params = stripped.split(';').next().unwrap_or(stripped);
    before_params.strip_suffix(":5060").unwrap_or(before_params).to_string()
}

#[cfg(test)]
#[path = "../tests/unit/contacts.rs"]
mod tests;
