use super::*;

fn record(uri: &str, ts: u64) -> CallRecord {
    CallRecord {
        remote_uri: uri.to_string(),
        direction: Direction::Outbound,
        timestamp: ts,
        duration_secs: 0,
        status: CallStatus::Missed,
    }
}

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("deelip-config-test-history-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn push_prepends_newest_first() {
    let mut hist = CallHistory::default();
    hist.push(record("a", 1));
    hist.push(record("b", 2));
    assert_eq!(hist.records[0].remote_uri, "b");
    assert_eq!(hist.records[1].remote_uri, "a");
}

#[test]
fn push_caps_at_200_records_dropping_the_oldest() {
    let mut hist = CallHistory::default();
    for i in 0..205u64 {
        hist.push(record(&format!("call{i}"), i));
    }
    assert_eq!(hist.records.len(), 200);
    assert_eq!(hist.records[0].remote_uri, "call204");
    assert_eq!(hist.records[199].remote_uri, "call5");
}

#[test]
fn save_then_load_round_trips_every_field_ordered_by_timestamp_desc() {
    let path = temp_db_path("roundtrip");
    let db = Db::open_at(&path).unwrap();
    let hist = CallHistory {
        records: vec![
            CallRecord {
                remote_uri: "sip:alice@x".into(),
                direction: Direction::Inbound,
                timestamp: 1000,
                duration_secs: 42,
                status: CallStatus::Answered,
            },
            CallRecord {
                remote_uri: "sip:bob@x".into(),
                direction: Direction::Outbound,
                timestamp: 900,
                duration_secs: 0,
                status: CallStatus::Missed,
            },
            CallRecord {
                remote_uri: "sip:carl@x".into(),
                direction: Direction::Outbound,
                timestamp: 800,
                duration_secs: 0,
                status: CallStatus::Rejected,
            },
            CallRecord {
                remote_uri: "sip:dan@x".into(),
                direction: Direction::Inbound,
                timestamp: 700,
                duration_secs: 0,
                status: CallStatus::Failed,
            },
        ],
    };
    hist.save(&db).unwrap();

    let loaded = CallHistory::load(&db).unwrap();
    assert_eq!(loaded.records.len(), 4);
    // `load` orders by timestamp DESC regardless of insertion order.
    assert_eq!(loaded.records[0].remote_uri, "sip:alice@x");
    assert_eq!(loaded.records[0].direction, Direction::Inbound);
    assert_eq!(loaded.records[0].duration_secs, 42);
    assert_eq!(loaded.records[0].status, CallStatus::Answered);
    assert_eq!(loaded.records[1].status, CallStatus::Missed);
    assert_eq!(loaded.records[2].status, CallStatus::Rejected);
    assert_eq!(loaded.records[3].status, CallStatus::Failed);
    std::fs::remove_file(&path).ok();
}

#[test]
fn load_caps_at_200_most_recent_rows_even_if_the_table_has_more() {
    // `save` has no truncation of its own -- it trusts `push`'s 200 cap. If
    // that invariant is ever violated (as here, on purpose), `load`'s own
    // `LIMIT 200` must still be the thing that protects a caller.
    let path = temp_db_path("cap");
    let db = Db::open_at(&path).unwrap();
    let hist = CallHistory { records: (0..250u64).map(|i| record(&format!("call{i}"), i)).collect() };
    hist.save(&db).unwrap();

    let loaded = CallHistory::load(&db).unwrap();
    assert_eq!(loaded.records.len(), 200);
    assert_eq!(loaded.records[0].remote_uri, "call249");
    std::fs::remove_file(&path).ok();
}
