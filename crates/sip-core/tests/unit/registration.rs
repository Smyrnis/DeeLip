use super::*;

// `resolve_ip_rewrite` is the pure decision extracted from
// `maybe_rewrite_advertised_ip` -- everything else in `register_once`
// (sending REGISTER, awaiting/matching a response by Call-ID, retrying with
// digest auth) needs a live transport/registrar round-trip and isn't
// meaningfully testable here; `build_challenge_response` itself (the digest
// retry piece) already has its own dedicated coverage in `wire/auth.rs`'s
// test file.

#[test]
fn adopts_received_param_when_it_differs_from_current() {
    let via = "SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bK1;received=203.0.113.9;rport=5060";
    let new_ip = resolve_ip_rewrite("10.0.0.5", true, false, Some(via));
    assert_eq!(new_ip.as_deref(), Some("203.0.113.9"));
}

#[test]
fn no_change_when_received_matches_current_advertised_ip() {
    let via = "SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bK1;received=203.0.113.9";
    let new_ip = resolve_ip_rewrite("203.0.113.9", true, false, Some(via));
    assert_eq!(new_ip, None);
}

#[test]
fn disabled_when_allow_ip_rewrite_is_off() {
    let via = "SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bK1;received=203.0.113.9";
    let new_ip = resolve_ip_rewrite("10.0.0.5", false, false, Some(via));
    assert_eq!(new_ip, None, "allow_ip_rewrite=false must never rewrite, even with a received= param present");
}

#[test]
fn disabled_when_public_address_override_is_set() {
    let via = "SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bK1;received=203.0.113.9";
    let new_ip = resolve_ip_rewrite("10.0.0.5", true, true, Some(via));
    assert_eq!(new_ip, None, "an explicit public_address override must always win over received=-based rewrite");
}

#[test]
fn no_via_header_is_a_no_op() {
    assert_eq!(resolve_ip_rewrite("10.0.0.5", true, false, None), None);
}

#[test]
fn via_without_received_param_is_a_no_op() {
    let via = "SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bK1";
    assert_eq!(resolve_ip_rewrite("10.0.0.5", true, false, Some(via)), None, "no NAT in the path -- nothing to adopt");
}

#[test]
fn permanent_reg_error_display_mentions_status_and_wont_retry() {
    let err = PermanentRegError(403);
    let msg = err.to_string();
    assert!(msg.contains("403"));
    assert!(msg.contains("won't retry"));
}
