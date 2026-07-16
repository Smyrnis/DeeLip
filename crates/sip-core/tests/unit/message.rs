use super::*;

#[test]
fn parses_well_formed_request() {
    let raw = "INVITE sip:bob@example.com SIP/2.0\r\n\
               Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK776asdhds\r\n\
               Max-Forwards: 70\r\n\
               To: <sip:bob@example.com>\r\n\
               From: <sip:alice@example.com>;tag=1928301774\r\n\
               Call-ID: a84b4c76e66710@192.0.2.1\r\n\
               CSeq: 314159 INVITE\r\n\
               Content-Length: 4\r\n\r\n\
               body";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    match &msg.start_line {
        SipStartLine::Request { method, uri } => {
            assert_eq!(*method, SipMethod::Invite);
            assert_eq!(uri, "sip:bob@example.com");
        }
        _ => panic!("expected a request"),
    }
    assert_eq!(msg.method(), Some(&SipMethod::Invite));
    assert_eq!(msg.call_id(), Some("a84b4c76e66710@192.0.2.1"));
    assert_eq!(msg.cseq(), Some((314159, SipMethod::Invite)));
    assert_eq!(msg.header("Max-Forwards"), Some("70"));
    assert_eq!(msg.body, b"body");
}

#[test]
fn parses_well_formed_response() {
    let raw = "SIP/2.0 200 OK\r\n\
               Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK776asdhds;received=203.0.113.9\r\n\
               To: <sip:bob@example.com>;tag=a6c85cf\r\n\
               From: <sip:alice@example.com>;tag=1928301774\r\n\
               Call-ID: a84b4c76e66710@192.0.2.1\r\n\
               CSeq: 314159 INVITE\r\n\
               Contact: <sip:bob@192.0.2.4:5060>\r\n\
               Content-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.status_code(), Some(200));
    assert_eq!(msg.reason_phrase(), Some("OK"));
    assert_eq!(msg.method(), None, "a response has no method");
    assert_eq!(msg.cseq(), Some((314159, SipMethod::Invite)));
    assert_eq!(msg.header("Contact"), Some("<sip:bob@192.0.2.4:5060>"));
    assert!(msg.body.is_empty());
}

/// RFC 3261 section 7.3.1 header folding: a continuation line beginning with
/// whitespace is unfolded into the previous header's value with a single
/// space, not treated as a separate header or dropped.
#[test]
fn header_folding_continuation_line_is_unfolded() {
    let raw = "SIP/2.0 200 OK\r\n\
               Subject: I know you're there,\r\n\
               \tpick up the phone\r\n\
               Call-ID: abc123\r\n\
               Content-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.header("Subject"), Some("I know you're there, pick up the phone"));
}

#[test]
fn header_folding_with_space_continuation() {
    let raw = "SIP/2.0 200 OK\r\nCall-ID: abc123\r\nWarning: 370 example.com \"Insufficient bandwidth,\r\n\x20speak up\"\r\nContent-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.header("Warning"), Some("370 example.com \"Insufficient bandwidth, speak up\""));
}

/// Some headers (e.g. `Via` when a request traversed multiple proxies) can
/// repeat; `headers_all` must preserve every occurrence in order, while
/// `header` keeps returning only the first (topmost) one.
#[test]
fn multiple_headers_with_same_name_are_all_preserved() {
    let raw = "SIP/2.0 200 OK\r\n\
               Via: SIP/2.0/UDP proxy2.example.com;branch=z9hG4bK2\r\n\
               Via: SIP/2.0/UDP proxy1.example.com;branch=z9hG4bK1\r\n\
               Call-ID: abc123\r\n\
               Content-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    let all = msg.headers_all("Via");
    assert_eq!(
        all,
        vec!["SIP/2.0/UDP proxy2.example.com;branch=z9hG4bK2", "SIP/2.0/UDP proxy1.example.com;branch=z9hG4bK1"]
    );
    assert_eq!(msg.header("Via"), Some("SIP/2.0/UDP proxy2.example.com;branch=z9hG4bK2"));
    // The branch param of the topmost Via -- what a UAC would extract when
    // matching a response's own top Via to the branch it sent.
    let branch = msg.header("Via").unwrap().split("branch=").nth(1).unwrap();
    assert_eq!(branch, "z9hG4bK2");
}

