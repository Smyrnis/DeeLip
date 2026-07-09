use super::*;

fn rule(pattern: &str, replacement: &str) -> DialPlanRule {
    DialPlanRule {
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
        enabled: true,
    }
}

#[test]
fn no_rules_returns_none() {
    assert_eq!(apply_dial_plan("1234", &[]), None);
}

#[test]
fn first_matching_rule_wins() {
    let rules = vec![
        rule(r"^0(\d+)$", "$1"),   // strip leading trunk-access 0
        rule(r"^(\d{4})$", "9$1"), // 4-digit extensions get an outside-line prefix
    ];
    assert_eq!(apply_dial_plan("01234", &rules), Some("1234".to_string()));
    assert_eq!(apply_dial_plan("1234", &rules), Some("91234".to_string()));
    assert_eq!(apply_dial_plan("12345", &rules), None);
}

#[test]
fn disabled_rule_is_skipped() {
    let mut r = rule(r"^(\d+)$", "9$1");
    r.enabled = false;
    assert_eq!(apply_dial_plan("1234", &[r]), None);
}

#[test]
fn non_matching_pattern_falls_through_to_next_rule() {
    let rules = vec![rule(r"^911$", "911"), rule(r"^(\d+)$", "9$1")];
    assert_eq!(apply_dial_plan("1234", &rules), Some("91234".to_string()));
}

#[test]
fn invalid_regex_is_skipped_not_fatal() {
    let rules = vec![rule("(unclosed", "x"), rule(r"^(\d+)$", "9$1")];
    assert_eq!(apply_dial_plan("1234", &rules), Some("91234".to_string()));
}
