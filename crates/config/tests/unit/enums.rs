use super::*;

// ── Transport protocol ────────────────────────────────────────────────────

#[test]
fn transport_protocol_round_trips_every_variant_through_known_strings() {
    let cases = [
        (TransportProtocol::Udp, "udp"),
        (TransportProtocol::Tcp, "tcp"),
        (TransportProtocol::Tls, "tls"),
        (TransportProtocol::Auto, "auto"),
    ];
    for (variant, s) in cases {
        assert_eq!(transport_to_str(&variant), s);
        assert_eq!(transport_from_str(s), variant);
    }
}

#[test]
fn transport_protocol_from_str_falls_back_to_udp() {
    assert_eq!(transport_from_str("nonsense"), TransportProtocol::Udp);
    assert_eq!(transport_from_str(""), TransportProtocol::Udp);
}

// ── Media encryption ───────────────────────────────────────────────────────

#[test]
fn media_encryption_round_trips_every_variant_through_known_strings() {
    let cases = [
        (MediaEncryption::MatchTransport, "match_transport"),
        (MediaEncryption::Disabled, "disabled"),
        (MediaEncryption::Enabled, "enabled"),
        (MediaEncryption::Zrtp, "zrtp"),
        (MediaEncryption::DtlsSrtp, "dtls_srtp"),
    ];
    for (variant, s) in cases {
        assert_eq!(media_encryption_to_str(variant), s);
        assert_eq!(media_encryption_from_str(s), variant);
    }
}

#[test]
fn media_encryption_from_str_falls_back_to_match_transport() {
    assert_eq!(media_encryption_from_str("garbage"), MediaEncryption::MatchTransport);
}

// ── DTMF mode ──────────────────────────────────────────────────────────────

#[test]
fn dtmf_mode_round_trips_every_variant_through_known_strings() {
    let cases = [
        (DtmfMode::Rfc2833, "rfc2833"),
        (DtmfMode::SipInfo, "sipinfo"),
        (DtmfMode::Inband, "inband"),
        (DtmfMode::Auto, "auto"),
    ];
    for (variant, s) in cases {
        assert_eq!(dtmf_mode_to_str(variant), s);
        assert_eq!(dtmf_mode_from_str(s), variant);
    }
}

#[test]
fn dtmf_mode_from_str_falls_back_to_rfc2833() {
    assert_eq!(dtmf_mode_from_str("whatever"), DtmfMode::Rfc2833);
}

// ── Update check frequency ─────────────────────────────────────────────────

#[test]
fn update_check_frequency_round_trips_every_variant_through_known_strings() {
    let cases = [
        (UpdateCheckFrequency::Always, "always"),
        (UpdateCheckFrequency::Daily, "daily"),
        (UpdateCheckFrequency::Weekly, "weekly"),
        (UpdateCheckFrequency::Never, "never"),
    ];
    for (variant, s) in cases {
        assert_eq!(update_check_frequency_to_str(variant), s);
        assert_eq!(update_check_frequency_from_str(s), variant);
    }
}

#[test]
fn update_check_frequency_from_str_falls_back_to_always() {
    assert_eq!(update_check_frequency_from_str("bogus"), UpdateCheckFrequency::Always);
}

#[test]
fn update_check_frequency_is_due_respects_min_interval() {
    // Always: due even immediately after a check.
    assert!(UpdateCheckFrequency::Always.is_due(Some(1_000), 1_000));
    // Never: never due, no matter how stale.
    assert!(!UpdateCheckFrequency::Never.is_due(Some(0), 1_000_000));
    // Daily: not due before 24h elapsed, due right at/after.
    let day = 24 * 3600;
    assert!(!UpdateCheckFrequency::Daily.is_due(Some(1_000), 1_000 + day - 1));
    assert!(UpdateCheckFrequency::Daily.is_due(Some(1_000), 1_000 + day));
    // No prior check recorded: always due regardless of frequency (except Never).
    assert!(UpdateCheckFrequency::Weekly.is_due(None, 0));
    assert!(!UpdateCheckFrequency::Never.is_due(None, 0));
}

// ── Default list action ───────────────────────────────────────────────────

#[test]
fn default_list_action_round_trips_every_variant_through_known_strings() {
    let cases =
        [(DefaultListAction::Call, "call"), (DefaultListAction::Message, "message"), (DefaultListAction::Edit, "edit")];
    for (variant, s) in cases {
        assert_eq!(default_list_action_to_str(variant), s);
        assert_eq!(default_list_action_from_str(s), variant);
    }
}

#[test]
fn default_list_action_from_str_falls_back_to_call() {
    assert_eq!(default_list_action_from_str("nope"), DefaultListAction::Call);
}

// ── Recording format ───────────────────────────────────────────────────────

#[test]
fn recording_format_round_trips_every_variant_through_known_strings() {
    let cases = [(RecordingFormat::Wav, "wav"), (RecordingFormat::Mp3, "mp3")];
    for (variant, s) in cases {
        assert_eq!(recording_format_to_str(variant), s);
        assert_eq!(recording_format_from_str(s), variant);
    }
}

#[test]
fn recording_format_from_str_falls_back_to_wav() {
    assert_eq!(recording_format_from_str("ogg"), RecordingFormat::Wav);
}

// ── Language ───────────────────────────────────────────────────────────────

#[test]
fn language_round_trips_its_only_variant_and_falls_back_to_it() {
    assert_eq!(language_to_str(Language::En), "en");
    assert_eq!(language_from_str("en"), Language::En);
    assert_eq!(language_from_str("fr"), Language::En);
}
