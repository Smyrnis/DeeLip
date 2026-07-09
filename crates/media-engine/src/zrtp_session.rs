//! Media-engine side of ZRTP (RFC 6189): drives `deelip_sip::zrtp::ZrtpEngine`
//! for one call's RTP socket -- retransmitting our own last-sent handshake
//! message on packet loss, persisting retained secrets in the same SQLite
//! database as the rest of DeeLip's config, and translating engine events
//! into what `engine.rs`'s RTP loop needs to act on (send bytes, swap in
//! SRTP keys, surface the SAS).
//!
//! Retransmission here is a flat retry (`RESEND_INTERVAL` apart, up to
//! `MAX_ATTEMPTS`) rather than RFC 6189's own exponential-backoff schedule
//! -- simpler, and this implementation's own tests are the only thing that
//! have ever exercised it (see `deelip_sip::zrtp`'s module docs for why).

use std::path::Path;
use std::time::{Duration, Instant};

use deelip_sip::zrtp::{
    CacheEntry, EngineEvent, RetainedSecrets, Role, SharedSecretStore, ZrtpEngine,
};

const RESEND_INTERVAL: Duration = Duration::from_millis(300);
const MAX_ATTEMPTS: u32 = 15;

pub struct SqliteSecretStore {
    conn: rusqlite::Connection,
}

impl SqliteSecretStore {
    /// Opens its own connection to the same `deelip.db` the rest of the app
    /// uses (schema owned by `deelip_config::db`, which always creates the
    /// `zrtp_cache` table before any call could reach this code) rather
    /// than threading a live `Db` handle from the `ui` crate into this
    /// RTP-loop task.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        Ok(Self { conn })
    }
}

fn zid_hex(zid: [u8; 12]) -> String {
    zid.iter().map(|b| format!("{b:02x}")).collect()
}

impl SharedSecretStore for SqliteSecretStore {
    fn load(&self, local_zid: [u8; 12], remote_zid: [u8; 12]) -> Option<CacheEntry> {
        self.conn
            .query_row(
                "SELECT rs1, rs2, verified FROM zrtp_cache WHERE local_zid = ?1 AND remote_zid = ?2",
                rusqlite::params![zid_hex(local_zid), zid_hex(remote_zid)],
                |row| {
                    let rs1: Vec<u8> = row.get(0)?;
                    let rs2: Vec<u8> = row.get(1)?;
                    let verified: i64 = row.get(2)?;
                    Ok(CacheEntry {
                        local_zid,
                        remote_zid,
                        secrets: RetainedSecrets {
                            rs1,
                            rs2,
                            verified: verified != 0,
                        },
                    })
                },
            )
            .ok()
    }

    fn store(&mut self, entry: CacheEntry) {
        let _ = self.conn.execute(
            "INSERT INTO zrtp_cache (local_zid, remote_zid, rs1, rs2, verified) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(local_zid, remote_zid) DO UPDATE SET \
                rs1 = excluded.rs1, rs2 = excluded.rs2, verified = excluded.verified",
            rusqlite::params![
                zid_hex(entry.local_zid),
                zid_hex(entry.remote_zid),
                entry.secrets.rs1,
                entry.secrets.rs2,
                entry.secrets.verified as i64,
            ],
        );
    }

    fn clear(&mut self, local_zid: [u8; 12], remote_zid: [u8; 12]) {
        let _ = self.conn.execute(
            "DELETE FROM zrtp_cache WHERE local_zid = ?1 AND remote_zid = ?2",
            rusqlite::params![zid_hex(local_zid), zid_hex(remote_zid)],
        );
    }
}

pub enum ZrtpOutcome {
    SendBytes(Vec<u8>),
    Sas(String),
    Secure {
        srtp_key_i: [u8; 16],
        srtp_salt_i: [u8; 14],
        srtp_key_r: [u8; 16],
        srtp_salt_r: [u8; 14],
        role: Role,
    },
    Failed(String),
}

