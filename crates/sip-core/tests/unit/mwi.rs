use super::*;

#[test]
fn parses_waiting_with_counts() {
    let body = "Messages-Waiting: yes\r\nMessage-Account: sip:1000@example.com\r\nVoice-Message: 3/2 (0/0)\r\n";
    let state = parse_mwi_summary(body).unwrap();
    assert_eq!(
        state,
        MwiState {
            waiting: true,
            new_messages: 3,
            old_messages: 2
        }
    );
}

#[test]
fn parses_not_waiting() {
    let body = "Messages-Waiting: no\r\n";
    let state = parse_mwi_summary(body).unwrap();
    assert_eq!(
        state,
        MwiState {
            waiting: false,
            new_messages: 0,
            old_messages: 0
        }
    );
}

#[test]
fn missing_messages_waiting_line_returns_none() {
    assert_eq!(parse_mwi_summary("Voice-Message: 1/0\r\n"), None);
}

#[test]
fn waiting_without_voice_message_line_defaults_counts_to_zero() {
    let state = parse_mwi_summary("Messages-Waiting: yes\r\n").unwrap();
    assert_eq!(
        state,
        MwiState {
            waiting: true,
            new_messages: 0,
            old_messages: 0
        }
    );
}
