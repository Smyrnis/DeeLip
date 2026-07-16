use super::*;
use crate::{
    DefaultListAction, DialPlanRule, DtmfMode, MediaEncryption, RecordingFormat, TransportProtocol,
    UpdateCheckFrequency,
};

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("deelip-config-test-account-db-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// A `SipAccount` with as many non-default fields set as possible, `n`
/// folded into every string/number field so multiple instances are
/// trivially distinguishable after a round trip.
fn sample_account(n: u16) -> SipAccount {
    SipAccount {
        username: format!("user{n}"),
        password: format!("pass{n}"),
        server: format!("sip{n}.example.com"),
        port: 5060 + n,
        display_name: Some(format!("Display {n}")),
        transport: TransportProtocol::Tls,
        enabled: n.is_multiple_of(2),
        tls_insecure_skip_verify: true,
        no_answer_forward: Some(format!("sip:forward{n}@example.com")),
        no_answer_timeout_secs: 15 + n as u32,
        dnd: true,
        forward_always: Some("sip:always@example.com".into()),
        forward_on_busy: Some("sip:busy@example.com".into()),
        codec_order: vec!["g729".into(), "opus".into()],
        force_incoming_codec: Some("pcma".into()),
        vad_enabled: true,
        dtmf_mode: DtmfMode::SipInfo,
        auto_answer_enabled: true,
        auto_answer_secs: 7,
        auto_answer_control_button: true,
        deny_incoming_control_button: true,
        mailbox: Some("*97".into()),
        account_name: Some(format!("Nickname {n}")),
        sip_proxy: Some("proxy.example.com:5070".into()),
        domain: Some("domain.example.com".into()),
        auth_username: Some(format!("auth{n}")),
        dialing_prefix: Some("9".into()),
        dial_plan: vec![DialPlanRule { pattern: r"^0(\d+)$".into(), replacement: "$1".into(), enabled: true }],
        hide_caller_id: true,
        register_expires: 1800,
        keepalive_secs: Some(25),
        media_encryption: MediaEncryption::DtlsSrtp,
        public_address: Some("203.0.113.1".into()),
        allow_ip_rewrite: true,
        publish_presence: true,
        ice_enabled: Some(true),
        session_timers_enabled: false,
        local_account: true,
        video_enabled: true,
    }
}

#[test]
fn save_then_load_round_trips_every_field_single_account() {
    let path = temp_db_path("single");
    let db = Db::open_at(&path).unwrap();
    let cfg = AppConfig { accounts: vec![sample_account(1)], ..AppConfig::default() };
    cfg.save(&db).unwrap();

    let loaded = AppConfig::load(&db).unwrap();
    assert_eq!(loaded.accounts.len(), 1);
    assert_eq!(loaded.accounts[0], cfg.accounts[0]);
    std::fs::remove_file(&path).ok();
}

#[test]
fn save_then_load_round_trips_multiple_accounts_preserving_order() {
    let path = temp_db_path("multi");
    let db = Db::open_at(&path).unwrap();
    let cfg =
        AppConfig { accounts: vec![sample_account(1), sample_account(2), sample_account(3)], ..AppConfig::default() };
    cfg.save(&db).unwrap();

    let loaded = AppConfig::load(&db).unwrap();
    assert_eq!(loaded.accounts, cfg.accounts);
    std::fs::remove_file(&path).ok();
}

#[test]
fn save_replaces_previous_accounts_rather_than_appending() {
    let path = temp_db_path("replace");
    let db = Db::open_at(&path).unwrap();
    let mut cfg = AppConfig { accounts: vec![sample_account(1), sample_account(2)], ..AppConfig::default() };
    cfg.save(&db).unwrap();

    cfg.accounts = vec![sample_account(3)];
    cfg.save(&db).unwrap();

    let loaded = AppConfig::load(&db).unwrap();
    assert_eq!(loaded.accounts.len(), 1);
    assert_eq!(loaded.accounts[0].username, "user3");
    std::fs::remove_file(&path).ok();
}

#[test]
fn default_account_round_trips() {
    // Guards against the all-defaults case (mostly NULLs/0s) silently
    // masking a column mismatch that a heavily-populated account wouldn't.
    let path = temp_db_path("default");
    let db = Db::open_at(&path).unwrap();
    let cfg = AppConfig { accounts: vec![SipAccount::default()], ..AppConfig::default() };
    cfg.save(&db).unwrap();
    let loaded = AppConfig::load(&db).unwrap();
    assert_eq!(loaded.accounts[0], SipAccount::default());
    std::fs::remove_file(&path).ok();
}

#[test]
fn optional_ice_enabled_distinguishes_none_some_false_some_true() {
    let path = temp_db_path("ice-tri-state");
    let db = Db::open_at(&path).unwrap();
    let mut acc_none = sample_account(1);
    acc_none.ice_enabled = None;
    let mut acc_false = sample_account(2);
    acc_false.ice_enabled = Some(false);
    let mut acc_true = sample_account(3);
    acc_true.ice_enabled = Some(true);

    let cfg = AppConfig { accounts: vec![acc_none, acc_false, acc_true], ..AppConfig::default() };
    cfg.save(&db).unwrap();

    let loaded = AppConfig::load(&db).unwrap();
    assert_eq!(loaded.accounts[0].ice_enabled, None);
    assert_eq!(loaded.accounts[1].ice_enabled, Some(false));
    assert_eq!(loaded.accounts[2].ice_enabled, Some(true));
    std::fs::remove_file(&path).ok();
}

#[test]
#[allow(clippy::field_reassign_with_default)] // many fields set individually is clearer here than one huge literal
fn save_then_load_round_trips_top_level_settings() {
    let path = temp_db_path("settings");
    let db = Db::open_at(&path).unwrap();
    let mut cfg = AppConfig::default();
    cfg.accounts = vec![];
    cfg.local_sip_port = 6060;
    cfg.stun_server = Some("stun.example.com:3478".into());
    cfg.turn_server = Some("turn.example.com:3478".into());
    cfg.turn_username = Some("turnuser".into());
    cfg.turn_password = Some("turnpass".into());
    cfg.rtp_port_min = Some(30000);
    cfg.rtp_port_max = Some(40000);
    cfg.custom_nameserver = Some("1.1.1.1".into());
    cfg.dns_srv_enabled = true;
    cfg.single_call_mode = true;
    cfg.dark_mode = false;
    cfg.notifications_enabled = false;
    cfg.recording_enabled = true;
    cfg.recording_format = RecordingFormat::Mp3;
    cfg.recordings_dir_override = Some("/tmp/recordings".into());
    cfg.blocklist = vec!["sip:spam@example.com".into(), "12345".into()];
    cfg.ice_enabled = true;
    cfg.global_hotkeys_enabled = true;
    cfg.hotkey_answer = "Ctrl+Alt+X".into();
    cfg.auto_update_enabled = true;
    cfg.update_skip_version = Some("1.2.3".into());
    cfg.update_check_frequency = UpdateCheckFrequency::Weekly;
    cfg.last_update_check = Some(1_700_000_000);
    cfg.default_list_action = DefaultListAction::Message;
    cfg.random_popup_position = true;
    cfg.zrtp_zid = Some("0123456789abcdef01234567".into());
    cfg.ldap_server = Some("ldap.example.com".into());
    cfg.ldap_port = 636;
    cfg.ldap_use_tls = true;
    cfg.ldap_base_dn = Some("dc=example,dc=com".into());
    cfg.ldap_bind_dn = Some("cn=reader,dc=example,dc=com".into());
    cfg.ldap_bind_password = Some("secret".into());
    cfg.ldap_search_filter = Some("(cn=*{query}*)".into());
    cfg.audio.input_device = Some("USB Mic".into());
    cfg.audio.video_capture_width = 1280;
    cfg.audio.video_capture_height = 720;

    cfg.save(&db).unwrap();
    let loaded = AppConfig::load(&db).unwrap();

    assert_eq!(loaded.local_sip_port, 6060);
    assert_eq!(loaded.stun_server, cfg.stun_server);
    assert_eq!(loaded.turn_server, cfg.turn_server);
    assert_eq!(loaded.turn_username, cfg.turn_username);
    assert_eq!(loaded.turn_password, cfg.turn_password);
    assert_eq!(loaded.rtp_port_min, cfg.rtp_port_min);
    assert_eq!(loaded.rtp_port_max, cfg.rtp_port_max);
    assert_eq!(loaded.custom_nameserver, cfg.custom_nameserver);
    assert!(loaded.dns_srv_enabled);
    assert!(loaded.single_call_mode);
    assert!(!loaded.dark_mode);
    assert!(!loaded.notifications_enabled);
    assert!(loaded.recording_enabled);
    assert_eq!(loaded.recording_format, RecordingFormat::Mp3);
    assert_eq!(loaded.recordings_dir_override, cfg.recordings_dir_override);
    assert_eq!(loaded.blocklist, cfg.blocklist);
    assert!(loaded.ice_enabled);
    assert!(loaded.global_hotkeys_enabled);
    assert_eq!(loaded.hotkey_answer, "Ctrl+Alt+X");
    assert!(loaded.auto_update_enabled);
    assert_eq!(loaded.update_skip_version, cfg.update_skip_version);
    assert_eq!(loaded.update_check_frequency, UpdateCheckFrequency::Weekly);
    assert_eq!(loaded.last_update_check, cfg.last_update_check);
    assert_eq!(loaded.default_list_action, DefaultListAction::Message);
    assert!(loaded.random_popup_position);
    assert_eq!(loaded.zrtp_zid, cfg.zrtp_zid);
    assert_eq!(loaded.ldap_server, cfg.ldap_server);
    assert_eq!(loaded.ldap_port, 636);
    assert!(loaded.ldap_use_tls);
    assert_eq!(loaded.ldap_base_dn, cfg.ldap_base_dn);
    assert_eq!(loaded.ldap_bind_dn, cfg.ldap_bind_dn);
    assert_eq!(loaded.ldap_bind_password, cfg.ldap_bind_password);
    assert_eq!(loaded.ldap_search_filter, cfg.ldap_search_filter);
    assert_eq!(loaded.audio.input_device, cfg.audio.input_device);
    assert_eq!(loaded.audio.video_capture_width, 1280);
    assert_eq!(loaded.audio.video_capture_height, 720);
    std::fs::remove_file(&path).ok();
}
