use super::*;

#[test]
fn parse_via_received_extracts_both_params() {
    let via = "SIP/2.0/UDP 192.168.1.5:5060;branch=z9hG4bK123;rport=54321;received=203.0.113.5";
    let (received, rport) = parse_via_received(via);
    assert_eq!(received, Some("203.0.113.5".to_string()));
    assert_eq!(rport, Some(54321));
}

#[test]
fn parse_via_received_handles_rport_with_no_value() {
    // A request's own Via has a bare `;rport` with no `=value` until the
    // server fills it in on the response -- shouldn't be mistaken for a real port.
    let via = "SIP/2.0/UDP 192.168.1.5:5060;branch=z9hG4bK123;rport";
    let (received, rport) = parse_via_received(via);
    assert_eq!(received, None);
    assert_eq!(rport, None);
}

#[test]
fn parse_via_received_missing_params_returns_none() {
    let via = "SIP/2.0/UDP 192.168.1.5:5060;branch=z9hG4bK123";
    assert_eq!(parse_via_received(via), (None, None));
}

#[test]
fn parse_session_expires_with_refresher() {
    assert_eq!(
        parse_session_expires("1800;refresher=uac"),
        Some((1800, Some("uac".to_string())))
    );
    assert_eq!(
        parse_session_expires("90;refresher=UAS"),
        Some((90, Some("uas".to_string())))
    );
}

#[test]
fn parse_session_expires_without_refresher() {
    assert_eq!(parse_session_expires("1800"), Some((1800, None)));
}

#[test]
fn parse_session_expires_invalid_interval_returns_none() {
    assert_eq!(parse_session_expires("not-a-number;refresher=uac"), None);
}

fn invite_with_call_info(call_info: &str) -> crate::wire::message::SipMessage {
    let raw = format!(
        "INVITE sip:bob@example.com SIP/2.0\r\n\
         Via: SIP/2.0/UDP 192.168.1.5:5060;branch=z9hG4bK123\r\n\
         From: <sip:alice@example.com>;tag=abc\r\n\
         To: <sip:bob@example.com>\r\n\
         Call-ID: xyz@192.168.1.5\r\n\
         CSeq: 1 INVITE\r\n\
         Call-Info: {call_info}\r\n\
         Content-Length: 0\r\n\r\n"
    );
    crate::wire::message::SipMessage::parse(raw.as_bytes()).unwrap()
}

#[test]
fn parse_call_info_answer_after_finds_param() {
    let msg = invite_with_call_info("<sip:paging@example.com>;answer-after=0");
    assert_eq!(parse_call_info_answer_after(&msg), Some(0));
}

#[test]
fn parse_call_info_answer_after_nonzero_delay() {
    let msg = invite_with_call_info("<sip:paging@example.com>;answer-after=2");
    assert_eq!(parse_call_info_answer_after(&msg), Some(2));
}

#[test]
fn parse_call_info_answer_after_missing_returns_none() {
    let msg = invite_with_call_info("<sip:foo@example.com>;purpose=icon");
    assert_eq!(parse_call_info_answer_after(&msg), None);
}

#[test]
fn uri_host_port_bare_ip_defaults_port() {
    assert_eq!(
        uri_host_port("192.168.1.50"),
        Some(("192.168.1.50".to_string(), 5060))
    );
}

#[test]
fn uri_host_port_sip_scheme_with_port() {
    assert_eq!(
        uri_host_port("sip:192.168.1.50:5061"),
        Some(("192.168.1.50".to_string(), 5061))
    );
}

#[test]
fn uri_host_port_strips_user_part() {
    assert_eq!(
        uri_host_port("sip:bob@192.168.1.50:5060"),
        Some(("192.168.1.50".to_string(), 5060))
    );
}

#[test]
fn uri_host_port_strips_uri_params() {
    assert_eq!(
        uri_host_port("sip:bob@192.168.1.50:5060;transport=udp"),
        Some(("192.168.1.50".to_string(), 5060))
    );
}

#[test]
fn uri_host_port_ipv6_literal() {
    assert_eq!(
        uri_host_port("sip:[::1]:5060"),
        Some(("::1".to_string(), 5060))
    );
    assert_eq!(uri_host_port("sip:[::1]"), Some(("::1".to_string(), 5060)));
}

#[test]
fn uri_host_port_empty_returns_none() {
    assert_eq!(uri_host_port("sip:"), None);
}

#[test]
fn parse_uri_prefers_angle_bracket_form() {
    assert_eq!(
        parse_uri("\"Alice\" <sip:alice@example.com>;tag=abc"),
        Some("sip:alice@example.com".to_string())
    );
}

#[test]
fn parse_uri_bare_form_well_formed() {
    assert_eq!(
        parse_uri("sip:bob@example.com;tag=xyz"),
        Some("sip:bob@example.com".to_string())
    );
}

#[test]
fn parse_uri_bare_form_with_glued_leading_token() {
    // Some UAs send a malformed bare header with a display-name-like token
    // glued directly onto the URI, no quotes/brackets -- must not store the
    // leading "600:" token as part of the URI.
    assert_eq!(
        parse_uri("600:sip:scco@10.0.0.5;tag=abc"),
        Some("sip:scco@10.0.0.5".to_string())
    );
}

#[test]
fn parse_uri_no_scheme_falls_back_to_whole_candidate() {
    assert_eq!(parse_uri("garbage;tag=abc"), Some("garbage".to_string()));
}
