use super::*;

#[test]
fn yields_message_with_no_body() {
    let mut framer = MessageFramer::new();
    framer.push(b"OPTIONS sip:foo SIP/2.0\r\nContent-Length: 0\r\n\r\n");
    let msg = framer.try_take_message().unwrap();
    assert_eq!(msg, b"OPTIONS sip:foo SIP/2.0\r\nContent-Length: 0\r\n\r\n");
    assert!(framer.try_take_message().is_none());
}

#[test]
fn waits_for_full_body_across_partial_reads() {
    let mut framer = MessageFramer::new();
    let full = b"INVITE sip:a SIP/2.0\r\nContent-Length: 5\r\n\r\nhello";

    framer.push(&full[..20]);
    assert!(framer.try_take_message().is_none());

    framer.push(&full[20..]);
    let msg = framer.try_take_message().unwrap();
    assert_eq!(msg, full.to_vec());
}

#[test]
fn drains_pipelined_messages_in_order() {
    let mut framer = MessageFramer::new();
    let first = b"OPTIONS sip:a SIP/2.0\r\nContent-Length: 0\r\n\r\n".to_vec();
    let second = b"OPTIONS sip:b SIP/2.0\r\nContent-Length: 3\r\n\r\nabc".to_vec();

    let mut combined = first.clone();
    combined.extend_from_slice(&second);
    framer.push(&combined);

    assert_eq!(framer.try_take_message().unwrap(), first);
    assert_eq!(framer.try_take_message().unwrap(), second);
    assert!(framer.try_take_message().is_none());
}

#[test]
fn defaults_content_length_to_zero_when_absent() {
    let mut framer = MessageFramer::new();
    framer.push(b"OPTIONS sip:a SIP/2.0\r\n\r\n");
    assert!(framer.try_take_message().is_some());
}
