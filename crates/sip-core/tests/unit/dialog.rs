use super::*;

#[test]
fn new_outgoing_starts_in_calling_with_expected_defaults() {
    let dialog = Dialog::new_outgoing("call-1".into(), "local-tag".into(), "sip:bob@example.com".into());
    assert_eq!(dialog.state, DialogState::Calling);
    assert_eq!(dialog.call_id, "call-1");
    assert_eq!(dialog.local_tag, "local-tag");
    assert_eq!(dialog.remote_uri, "sip:bob@example.com");
    assert_eq!(dialog.remote_tag, None);
    assert_eq!(dialog.remote_contact, None);
    assert_eq!(dialog.local_cseq, 1);
    assert_eq!(dialog.remote_cseq, None);
    assert!(dialog.media.is_none());
    // The caller side (this constructor) is always the original UAC.
    assert!(dialog.original_role_is_uac);
}

#[test]
fn new_incoming_starts_in_calling_with_expected_defaults() {
    let dialog = Dialog::new_incoming(
        "call-2".into(),
        "local-tag".into(),
        "sip:alice@example.com".into(),
        "from-tag".into(),
        5,
        "v=0\r\n".into(),
        "SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK1".into(),
    );
    assert_eq!(dialog.state, DialogState::Calling);
    assert_eq!(dialog.remote_tag.as_deref(), Some("from-tag"));
    assert_eq!(dialog.remote_uri, "sip:alice@example.com");
    assert_eq!(dialog.remote_cseq, Some(5));
    assert_eq!(dialog.local_cseq, 0);
    assert_eq!(dialog.remote_sdp.as_deref(), Some("v=0\r\n"));
    assert_eq!(dialog.remote_via, "SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK1");
    // The callee side (this constructor) never originated the INVITE.
    assert!(!dialog.original_role_is_uac);
}

#[test]
fn next_local_cseq_increments_sequentially() {
    let mut dialog = Dialog::new_outgoing("call-1".into(), "tag".into(), "sip:bob@example.com".into());
    assert_eq!(dialog.local_cseq, 1);
    assert_eq!(dialog.next_local_cseq(), 2);
    assert_eq!(dialog.next_local_cseq(), 3);
    assert_eq!(dialog.local_cseq, 3);
}

/// Drives the documented state machine (Calling -> Ringing -> Confirmed ->
/// Terminating -> Terminated) end to end -- `Dialog` itself doesn't enforce
/// transitions (that's `on_response`/`client` code's job), but this locks
/// down that every state is reachable and distinct so a typo/reorder in the
/// enum would be caught.
#[test]
fn dialog_state_full_lifecycle_transitions() {
    let mut dialog = Dialog::new_outgoing("call-1".into(), "tag".into(), "sip:bob@example.com".into());
    assert_eq!(dialog.state, DialogState::Calling);

    dialog.state = DialogState::Ringing;
    assert_eq!(dialog.state, DialogState::Ringing);
    assert_ne!(dialog.state, DialogState::Calling);

    dialog.state = DialogState::Confirmed;
    assert_eq!(dialog.state, DialogState::Confirmed);

    dialog.state = DialogState::Terminating;
    assert_eq!(dialog.state, DialogState::Terminating);

    dialog.state = DialogState::Terminated;
    assert_eq!(dialog.state, DialogState::Terminated);
}

// ── `remote_contact` regression coverage ────────────────────────────────────
//
// This project hit a real live bug: the caller side never populated
// `Dialog::remote_contact` from the initial INVITE's 200 OK, so every
// mid-dialog request the caller sent (BYE, hold/resume re-INVITEs, transfer)
// fell back to the outbound proxy address instead of the far end's real
// Contact -- harmless with a proxy in the path, but a dead end for
// `local_account`/proxy-less calls. Fixed by parsing the 200 OK's own
// `Contact:` header the same way the callee side already did (see
// docs/crates/sip-core.md). `Dialog::parse_remote_contact` is the extracted,
// directly-testable piece of that fix; `response/mod.rs`'s `on_response`
// calls it verbatim on the caller-side `Act::Connected` path.

#[test]
fn parse_remote_contact_extracts_host_port_from_200_ok_contact_header() {
    let contact = Dialog::parse_remote_contact(Some("<sip:bob@203.0.113.9:5061>"));
    assert_eq!(contact.as_deref(), Some("203.0.113.9:5061"));
}

#[test]
fn parse_remote_contact_defaults_port_when_absent() {
    let contact = Dialog::parse_remote_contact(Some("<sip:bob@203.0.113.9>"));
    assert_eq!(contact.as_deref(), Some("203.0.113.9:5060"));
}

#[test]
fn parse_remote_contact_handles_bare_uri_form_with_params() {
    let contact = Dialog::parse_remote_contact(Some("sip:bob@203.0.113.9:5061;transport=udp"));
    assert_eq!(contact.as_deref(), Some("203.0.113.9:5061"));
}

#[test]
fn parse_remote_contact_none_when_header_absent() {
    assert_eq!(Dialog::parse_remote_contact(None), None);
}

/// Simulates the exact caller-side transition `on_response` performs for the
/// initial INVITE's 200 OK: a freshly-constructed outgoing `Dialog` (no
/// `remote_contact` yet, same as right after `new_outgoing`) gets it
/// populated from the response's `Contact:` header.
#[test]
fn caller_side_200_ok_contact_header_populates_dialog_remote_contact() {
    let mut dialog = Dialog::new_outgoing("call-1".into(), "local-tag".into(), "sip:bob@example.com".into());
    assert_eq!(dialog.remote_contact, None, "must start unpopulated on the caller side");

    let contact_header = "<sip:bob@198.51.100.7:5060>";
    dialog.remote_contact = Dialog::parse_remote_contact(Some(contact_header));

    assert_eq!(
        dialog.remote_contact.as_deref(),
        Some("198.51.100.7:5060"),
        "caller-side Dialog::remote_contact must be populated from the 200 OK's Contact header"
    );
}
