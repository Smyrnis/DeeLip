use super::*;

#[test]
fn parses_open_and_closed_basic() {
    let open = "<?xml version=\"1.0\"?><presence><tuple id=\"a\"><status><basic>open</basic></status></tuple></presence>";
    assert_eq!(parse_pidf_basic(open), Some(PresenceState::Available));
    let closed = "<presence><tuple><status><basic>closed</basic></status></tuple></presence>";
    assert_eq!(parse_pidf_basic(closed), Some(PresenceState::Offline));
}

#[test]
fn missing_basic_element_returns_none() {
    assert_eq!(parse_pidf_basic("<presence></presence>"), None);
}

#[test]
fn unrecognized_basic_value_returns_none() {
    assert_eq!(parse_pidf_basic("<basic>maybe</basic>"), None);
}

#[test]
fn parses_subscription_state_active_with_expires() {
    let (state, expires) = parse_subscription_state("active;expires=3600");
    assert_eq!(state, "active");
    assert_eq!(expires, Some(3600));
}

#[test]
fn parses_subscription_state_terminated_with_retry_after() {
    let (state, retry) = parse_subscription_state("terminated;reason=deactivated;retry-after=60");
    assert_eq!(state, "terminated");
    assert_eq!(retry, Some(60));
}

#[test]
fn parses_bare_subscription_state_with_no_params() {
    let (state, param) = parse_subscription_state("active");
    assert_eq!(state, "active");
    assert_eq!(param, None);
}
