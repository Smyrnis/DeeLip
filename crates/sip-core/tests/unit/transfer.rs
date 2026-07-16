use super::*;

// `resolve_contact_addr`/`build_refer_to_with_replaces` are the pure
// request-construction pieces extracted from `blind_transfer`/
// `attended_transfer`/`redirect_call` -- the rest (actually sending the
// REFER/302, mutating `self.dialogs`) needs a live transport/dialog map and
// isn't meaningfully testable without a full `SipStack`.

#[test]
fn resolve_contact_addr_prefers_remote_contact_when_parseable() {
    let fallback: SocketAddr = "198.51.100.1:5060".parse().unwrap();
    let resolved = resolve_contact_addr(Some("203.0.113.9:5061"), fallback);
    assert_eq!(resolved, "203.0.113.9:5061".parse::<SocketAddr>().unwrap());
}

#[test]
fn resolve_contact_addr_falls_back_when_remote_contact_is_none() {
    let fallback: SocketAddr = "198.51.100.1:5060".parse().unwrap();
    assert_eq!(resolve_contact_addr(None, fallback), fallback);
}

#[test]
fn resolve_contact_addr_falls_back_when_remote_contact_is_unparseable() {
    let fallback: SocketAddr = "198.51.100.1:5060".parse().unwrap();
    // e.g. a bare hostname with no port -- `SocketAddr::parse` rejects this.
    assert_eq!(resolve_contact_addr(Some("not-an-address"), fallback), fallback);
}

#[test]
fn build_refer_to_with_replaces_percent_encodes_the_replaces_param() {
    let replaces = "abc123@host;to-tag=xyz;from-tag=def";
    let refer_to = build_refer_to_with_replaces("sip:carol@example.com", replaces);
    // `;`, `=`, and `@` inside the Replaces value all get percent-encoded
    // (per `encode_replaces_param`) so they can't be mistaken for the
    // enclosing URI's own param/query syntax; the target URI itself
    // ("sip:carol@example.com") is untouched -- only `replaces` is encoded.
    assert_eq!(refer_to, "sip:carol@example.com?Replaces=abc123%40host%3Bto-tag%3Dxyz%3Bfrom-tag%3Ddef");
    assert!(!refer_to.contains(";to-tag="), "raw ';' must not survive inside the Replaces value");
    assert!(refer_to.starts_with("sip:carol@example.com?Replaces="), "the target URI's own '@' must stay literal");
}

#[test]
fn build_refer_to_with_replaces_round_trips_via_encode_replaces_param() {
    let replaces = "call-id-1;to-tag=a;from-tag=b";
    let refer_to = build_refer_to_with_replaces("sip:target@example.com", replaces);
    let expected = format!("sip:target@example.com?Replaces={}", encode_replaces_param(replaces));
    assert_eq!(refer_to, expected);
}
