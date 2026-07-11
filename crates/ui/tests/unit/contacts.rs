use super::*;

#[test]
fn parse_csv_line_splits_on_commas() {
    assert_eq!(parse_csv_line("Alice,sip:alice@example.com"), vec!["Alice", "sip:alice@example.com"]);
}

#[test]
fn parse_csv_line_honors_quoted_fields_with_embedded_commas() {
    assert_eq!(parse_csv_line(r#""Doe, Jane",sip:jane@example.com"#), vec!["Doe, Jane", "sip:jane@example.com"]);
}

#[test]
fn parse_csv_line_unescapes_doubled_quotes() {
    assert_eq!(parse_csv_line(r#""She said ""hi""",600"#), vec![r#"She said "hi""#, "600"]);
}

#[test]
fn parse_contacts_csv_skips_header_and_blank_lines() {
    let content = "name,sip_uri\nAlice,sip:alice@example.com\n\nBob,600\n";
    let contacts = parse_contacts_csv(content);
    assert_eq!(contacts.len(), 2);
    assert_eq!(contacts[0].name, "Alice");
    assert_eq!(contacts[0].sip_uri, "sip:alice@example.com");
    assert_eq!(contacts[1].name, "Bob");
    assert_eq!(contacts[1].sip_uri, "600");
}

#[test]
fn parse_contacts_csv_skips_rows_missing_a_uri_field() {
    let content = "name,sip_uri\nAlice\n";
    assert!(parse_contacts_csv(content).is_empty());
}

#[test]
fn parse_vcard_extracts_fn_and_tel() {
    let content = "BEGIN:VCARD\r\nFN:Alice Example\r\nTEL:600\r\nEND:VCARD\r\n";
    let contacts = parse_vcard(content);
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].name, "Alice Example");
    assert_eq!(contacts[0].sip_uri, "600");
}

#[test]
fn parse_vcard_falls_back_to_impp_when_no_tel() {
    let content = "BEGIN:VCARD\nFN:Bob\nIMPP:sip:bob@example.com\nEND:VCARD\n";
    let contacts = parse_vcard(content);
    assert_eq!(contacts[0].sip_uri, "sip:bob@example.com");
}

#[test]
fn parse_vcard_ignores_property_parameters() {
    let content = "BEGIN:VCARD\nFN;CHARSET=UTF-8:Alice\nTEL;TYPE=CELL:600\nEND:VCARD\n";
    let contacts = parse_vcard(content);
    assert_eq!(contacts[0].name, "Alice");
    assert_eq!(contacts[0].sip_uri, "600");
}

#[test]
fn parse_vcard_skips_incomplete_blocks_and_handles_multiple_cards() {
    let content = "BEGIN:VCARD\nFN:NoUri\nEND:VCARD\nBEGIN:VCARD\nFN:Carol\nTEL:601\nEND:VCARD\n";
    let contacts = parse_vcard(content);
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].name, "Carol");
}

#[test]
fn parse_vcard_keeps_first_matching_property_per_card() {
    let content = "BEGIN:VCARD\nFN:First\nFN:Second\nTEL:600\nTEL:601\nEND:VCARD\n";
    let contacts = parse_vcard(content);
    assert_eq!(contacts[0].name, "First");
    assert_eq!(contacts[0].sip_uri, "600");
}
