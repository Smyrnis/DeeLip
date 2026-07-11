use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::Db;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub peer_uri: String,
    pub direction: MessageDirection,
    pub body: String,
    /// Unix timestamp (seconds) when the message was sent/received.
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageLog {
    pub messages: Vec<Message>,
}

fn direction_to_str(d: &MessageDirection) -> &'static str {
    match d {
        MessageDirection::Inbound => "inbound",
        MessageDirection::Outbound => "outbound",
    }
}
fn direction_from_str(s: &str) -> MessageDirection {
    match s {
        "outbound" => MessageDirection::Outbound,
        _ => MessageDirection::Inbound,
    }
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
                    direction: direction_from_str(&direction_str),
                    body: row.get("body")?,
                    timestamp: row.get("timestamp")?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading messages from database")?;
        Ok(MessageLog { messages })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.conn.execute("DELETE FROM messages", []).context("Clearing messages table")?;
        // `messages` is newest-first (see `push`); capped at 200 there too,
        // so no separate truncation needed here.
        for m in &self.messages {
            db.conn
                .execute(
                    "INSERT INTO messages (peer_uri, direction, body, timestamp) \
                 VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![m.peer_uri, direction_to_str(&m.direction), m.body, m.timestamp],
                )
                .context("Inserting message")?;
        }
        Ok(())
    }

    /// Prepend a message, keeping at most 200 entries.
    pub fn push(&mut self, message: Message) {
        self.messages.insert(0, message);
        self.messages.truncate(200);
    }
}
