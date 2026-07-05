use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::Db;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallStatus {
    Answered,
    Missed,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub remote_uri: String,
    pub direction: CallDirection,
    /// Unix timestamp (seconds) when the call was initiated/received.
    pub timestamp: u64,
    /// Duration in seconds; 0 for unanswered calls.
    pub duration_secs: u32,
    pub status: CallStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallHistory {
    pub records: Vec<CallRecord>,
}

fn call_direction_to_str(d: &CallDirection) -> &'static str {
    match d {
        CallDirection::Inbound => "inbound",
        CallDirection::Outbound => "outbound",
    }
}
fn call_direction_from_str(s: &str) -> CallDirection {
    match s {
        "outbound" => CallDirection::Outbound,
        _ => CallDirection::Inbound,
    }
}

fn call_status_to_str(s: &CallStatus) -> &'static str {
    match s {
        CallStatus::Answered => "answered",
        CallStatus::Missed => "missed",
        CallStatus::Rejected => "rejected",
        CallStatus::Failed => "failed",
    }
}
fn call_status_from_str(s: &str) -> CallStatus {
    match s {
        "answered" => CallStatus::Answered,
        "rejected" => CallStatus::Rejected,
        "failed" => CallStatus::Failed,
        _ => CallStatus::Missed,
    }
}

impl CallHistory {
    pub fn load(db: &Db) -> anyhow::Result<Self> {
        let mut stmt = db.conn.prepare(
            "SELECT remote_uri, direction, timestamp, duration_secs, status \
             FROM call_history ORDER BY timestamp DESC LIMIT 200",
        )?;
        let records = stmt
            .query_map([], |row| {
                let direction_str: String = row.get(1)?;
                let status_str: String = row.get(4)?;
                Ok(CallRecord {
                    remote_uri: row.get(0)?,
                    direction: call_direction_from_str(&direction_str),
                    timestamp: row.get(2)?,
                    duration_secs: row.get(3)?,
                    status: call_status_from_str(&status_str),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading call history from database")?;
        Ok(CallHistory { records })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.conn
            .execute("DELETE FROM call_history", [])
            .context("Clearing call_history table")?;
        // `records` is newest-first (see `push`); capped at 200 there too, so
        // no separate truncation needed here.
        for r in &self.records {
            db.conn.execute(
                "INSERT INTO call_history (remote_uri, direction, timestamp, duration_secs, status) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    r.remote_uri,
                    call_direction_to_str(&r.direction),
                    r.timestamp,
                    r.duration_secs,
                    call_status_to_str(&r.status),
                ],
            ).context("Inserting call history record")?;
        }
        Ok(())
    }

    /// Prepend a record, keeping at most 200 entries.
    pub fn push(&mut self, record: CallRecord) {
        self.records.insert(0, record);
        self.records.truncate(200);
    }
}