#[test]
fn header_lookup_is_case_insensitive() {
    let raw = "SIP/2.0 200 OK\r\ncall-id: abc123\r\nCSEQ: 1 INVITE\r\nContent-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.header("Call-ID"), Some("abc123"));
    assert_eq!(msg.call_id(), Some("abc123"));
    assert_eq!(msg.cseq(), Some((1, SipMethod::Invite)));
}

#[test]
fn call_id_falls_back_to_compact_i_header() {
    let raw = "SIP/2.0 200 OK\r\ni: compact-call-id\r\nContent-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.call_id(), Some("compact-call-id"));
}

#[test]
fn unknown_method_becomes_other_variant() {
    let raw = "PING sip:bob@example.com SIP/2.0\r\nCall-ID: abc\r\nContent-Length: 0\r\n\r\n";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.method(), Some(&SipMethod::Other("PING".to_string())));
    assert_eq!(msg.method().unwrap().as_str(), "PING");
}

#[test]
fn content_length_header_is_readable_and_body_preserved_verbatim() {
    let body = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n";
    let raw = format!(
        "SIP/2.0 200 OK\r\nCall-ID: abc\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.header("Content-Length"), Some(body.len().to_string()).as_deref());
    assert_eq!(msg.body, body.as_bytes());
}

/// `SipMessage::parse` itself does no Content-Length-based truncation (that's
/// `wire::framing::MessageFramer`'s job for stream transports) -- for a
/// single already-complete buffer, everything after the blank line is the
/// body verbatim, even if it doesn't match the declared Content-Length.
#[test]
fn body_is_not_truncated_to_declared_content_length() {
    let raw = "SIP/2.0 200 OK\r\nCall-ID: abc\r\nContent-Length: 2\r\n\r\nthis is actually longer than declared";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should parse");
    assert_eq!(msg.body, b"this is actually longer than declared");
}

#[test]
fn malformed_empty_input_returns_none() {
    assert!(SipMessage::parse(b"").is_none());
}

#[test]
fn malformed_response_missing_status_code_returns_none() {
    assert!(SipMessage::parse(b"SIP/2.0 \r\nCall-ID: abc\r\n\r\n").is_none());
}

#[test]
fn malformed_request_missing_uri_returns_none() {
    assert!(SipMessage::parse(b"INVITE\r\nCall-ID: abc\r\n\r\n").is_none());
}

#[test]
fn invalid_utf8_returns_none() {
    let raw: &[u8] = &[0xFF, 0xFE, 0xFD];
    assert!(SipMessage::parse(raw).is_none());
}

/// A message truncated mid-headers (no blank-line body separator at all) is
/// still parsed -- every line seen is treated as a header and the body ends
/// up empty, rather than the whole parse failing. Documents actual behavior
/// so a change here is deliberate, not accidental.
#[test]
fn truncated_message_with_no_blank_line_yields_empty_body() {
    let raw = "SIP/2.0 100 Trying\r\nCall-ID: abc\r\nCSeq: 1 INVITE";
    let msg = SipMessage::parse(raw.as_bytes()).expect("should still parse headers");
    assert_eq!(msg.status_code(), Some(100));
    assert_eq!(msg.call_id(), Some("abc"));
    assert!(msg.body.is_empty());
}

#[test]
fn sip_method_as_str_round_trips_for_every_known_variant() {
    let known = [
        ("REGISTER", SipMethod::Register),
        ("INVITE", SipMethod::Invite),
        ("ACK", SipMethod::Ack),
        ("BYE", SipMethod::Bye),
        ("CANCEL", SipMethod::Cancel),
        ("OPTIONS", SipMethod::Options),
        ("INFO", SipMethod::Info),
        ("NOTIFY", SipMethod::Notify),
        ("SUBSCRIBE", SipMethod::Subscribe),
        ("REFER", SipMethod::Refer),
        ("MESSAGE", SipMethod::Message),
        ("PUBLISH", SipMethod::Publish),
    ];
    for (s, variant) in known {
        assert_eq!(SipMethod::from(s), variant);
        assert_eq!(variant.as_str(), s);
    }
}
