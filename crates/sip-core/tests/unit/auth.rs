use super::*;

/// Known-answer vector for `compute_digest_response`, derived by hand from
/// the RFC 2617 digest formula (no `qop`: `response =
/// MD5(HA1:nonce:HA2)`) and cross-checked independently two ways --
/// `hashlib.md5` in Python and `openssl dgst -md5` on the command line both
/// agree on every intermediate value:
///   HA1 = MD5("alice:asterisk:hunter2")     = 845217e878cd7866a79e4361b64bc5b4
///   HA2 = MD5("REGISTER:sip:example.com")   = 0264b00abe5b31d87fb22979689b883f
///   response = MD5(HA1:nonce:HA2)           = e9575024bba397ff6ee05cf5ab2400d7
#[test]
fn compute_digest_response_matches_known_answer_vector() {
    let response =
        compute_digest_response("alice", "asterisk", "hunter2", "REGISTER", "sip:example.com", "abcdef0123456789");
    assert_eq!(response, "e9575024bba397ff6ee05cf5ab2400d7");
}

#[test]
fn compute_digest_response_wrong_password_yields_different_response() {
    let right =
        compute_digest_response("alice", "asterisk", "hunter2", "REGISTER", "sip:example.com", "abcdef0123456789");
    let wrong =
        compute_digest_response("alice", "asterisk", "wrongpass", "REGISTER", "sip:example.com", "abcdef0123456789");
    assert_ne!(right, wrong);
    // Also matches the independently-computed known-answer vector for the
    // wrong password, not just "differs" -- guards against a compute bug
    // that happens to differ but for the wrong reason.
    assert_eq!(wrong, "dc5ab62602c1646081a7223735cf2732");
}

#[test]
fn compute_digest_response_is_deterministic() {
    let a = compute_digest_response("bob", "realm", "pw", "INVITE", "sip:bob@example.com", "nonce123");
    let b = compute_digest_response("bob", "realm", "pw", "INVITE", "sip:bob@example.com", "nonce123");
    assert_eq!(a, b);
    // A 32-char lowercase hex string (MD5 output).
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn build_auth_header_has_expected_shape() {
    let hdr = build_auth_header(
        "alice",
        "asterisk",
        "abcdef0123456789",
        "sip:example.com",
        "e9575024bba397ff6ee05cf5ab2400d7",
    );
    assert_eq!(
        hdr,
        "Authorization: Digest username=\"alice\", realm=\"asterisk\", nonce=\"abcdef0123456789\", \
         uri=\"sip:example.com\", response=\"e9575024bba397ff6ee05cf5ab2400d7\", algorithm=MD5"
    );
}

#[test]
fn digest_challenge_parses_standard_header() {
    let challenge = DigestChallenge::parse(
        r#"Digest realm="asterisk", nonce="abcdef0123456789", algorithm=MD5, opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
    )
    .expect("should parse");
    assert_eq!(challenge.realm, "asterisk");
    assert_eq!(challenge.nonce, "abcdef0123456789");
    assert_eq!(challenge.algorithm, "MD5");
    assert_eq!(challenge.opaque.as_deref(), Some("5ccc069c403ebaf9f0171e9517f40e41"));
}

#[test]
fn digest_challenge_parse_is_case_insensitive_on_digest_prefix() {
    let challenge = DigestChallenge::parse(r#"digest realm="r", nonce="n""#).expect("should parse lowercase digest");
    assert_eq!(challenge.realm, "r");
    assert_eq!(challenge.nonce, "n");
}

#[test]
fn digest_challenge_defaults_algorithm_to_md5_when_absent() {
    let challenge = DigestChallenge::parse(r#"Digest realm="r", nonce="n""#).expect("should parse");
    assert_eq!(challenge.algorithm, "MD5");
    assert_eq!(challenge.opaque, None);
}

/// This crate's `DigestChallenge`/`build_challenge_response` implement plain
/// RFC 2617 digest auth with no `qop`/`cnonce`/`nc` support at all -- any
/// `qop="auth"` param on the challenge is simply ignored (not read into
/// `DigestChallenge`, and never echoed back), so the resulting
/// `Authorization:` header never carries `qop=`/`cnonce=`/`nc=`, even if the
/// server offered `qop`.
#[test]
fn qop_param_on_challenge_is_ignored_not_reflected_in_response() {
    let challenge_hdr = r#"Digest realm="asterisk", nonce="abcdef0123456789", qop="auth", algorithm=MD5"#;
    let auth = build_challenge_response("alice", "hunter2", "REGISTER", "sip:example.com", challenge_hdr)
        .expect("should build a response despite qop being present");
    assert!(!auth.contains("qop="), "qop must not be reflected -- this implementation doesn't support it");
    assert!(!auth.contains("cnonce="), "cnonce must not appear -- no qop=auth support");
    assert!(!auth.contains("nc="));
    // The response value itself must still match the plain (no-qop) formula.
    assert!(auth.contains("e9575024bba397ff6ee05cf5ab2400d7"));
}

#[test]
fn build_challenge_response_end_to_end_matches_known_answer() {
    let challenge_hdr = r#"Digest realm="asterisk", nonce="abcdef0123456789""#;
    let auth = build_challenge_response("alice", "hunter2", "REGISTER", "sip:example.com", challenge_hdr)
        .expect("should build");
    assert_eq!(
        auth,
        "Authorization: Digest username=\"alice\", realm=\"asterisk\", nonce=\"abcdef0123456789\", \
         uri=\"sip:example.com\", response=\"e9575024bba397ff6ee05cf5ab2400d7\", algorithm=MD5"
    );
}

#[test]
fn build_challenge_response_wrong_password_produces_different_header() {
    let challenge_hdr = r#"Digest realm="asterisk", nonce="abcdef0123456789""#;
    let right = build_challenge_response("alice", "hunter2", "REGISTER", "sip:example.com", challenge_hdr).unwrap();
    let wrong = build_challenge_response("alice", "wrongpass", "REGISTER", "sip:example.com", challenge_hdr).unwrap();
    assert_ne!(right, wrong);
}

#[test]
fn digest_challenge_missing_nonce_returns_none() {
    assert!(DigestChallenge::parse(r#"Digest realm="asterisk""#).is_none());
}

#[test]
fn digest_challenge_missing_realm_returns_none() {
    assert!(DigestChallenge::parse(r#"Digest nonce="abc""#).is_none());
}

#[test]
fn build_challenge_response_bad_challenge_header_returns_none() {
    assert!(
        build_challenge_response("alice", "hunter2", "REGISTER", "sip:example.com", "garbage, not a digest challenge")
            .is_none()
    );
}

/// Commas embedded inside a quoted param value (allowed by RFC 2617's
/// quoted-string grammar) must not be mistaken for a param separator.
#[test]
fn parse_kv_pairs_ignores_commas_inside_quotes() {
    let challenge = DigestChallenge::parse(r#"Digest realm="Comma, Inside, Realm", nonce="abcdef0123456789""#).unwrap();
    assert_eq!(challenge.realm, "Comma, Inside, Realm");
    assert_eq!(challenge.nonce, "abcdef0123456789");
}
