use super::*;

fn temp_db_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("deelip-config-test-db-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn open_at_creates_all_expected_tables() {
    let path = temp_db_path("schema");
    let db = Db::open_at(&path).unwrap();
    let names: Vec<String> = {
        let mut stmt = db.conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap();
        stmt.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    for expected in ["accounts", "contacts", "call_history", "messages", "settings", "zrtp_cache"] {
        assert!(names.iter().any(|n| n == expected), "missing table {expected}, got {names:?}");
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn get_setting_returns_none_for_missing_key() {
    let path = temp_db_path("get-missing");
    let db = Db::open_at(&path).unwrap();
    assert_eq!(db.get_setting("nope"), None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn set_setting_then_get_setting_round_trips() {
    let path = temp_db_path("set-get");
    let db = Db::open_at(&path).unwrap();
    db.set_setting("dark_mode", "1").unwrap();
    assert_eq!(db.get_setting("dark_mode"), Some("1".to_string()));
    std::fs::remove_file(&path).ok();
}

#[test]
fn set_setting_upserts_without_duplicating_the_row() {
    let path = temp_db_path("upsert");
    let db = Db::open_at(&path).unwrap();
    db.set_setting("stun_server", "first.example.com").unwrap();
    db.set_setting("stun_server", "second.example.com").unwrap();
    assert_eq!(db.get_setting("stun_server"), Some("second.example.com".to_string()));
    let count: i64 =
        db.conn.query_row("SELECT COUNT(*) FROM settings WHERE key = 'stun_server'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
    std::fs::remove_file(&path).ok();
}

#[test]
fn delete_setting_removes_it() {
    let path = temp_db_path("delete");
    let db = Db::open_at(&path).unwrap();
    db.set_setting("turn_server", "turn.example.com").unwrap();
    db.delete_setting("turn_server").unwrap();
    assert_eq!(db.get_setting("turn_server"), None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn set_setting_opt_some_writes_and_none_deletes() {
    let path = temp_db_path("opt");
    let db = Db::open_at(&path).unwrap();
    db.set_setting_opt("ldap_server", &Some("ldap.example.com".to_string())).unwrap();
    assert_eq!(db.get_setting("ldap_server"), Some("ldap.example.com".to_string()));
    db.set_setting_opt("ldap_server", &None).unwrap();
    assert_eq!(db.get_setting("ldap_server"), None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn bool_sql_conversions_round_trip() {
    assert_eq!(bool_to_sql(true), "1");
    assert_eq!(bool_to_sql(false), "0");
    assert!(sql_to_bool("1"));
    assert!(!sql_to_bool("0"));
    assert!(!sql_to_bool("garbage"));
    assert!(sql_int_to_bool(1));
    assert!(!sql_int_to_bool(0));
    assert!(sql_int_to_bool(-7));
}

#[test]
fn default_db_path_points_at_deelip_db_file() {
    let path = default_db_path().unwrap();
    assert_eq!(path.file_name().unwrap(), "deelip.db");
    assert_eq!(path.parent().unwrap().file_name().unwrap(), "deelip");
}

#[test]
fn replace_all_in_transaction_atomically_replaces_rows() {
    let path = temp_db_path("replace-all");
    let db = Db::open_at(&path).unwrap();
    db.replace_all_in_transaction("contacts", |tx| {
        tx.execute(
            "INSERT INTO contacts (name, sip_uri, watch_presence, presence_account) VALUES ('A','sip:a',0,NULL)",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO contacts (name, sip_uri, watch_presence, presence_account) VALUES ('B','sip:b',0,NULL)",
            [],
        )
        .unwrap();
        Ok(())
    })
    .unwrap();
    let count_after_first: i64 = db.conn.query_row("SELECT COUNT(*) FROM contacts", [], |r| r.get(0)).unwrap();
    assert_eq!(count_after_first, 2);

    db.replace_all_in_transaction("contacts", |tx| {
        tx.execute(
            "INSERT INTO contacts (name, sip_uri, watch_presence, presence_account) VALUES ('C','sip:c',0,NULL)",
            [],
        )
        .unwrap();
        Ok(())
    })
    .unwrap();
    let count_after_second: i64 = db.conn.query_row("SELECT COUNT(*) FROM contacts", [], |r| r.get(0)).unwrap();
    assert_eq!(count_after_second, 1);
    let name: String = db.conn.query_row("SELECT name FROM contacts", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "C");
    std::fs::remove_file(&path).ok();
}
