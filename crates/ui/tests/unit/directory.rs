use super::*;

#[test]
fn escape_ldap_filter_escapes_special_chars() {
    assert_eq!(escape_ldap_filter("a*b(c)d\\e"), "a\\2ab\\28c\\29d\\5ce");
}

#[test]
fn escape_ldap_filter_leaves_plain_text_untouched() {
    assert_eq!(escape_ldap_filter("Alice Example"), "Alice Example");
}

#[test]
fn escape_ldap_filter_prevents_filter_injection() {
    // A naive un-escaped "*)(uid=*" style injection should come back inert.
    let escaped = escape_ldap_filter("*)(uid=*");
    assert!(!escaped.contains('*'));
    assert!(!escaped.contains('('));
    assert!(!escaped.contains(')'));
}
