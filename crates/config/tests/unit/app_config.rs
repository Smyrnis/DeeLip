use super::*;

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("deelip-config-test-app-config-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn default_config_has_one_account_and_documented_toggle_defaults() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.accounts.len(), 1);
    assert!(cfg.dark_mode);
    assert!(cfg.notifications_enabled);
    assert!(cfg.ringtone_enabled);
    assert!(cfg.crash_reporting_enabled);
    assert!(!cfg.recording_enabled);
    assert!(!cfg.ice_enabled);
    assert!(!cfg.single_call_mode);
    assert_eq!(cfg.stun_server.as_deref(), Some("stun.l.google.com:19302"));
    assert!(cfg.turn_server.is_none());
    assert!(cfg.zrtp_zid.is_none());
    assert_eq!(cfg.recording_format, RecordingFormat::Wav);
    assert_eq!(cfg.update_check_frequency, UpdateCheckFrequency::Always);
    assert_eq!(cfg.default_list_action, DefaultListAction::Call);
}

#[test]
fn zrtp_zid_bytes_generates_and_persists_on_first_use() {
    let path = temp_db_path("gen");
    let db = Db::open_at(&path).unwrap();
    let mut cfg = AppConfig::default();
    assert!(cfg.zrtp_zid.is_none());

    let bytes = cfg.zrtp_zid_bytes(&db).unwrap();
    assert_eq!(bytes.len(), 12);
    assert!(cfg.zrtp_zid.is_some());
    assert_eq!(db.get_setting("zrtp_zid"), cfg.zrtp_zid);
    std::fs::remove_file(&path).ok();
}

#[test]
fn zrtp_zid_bytes_is_stable_across_repeated_calls() {
    let path = temp_db_path("stable");
    let db = Db::open_at(&path).unwrap();
    let mut cfg = AppConfig::default();
    let first = cfg.zrtp_zid_bytes(&db).unwrap();
    let second = cfg.zrtp_zid_bytes(&db).unwrap();
    assert_eq!(first, second);
    std::fs::remove_file(&path).ok();
}

#[test]
fn zrtp_zid_bytes_parses_existing_valid_hex_without_regenerating() {
    let path = temp_db_path("valid-hex");
    let db = Db::open_at(&path).unwrap();
    let mut cfg = AppConfig { zrtp_zid: Some("0123456789abcdef00112233".to_string()), ..AppConfig::default() };

    let bytes = cfg.zrtp_zid_bytes(&db).unwrap();
    assert_eq!(bytes, [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x00, 0x11, 0x22, 0x33]);
    // Existing valid hex is used as-is -- nothing gets (re)written to the db.
    assert_eq!(db.get_setting("zrtp_zid"), None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn zrtp_zid_bytes_regenerates_on_non_hex_characters() {
    let path = temp_db_path("bad-chars");
    let db = Db::open_at(&path).unwrap();
    let mut cfg = AppConfig { zrtp_zid: Some("not-valid-hex-string!!!!".to_string()), ..AppConfig::default() };

    let bytes = cfg.zrtp_zid_bytes(&db).unwrap();
    assert_eq!(bytes.len(), 12);
    assert_ne!(cfg.zrtp_zid.as_deref(), Some("not-valid-hex-string!!!!"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn zrtp_zid_bytes_regenerates_on_wrong_length_hex() {
    let path = temp_db_path("bad-len");
    let db = Db::open_at(&path).unwrap();
    // Valid hex, but not 24 chars / 12 bytes.
    let mut cfg = AppConfig { zrtp_zid: Some("0011".to_string()), ..AppConfig::default() };

    let bytes = cfg.zrtp_zid_bytes(&db).unwrap();
    assert_eq!(bytes.len(), 12);
    assert_ne!(cfg.zrtp_zid.as_deref(), Some("0011"));
    std::fs::remove_file(&path).ok();
}
