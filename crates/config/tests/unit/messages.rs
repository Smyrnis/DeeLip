use super::*;

fn msg(peer: &str, body: &str, ts: u64, dir: Direction) -> Message {
    Message { peer_uri: peer.to_string(), direction: dir, body: body.to_string(), timestamp: ts }
}

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("deelip-config-test-messages-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn push_prepends_newest_first() {
    let mut log = MessageLog::default();
    log.push(msg("sip:a@x", "hi", 1, Direction::Outbound));
    log.push(msg("sip:b@x", "hey", 2, Direction::Inbound));
    assert_eq!(log.messages[0].peer_uri, "sip:b@x");
    assert_eq!(log.messages[1].peer_uri, "sip:a@x");
}

#[test]
fn push_caps_at_200_messages_dropping_the_oldest() {
    let mut log = MessageLog::default();
    for i in 0..205u64 {
        log.push(msg(&format!("peer{i}"), "body", i, Direction::Outbound));
    }
    assert_eq!(log.messages.len(), 200);
    assert_eq!(log.messages[0].peer_uri, "peer204");
    assert_eq!(log.messages[199].peer_uri, "peer5");
}

#[test]
fn save_then_load_round_trips_every_field_ordered_by_timestamp_desc() {
    let path = temp_db_path("roundtrip");
    let db = Db::open_at(&path).unwrap();
    let log = MessageLog {
        messages: vec![
            msg("sip:alice@x", "hello", 500, Direction::Inbound),
            msg("sip:bob@x", "hi there", 600, Direction::Outbound),
        ],
    };
    log.save(&db).unwrap();

    let loaded = MessageLog::load(&db).unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].peer_uri, "sip:bob@x");
    assert_eq!(loaded.messages[0].body, "hi there");
    assert_eq!(loaded.messages[0].direction, Direction::Outbound);
    assert_eq!(loaded.messages[0].timestamp, 600);
    assert_eq!(loaded.messages[1].peer_uri, "sip:alice@x");
    assert_eq!(loaded.messages[1].direction, Direction::Inbound);
    std::fs::remove_file(&path).ok();
}

#[test]
fn save_replaces_rather_than_appends() {
    let path = temp_db_path("replace");
    let db = Db::open_at(&path).unwrap();
    MessageLog { messages: vec![msg("a", "1", 1, Direction::Outbound), msg("b", "2", 2, Direction::Outbound)] }
        .save(&db)
        .unwrap();
    MessageLog { messages: vec![msg("c", "3", 3, Direction::Outbound)] }.save(&db).unwrap();

    let loaded = MessageLog::load(&db).unwrap();
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].peer_uri, "c");
    std::fs::remove_file(&path).ok();
}

#[test]
fn load_caps_at_200_most_recent_rows_even_if_the_table_has_more() {
    let path = temp_db_path("cap");
    let db = Db::open_at(&path).unwrap();
    let log =
        MessageLog { messages: (0..250u64).map(|i| msg(&format!("peer{i}"), "b", i, Direction::Outbound)).collect() };
    log.save(&db).unwrap();

    let loaded = MessageLog::load(&db).unwrap();
    assert_eq!(loaded.messages.len(), 200);
    assert_eq!(loaded.messages[0].peer_uri, "peer249");
    std::fs::remove_file(&path).ok();
}
