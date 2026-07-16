//! Retained-secrets cache (RFC 6189 section 4.3.2's "rs1"/"rs2" continuity
//! mechanism): remembering a shared secret from a previous successful call
//! with the same peer lets later calls auto-verify (matching retained
//! secrets prove there was no MITM on *this* call either, without asking
//! the user to read out the SAS again every time).

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedSecrets {
    pub rs1: Vec<u8>,
    pub rs2: Vec<u8>,
    /// Whether the user has actually confirmed the SAS out loud with this
    /// peer at some point -- retained secrets alone only prove continuity
    /// with whoever we talked to *last* time, not that either call was
    /// actually MITM-free unless a human verified it at least once.
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    pub local_zid: [u8; 12],
    pub remote_zid: [u8; 12],
    pub secrets: RetainedSecrets,
}

pub trait SharedSecretStore {
    fn load(&self, local_zid: [u8; 12], remote_zid: [u8; 12]) -> Option<CacheEntry>;
    fn store(&mut self, entry: CacheEntry);
    fn clear(&mut self, local_zid: [u8; 12], remote_zid: [u8; 12]);
}

#[derive(Default)]
pub struct MemorySharedSecretStore {
    entries: HashMap<([u8; 12], [u8; 12]), CacheEntry>,
}

impl SharedSecretStore for MemorySharedSecretStore {
    fn load(&self, local_zid: [u8; 12], remote_zid: [u8; 12]) -> Option<CacheEntry> {
        self.entries.get(&(local_zid, remote_zid)).cloned()
    }
    fn store(&mut self, entry: CacheEntry) {
        self.entries.insert((entry.local_zid, entry.remote_zid), entry);
    }
    fn clear(&mut self, local_zid: [u8; 12], remote_zid: [u8; 12]) {
        self.entries.remove(&(local_zid, remote_zid));
    }
}

#[cfg(test)]
#[path = "../../tests/unit/zrtp/cache.rs"]
mod tests;