pub struct ZrtpRuntime {
    engine: ZrtpEngine<SqliteSecretStore>,
    pending_resend: Option<(Vec<u8>, u32)>,
    next_resend_at: Instant,
}

impl ZrtpRuntime {
    pub fn new(
        role: Role,
        local_zid: [u8; 12],
        client_id: [u8; 16],
        db_path: &Path,
    ) -> anyhow::Result<(Self, Vec<ZrtpOutcome>)> {
        let store = SqliteSecretStore::open(db_path)?;
        let mut engine = ZrtpEngine::new(role, local_zid, client_id, store);
        let events = engine.start();
        let mut runtime = Self {
            engine,
            pending_resend: None,
            next_resend_at: Instant::now() + RESEND_INTERVAL,
        };
        let outcomes = runtime.translate(events);
        Ok((runtime, outcomes))
    }

    pub fn handle_incoming(&mut self, bytes: &[u8]) -> Vec<ZrtpOutcome> {
        match self.engine.receive(bytes) {
            Ok(events) => self.translate(events),
            // A malformed/tampered packet on an otherwise-live handshake
            // isn't fatal by itself -- our own retransmit timer will just
            // keep resending the last message until the peer sends a valid
            // one, same as if this packet had been dropped entirely.
            Err(_) => Vec::new(),
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<ZrtpOutcome> {
        let Some((bytes, attempts)) = &mut self.pending_resend else {
            return Vec::new();
        };
        if now < self.next_resend_at {
            return Vec::new();
        }
        if *attempts >= MAX_ATTEMPTS {
            self.pending_resend = None;
            return vec![ZrtpOutcome::Failed("ZRTP handshake timed out".into())];
        }
        *attempts += 1;
        self.next_resend_at = now + RESEND_INTERVAL;
        vec![ZrtpOutcome::SendBytes(bytes.clone())]
    }

    fn translate(&mut self, events: Vec<EngineEvent>) -> Vec<ZrtpOutcome> {
        let mut out = Vec::with_capacity(events.len());
        for event in events {
            match event {
                EngineEvent::Send(msg) => {
                    let bytes = msg.encode();
                    self.pending_resend = Some((bytes.clone(), 0));
                    self.next_resend_at = Instant::now() + RESEND_INTERVAL;
                    out.push(ZrtpOutcome::SendBytes(bytes));
                }
                EngineEvent::SasReady(sas) => out.push(ZrtpOutcome::Sas(sas)),
                EngineEvent::SecureOn(session) => {
                    self.pending_resend = None;
                    out.push(ZrtpOutcome::Secure {
                        srtp_key_i: session.srtp_key_i,
                        srtp_salt_i: session.srtp_salt_i,
                        srtp_key_r: session.srtp_key_r,
                        srtp_salt_r: session.srtp_salt_r,
                        role: session.role,
                    });
                }
                EngineEvent::Failed(reason) => {
                    self.pending_resend = None;
                    out.push(ZrtpOutcome::Failed(reason));
                }
            }
        }
        out
    }
}

/// Parameters needed to start a ZRTP session for one call leg -- everything
/// `engine.rs`'s recv task needs to construct a `ZrtpRuntime` once the RTP
/// socket exists. `role` maps directly from which side sent the original
/// INVITE (see `deelip_sip::zrtp::engine`'s module doc): the SIP caller is
/// `Role::Initiator`, the callee `Role::Responder`.
#[derive(Clone)]
pub struct ZrtpParams {
    pub role: Role,
    pub local_zid: [u8; 12],
}

/// A 16-byte ASCII client identifier for our own Hello -- space-padded,
/// matching `Hello::client_id`'s fixed size.
pub fn client_id() -> [u8; 16] {
    let mut id = [b' '; 16];
    let name = b"DeeLip";
    id[..name.len()].copy_from_slice(name);
    id
}
