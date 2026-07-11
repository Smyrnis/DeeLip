use super::*;

#[test]
fn shared_gain_defaults_to_unity() {
    let gain = new_shared_gain();
    assert_eq!(load_gain(&gain), 1.0);
}

#[test]
fn shared_gain_round_trips() {
    let gain = new_shared_gain();
    store_gain(&gain, 0.42);
    assert_eq!(load_gain(&gain), 0.42);
}

#[test]
fn push_frame_to_echo_ref_none_is_a_noop() {
    push_frame_to_echo_ref(&None, &[1, 2, 3]);
}

#[test]
fn push_frame_to_echo_ref_appends_in_order() {
    let echo_ref: EchoRefBuf = Arc::new(Mutex::new(VecDeque::new()));
    push_frame_to_echo_ref(&Some(echo_ref.clone()), &[1, 2, 3]);
    push_frame_to_echo_ref(&Some(echo_ref.clone()), &[4, 5]);
    let buf = echo_ref.lock().unwrap();
    assert_eq!(buf.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
}

#[test]
fn push_frame_to_echo_ref_caps_at_one_second() {
    let echo_ref: EchoRefBuf = Arc::new(Mutex::new(VecDeque::new()));
    let max = FRAME_SAMPLES * 50;
    let frame = vec![7i16; max + 500];
    push_frame_to_echo_ref(&Some(echo_ref.clone()), &frame);
    assert_eq!(echo_ref.lock().unwrap().len(), max);
}
