use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::{Db, Direction};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub peer_uri: String,
    pub direction: Direction,
    pub body: String,
    /// Unix timestamp (seconds) when the message was sent/received.
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageLog {
    pub messages: Vec<Message>,
}

impl MessageLog {
    pub fn load(db: &Db) -> anyhow::Result<Self> {
        let mut stmt = db.conn.prepare(
            "SELECT peer_uri, direction, body, timestamp \
             FROM messages ORDER BY timestamp DESC LIMIT 200",
        )?;
        let messages = stmt
            .query_map([], |row| {
                let direction_str: String = row.get("direction")?;
                Ok(Message {
                    peer_uri: row.get("peer_uri")?,
                    direction: Direction::from_str(&direction_str),
                    body: row.get("body")?,
                    timestamp: row.get("timestamp")?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading messages from database")?;
        Ok(MessageLog { messages })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.replace_all_in_transaction("messages", |tx| {
            // `messages` is newest-first (see `push`); capped at 200 there
            // too, so no separate truncation needed here.
            for m in &self.messages {
                tx.execute(
                    "INSERT INTO messages (peer_uri, direction, body, timestamp) \
                 VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![m.peer_uri, m.direction.to_str(), m.body, m.timestamp],
                )
                .context("Inserting message")?;
            }
            Ok(())
        })
    }

    /// Prepend a message, keeping at most 200 entries.
    pub fn push(&mut self, message: Message) {
        self.messages.insert(0, message);
        self.messages.truncate(200);
    }
}
