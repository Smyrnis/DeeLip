use super::*;

fn contact(name: &str, sip_uri: &str) -> Contact {
    Contact { name: name.to_string(), sip_uri: sip_uri.to_string(), watch_presence: false, presence_account: None }
}

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("deelip-config-test-contacts-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn find_by_uri_exact_match() {
    let book = ContactBook { contacts: vec![contact("Bob", "sip:600@127.0.0.1")] };
    assert_eq!(book.find_by_uri("sip:600@127.0.0.1").unwrap().name, "Bob");
}

#[test]
fn find_by_uri_ignores_case() {
    let book = ContactBook { contacts: vec![contact("Bob", "sip:Bob@Example.com")] };
    assert_eq!(book.find_by_uri("SIP:bob@example.com").unwrap().name, "Bob");
}

#[test]
fn find_by_uri_ignores_trailing_params() {
    let book = ContactBook { contacts: vec![contact("Bob", "sip:600@127.0.0.1")] };
    assert_eq!(book.find_by_uri("sip:600@127.0.0.1;user=phone").unwrap().name, "Bob");
}

#[test]
fn find_by_uri_ignores_explicit_default_port() {
    let book = ContactBook { contacts: vec![contact("Bob", "sip:600@127.0.0.1")] };
    assert_eq!(book.find_by_uri("sip:600@127.0.0.1:5060").unwrap().name, "Bob");
}

#[test]
fn find_by_uri_no_match_returns_none() {
    let book = ContactBook { contacts: vec![contact("Bob", "sip:600@127.0.0.1")] };
    assert!(book.find_by_uri("sip:700@127.0.0.1").is_none());
}

#[test]
fn search_matches_name_or_uri_case_insensitively() {
    let book = ContactBook {
        contacts: vec![contact("Alice Smith", "sip:alice@example.com"), contact("Bob Jones", "sip:bob@example.com")],
    };
    let by_name = book.search("ALICE");
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0].0, 0);
    assert_eq!(by_name[0].1.name, "Alice Smith");

    let by_uri = book.search("bob@example");
    assert_eq!(by_uri.len(), 1);
    assert_eq!(by_uri[0].0, 1);
    assert_eq!(by_uri[0].1.name, "Bob Jones");
}

#[test]
fn search_empty_query_returns_everything_with_original_indices() {
    let book = ContactBook { contacts: vec![contact("A", "sip:a@x"), contact("B", "sip:b@x")] };
    let results = book.search("");
    assert_eq!(results.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn search_no_match_returns_empty() {
    let book = ContactBook { contacts: vec![contact("Alice", "sip:alice@x")] };
    assert!(book.search("zzz").is_empty());
}

#[test]
fn save_then_load_round_trips_contacts_including_presence_fields() {
    let path = temp_db_path("roundtrip");
    let db = Db::open_at(&path).unwrap();
    let book = ContactBook {
        contacts: vec![
            Contact {
                name: "Alice".into(),
                sip_uri: "sip:alice@example.com".into(),
                watch_presence: true,
                presence_account: Some("work".into()),
            },
            Contact {
                name: "Bob".into(),
                sip_uri: "sip:bob@example.com".into(),
                watch_presence: false,
                presence_account: None,
            },
        ],
    };
    book.save(&db).unwrap();

    let loaded = ContactBook::load(&db).unwrap();
    assert_eq!(loaded.contacts.len(), 2);
    assert_eq!(loaded.contacts[0].name, "Alice");
    assert!(loaded.contacts[0].watch_presence);
    assert_eq!(loaded.contacts[0].presence_account.as_deref(), Some("work"));
    assert_eq!(loaded.contacts[1].name, "Bob");
    assert!(!loaded.contacts[1].watch_presence);
    assert_eq!(loaded.contacts[1].presence_account, None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn save_replaces_rather_than_appends() {
    let path = temp_db_path("replace");
    let db = Db::open_at(&path).unwrap();
    ContactBook { contacts: vec![contact("A", "sip:a@x"), contact("B", "sip:b@x")] }.save(&db).unwrap();
    ContactBook { contacts: vec![contact("C", "sip:c@x")] }.save(&db).unwrap();

    let loaded = ContactBook::load(&db).unwrap();
    assert_eq!(loaded.contacts.len(), 1);
    assert_eq!(loaded.contacts[0].name, "C");
    std::fs::remove_file(&path).ok();
}
