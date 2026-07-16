use super::*;

fn zid(byte: u8) -> [u8; 12] {
    [byte; 12]
}

fn secrets(tag: u8) -> RetainedSecrets {
    RetainedSecrets { rs1: vec![tag; 32], rs2: vec![tag.wrapping_add(1); 32], verified: false }
}

#[test]
fn store_then_load_round_trips() {
    let mut store = MemorySharedSecretStore::default();
    let entry = CacheEntry { local_zid: zid(1), remote_zid: zid(2), secrets: secrets(0xAA) };
    store.store(entry.clone());
    let loaded = store.load(zid(1), zid(2)).expect("entry should be found");
    assert_eq!(loaded, entry);
}

#[test]
fn load_missing_entry_returns_none() {
    let store = MemorySharedSecretStore::default();
    assert_eq!(store.load(zid(1), zid(2)), None);
}

#[test]
fn entries_are_keyed_by_the_full_local_remote_zid_pair() {
    let mut store = MemorySharedSecretStore::default();
    let entry_a = CacheEntry { local_zid: zid(1), remote_zid: zid(2), secrets: secrets(1) };
    let entry_b = CacheEntry { local_zid: zid(1), remote_zid: zid(3), secrets: secrets(2) };
    store.store(entry_a.clone());
    store.store(entry_b.clone());

    assert_eq!(store.load(zid(1), zid(2)), Some(entry_a));
    assert_eq!(store.load(zid(1), zid(3)), Some(entry_b));
    // Swapping local/remote must not accidentally hit the other peer's entry.
    assert_eq!(store.load(zid(2), zid(1)), None);
}

#[test]
fn storing_again_for_the_same_pair_overwrites_the_previous_entry() {
    let mut store = MemorySharedSecretStore::default();
    store.store(CacheEntry { local_zid: zid(1), remote_zid: zid(2), secrets: secrets(1) });
    let updated = CacheEntry { local_zid: zid(1), remote_zid: zid(2), secrets: secrets(2) };
    store.store(updated.clone());

    assert_eq!(store.load(zid(1), zid(2)), Some(updated));
}

#[test]
fn clear_removes_only_the_targeted_entry() {
    let mut store = MemorySharedSecretStore::default();
    store.store(CacheEntry { local_zid: zid(1), remote_zid: zid(2), secrets: secrets(1) });
    store.store(CacheEntry { local_zid: zid(1), remote_zid: zid(3), secrets: secrets(2) });

    store.clear(zid(1), zid(2));

    assert_eq!(store.load(zid(1), zid(2)), None);
    assert!(store.load(zid(1), zid(3)).is_some(), "the other peer's entry must survive");
}

#[test]
fn clear_on_missing_entry_is_a_harmless_no_op() {
    let mut store = MemorySharedSecretStore::default();
    // Must not panic.
    store.clear(zid(9), zid(9));
}

#[test]
fn verified_flag_is_preserved_through_store_and_load() {
    let mut store = MemorySharedSecretStore::default();
    let entry = CacheEntry {
        local_zid: zid(4),
        remote_zid: zid(5),
        secrets: RetainedSecrets { rs1: vec![1], rs2: vec![2], verified: true },
    };
    store.store(entry.clone());
    let loaded = store.load(zid(4), zid(5)).unwrap();
    assert!(loaded.secrets.verified);
}
