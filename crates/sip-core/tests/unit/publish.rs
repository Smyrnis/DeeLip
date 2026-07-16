use super::*;

// `own_pidf` is the pure PIDF-XML body builder used by every outgoing
// PUBLISH (initial, refresh, and DND-toggle re-publish); the rest of
// `publish.rs`'s logic (`build_publish`, `on_publish_response`) lives on
// `SipStack` and needs a live transport/dialog map to exercise meaningfully,
// so it isn't covered here -- mirrors this crate's existing convention
// (`presence.rs`/`mwi.rs` test files only cover their own free-standing
// parse functions, not their `SipStack` methods).

#[test]
fn own_pidf_available_reports_open_basic_status() {
    let xml = own_pidf("sip:alice@example.com", true);
    assert!(xml.contains("entity=\"sip:alice@example.com\""));
    assert!(xml.contains("<basic>open</basic>"));
    assert!(!xml.contains("closed"));
}

#[test]
fn own_pidf_unavailable_reports_closed_basic_status() {
    let xml = own_pidf("sip:alice@example.com", false);
    assert!(xml.contains("<basic>closed</basic>"));
    assert!(!xml.contains("open"));
}

#[test]
fn own_pidf_is_well_formed_minimal_xml() {
    let xml = own_pidf("sip:bob@example.com", true);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.contains("<presence xmlns=\"urn:ietf:params:xml:ns:pidf\""));
    assert!(xml.contains("<tuple id=\"deelip\">"));
    // Every opening tag that appears must be closed -- a cheap well-formedness
    // sanity check without pulling in an XML parser dependency.
    for tag in ["presence", "tuple", "status", "basic"] {
        assert_eq!(
            xml.matches(&format!("<{tag}")).count(),
            xml.matches(&format!("</{tag}>")).count(),
            "tag <{tag}> must be balanced"
        );
    }
}

#[test]
fn publish_expires_constant_is_one_hour() {
    assert_eq!(PUBLISH_EXPIRES, 3600);
}
