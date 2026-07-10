use super::*;
use deelip_config::{Contact, ContactBook};

#[test]
fn friendly_uri_strips_scheme_and_host() {
    assert_eq!(friendly_uri("sip:600@127.0.0.1"), "600");
}

#[test]
fn friendly_uri_strips_params() {
    assert_eq!(friendly_uri("sip:600@127.0.0.1;user=phone"), "600");
}

#[test]
fn friendly_uri_handles_bare_no_host() {
    assert_eq!(friendly_uri("sip:600"), "600");
}

#[test]
fn friendly_uri_anonymous_caller() {
    assert_eq!(
        friendly_uri("sip:anonymous@anonymous.invalid"),
        "Unknown caller"
    );
    assert_eq!(
        friendly_uri("sip:Anonymous@Anonymous.Invalid"),
        "Unknown caller"
    );
}

#[test]
fn friendly_uri_bare_anonymous_user_without_matching_host_is_not_special_cased() {
    assert_eq!(friendly_uri("sip:anonymous@example.com"), "anonymous");
}

#[test]
fn resolve_caller_prefers_contact_name() {
    let book = ContactBook {
        contacts: vec![Contact {
            name: "Bob Marley".to_string(),
            sip_uri: "sip:600@127.0.0.1".to_string(),
            watch_presence: false,
            presence_account: None,
        }],
    };
    assert_eq!(
        resolve_caller(&book, "sip:600@127.0.0.1"),
        ("Bob Marley".to_string(), true)
    );
}

#[test]
fn resolve_caller_falls_back_to_friendly_uri() {
    let book = ContactBook::default();
    assert_eq!(
        resolve_caller(&book, "sip:700@127.0.0.1"),
        ("700".to_string(), false)
    );
}
