use super::{extract_user_part, normalize_target};

#[test]
fn bare_number_gets_domain_appended() {
    assert_eq!(normalize_target("600", "127.0.0.1"), "sip:600@127.0.0.1");
}

#[test]
fn existing_sip_uri_is_untouched() {
    assert_eq!(
        normalize_target("sip:600@127.0.0.1", "example.com"),
        "sip:600@127.0.0.1"
    );
}

#[test]
fn sips_uri_is_untouched() {
    assert_eq!(
        normalize_target("sips:bob@example.com", "example.com"),
        "sips:bob@example.com"
    );
}

#[test]
fn user_at_host_without_scheme_gets_scheme_added() {
    assert_eq!(
        normalize_target("bob@example.com", "example.com"),
        "sip:bob@example.com"
    );
}

#[test]
fn trims_whitespace() {
    assert_eq!(
        normalize_target("  600  ", "127.0.0.1"),
        "sip:600@127.0.0.1"
    );
}

#[test]
fn extracts_user_from_bare_number() {
    assert_eq!(extract_user_part("5551234"), "5551234");
}

#[test]
fn extracts_user_from_full_uri_with_params() {
    assert_eq!(
        extract_user_part("sip:5551234@host.example;user=phone"),
        "5551234"
    );
}

#[test]
fn extract_user_part_is_case_insensitive() {
    assert_eq!(
        extract_user_part("SIP:Bob@Example.com"),
        extract_user_part("sip:bob@example.com")
    );
}
