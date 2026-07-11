use std::path::Path;
use std::time::{Duration, Instant};

use super::*;

fn open_test_store() -> SqliteSecretStore {
    let store = SqliteSecretStore::open(Path::new(":memory:")).unwrap();
    store
        .conn
        .execute_batch(
            "CREATE TABLE zrtp_cache (
                local_zid   TEXT NOT NULL,
                remote_zid  TEXT NOT NULL,
                rs1         BLOB NOT NULL,
                rs2         BLOB NOT NULL,
                verified    INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (local_zid, remote_zid)
            );",
        )
        .unwrap();
    store
}

#[test]
fn zid_hex_formats_as_lowercase_hex() {
    let zid = [0xDEu8, 0xAD, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF];
    assert_eq!(zid_hex(zid), "dead000000000000000000ff");
}

#[test]
fn client_id_is_deelip_space_padded_to_16_bytes() {
    let id = client_id();
    assert_eq!(id.len(), 16);
    assert_eq!(&id[..6], b"DeeLip");
    assert!(id[6..].iter().all(|&b| b == b' '));
}

#[test]
fn secret_store_load_returns_none_when_absent() {
    let store = open_test_store();
    assert!(store.load([1; 12], [2; 12]).is_none());
}

#[test]
fn secret_store_round_trips_and_clears() {
    let mut store = open_test_store();
    let local = [1u8; 12];
    let remote = [2u8; 12];
    let entry = CacheEntry {
        local_zid: local,
        remote_zid: remote,
        secrets: RetainedSecrets { rs1: vec![1, 2, 3], rs2: vec![4, 5, 6], verified: true },
    };
    store.store(entry);

    let loaded = store.load(local, remote).expect("just-stored entry");
    assert_eq!(loaded.secrets.rs1, vec![1, 2, 3]);
    assert_eq!(loaded.secrets.rs2, vec![4, 5, 6]);
    assert!(loaded.secrets.verified);

    store.clear(local, remote);
    assert!(store.load(local, remote).is_none());
}

#[test]
fn secret_store_upsert_overwrites_existing_entry() {
    let mut store = open_test_store();
    let local = [1u8; 12];
    let remote = [2u8; 12];
    store.store(CacheEntry {
        local_zid: local,
        remote_zid: remote,
        secrets: RetainedSecrets { rs1: vec![1], rs2: vec![2], verified: false },
    });
    store.store(CacheEntry {
        local_zid: local,
        remote_zid: remote,
        secrets: RetainedSecrets { rs1: vec![9], rs2: vec![9], verified: true },
    });

    let loaded = store.load(local, remote).unwrap();
    assert_eq!(loaded.secrets.rs1, vec![9]);
    assert!(loaded.secrets.verified);
}

#[test]
fn new_runtime_sends_hello_and_arms_the_resend_timer() {
    let (runtime, outcomes) = ZrtpRuntime::new(Role::Initiator, [3; 12], client_id(), Path::new(":memory:")).unwrap();
    assert!(matches!(outcomes.as_slice(), [ZrtpOutcome::SendBytes(_)]));
    assert!(runtime.pending_resend.is_some());
}

#[test]
fn tick_is_a_noop_before_the_resend_interval_elapses() {
    let (mut runtime, _) = ZrtpRuntime::new(Role::Initiator, [3; 12], client_id(), Path::new(":memory:")).unwrap();
    assert!(runtime.tick(Instant::now()).is_empty());
}

#[test]
fn tick_resends_after_the_interval_then_fails_past_max_attempts() {
    let (mut runtime, _) = ZrtpRuntime::new(Role::Initiator, [3; 12], client_id(), Path::new(":memory:")).unwrap();

    for attempt in 0..MAX_ATTEMPTS {
        // Force the timer to look elapsed without a real 300ms sleep.
        runtime.next_resend_at = Instant::now() - Duration::from_millis(1);
        let outcomes = runtime.tick(Instant::now());
        assert!(matches!(outcomes.as_slice(), [ZrtpOutcome::SendBytes(_)]), "expected a resend on attempt {attempt}");
    }

    runtime.next_resend_at = Instant::now() - Duration::from_millis(1);
    let outcomes = runtime.tick(Instant::now());
    assert!(matches!(outcomes.as_slice(), [ZrtpOutcome::Failed(_)]));
    assert!(runtime.pending_resend.is_none());
}

#[test]
fn handle_incoming_ignores_malformed_bytes() {
    let (mut runtime, _) = ZrtpRuntime::new(Role::Initiator, [3; 12], client_id(), Path::new(":memory:")).unwrap();
    assert!(runtime.handle_incoming(&[0xff; 4]).is_empty());
}
