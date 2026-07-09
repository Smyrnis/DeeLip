use super::*;
use crate::rtp::RtpSender;
use deelip_sip::sdp::{H264_PAYLOAD_TYPE, SrtpParams};
use deelip_sip::SrtpSession;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The zero-`ts_increment` `RtpSender` trick this module's send task relies
/// on, isolated from the full network round trip: fragments of one frame
/// must share a timestamp (only `sequence` advances per `next_packet`
/// call), and a manual bump moves the clock to the next frame.
#[test]
fn zero_ts_increment_sender_shares_timestamp_within_a_frame() {
    let mut sender = RtpSender::new(H264_PAYLOAD_TYPE, 0);
    let p1 = sender.next_packet(vec![1]);
    let p2 = sender.next_packet(vec![2]);
    let p3 = sender.next_packet(vec![3]);

    assert_eq!(p1.timestamp, p2.timestamp);
    assert_eq!(p2.timestamp, p3.timestamp);
    assert_eq!(p2.sequence, p1.sequence.wrapping_add(1));
    assert_eq!(p3.sequence, p2.sequence.wrapping_add(1));

    let before = sender.timestamp;
    sender.timestamp = sender.timestamp.wrapping_add(3000); // e.g. 90000/30fps
    let p4 = sender.next_packet(vec![4]);
    assert_eq!(p4.timestamp, before.wrapping_add(3000));
}

async fn run_pair(
    alice_port: u16,
    bob_port: u16,
    alice_srtp: Option<SrtpSession>,
    bob_srtp: Option<SrtpSession>,
) -> (Arc<Mutex<Option<Yuv420Frame>>>, VideoEngine, VideoEngine) {
    let alice_source: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
    let bob_source: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));

    // Simulate a camera on each side: overwrite the frame source with a
    // fresh synthetic frame a few times a second, exactly the shape a real
    // `video_capture::CaptureHandle` would drive this same slot with.
    let a = alice_source.clone();
    tokio::spawn(async move {
        for i in 0..30u8 {
            *a.lock().unwrap() = Some(Yuv420Frame::solid_color(64, 64, 100u8.wrapping_add(i), 128, 128));
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    });
    let b = bob_source.clone();
    tokio::spawn(async move {
        for i in 0..30u8 {
            *b.lock().unwrap() = Some(Yuv420Frame::solid_color(64, 64, 200u8.wrapping_sub(i), 64, 64));
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    });

    let alice = VideoEngine::start(
        alice_port,
        format!("127.0.0.1:{bob_port}").parse().unwrap(),
        alice_srtp,
        None,
        alice_source.clone(),
        30,
        300_000,
    )
    .await
    .unwrap();
    let bob = VideoEngine::start(
        bob_port,
        format!("127.0.0.1:{alice_port}").parse().unwrap(),
        bob_srtp,
        None,
        bob_source,
        30,
        300_000,
    )
    .await
    .unwrap();

    (alice_source, alice, bob)
}

#[tokio::test]
async fn plaintext_video_round_trips_over_real_udp() {
    let (_alice_source, alice, bob) = run_pair(41100, 41102, None, None).await;

    let mut got_alice_side = false;
    let mut got_bob_side = false;
    for _ in 0..40 {
        if alice.latest_decoded_frame().is_some() {
            got_alice_side = true;
        }
        if bob.latest_decoded_frame().is_some() {
            got_bob_side = true;
        }
        if got_alice_side && got_bob_side {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(got_bob_side, "Bob should have decoded a frame from Alice");
    assert!(got_alice_side, "Alice should have decoded a frame from Bob");
    let frame = bob.latest_decoded_frame().unwrap();
    assert_eq!(frame.width, 64);
    assert_eq!(frame.height, 64);

    let alice_stats = alice.stats();
    assert!(alice_stats.packets_sent > 0);
    let bob_stats = bob.stats();
    assert!(bob_stats.packets_received > 0);

    alice.stop().await;
    bob.stop().await;
}

#[tokio::test]
async fn encrypted_video_round_trips_over_real_udp() {
    let alice_key = SrtpParams::generate();
    let bob_key = SrtpParams::generate();
    // Each side encrypts with its own key and decrypts with the other's --
    // same RFC 4568 convention `SrtpSession` already encodes elsewhere.
    let alice_srtp = SrtpSession { local: alice_key.clone(), remote: bob_key.clone() };
    let bob_srtp = SrtpSession { local: bob_key, remote: alice_key };

    let (_alice_source, alice, bob) =
        run_pair(41110, 41112, Some(alice_srtp), Some(bob_srtp)).await;

    let mut got_bob_side = false;
    for _ in 0..40 {
        if bob.latest_decoded_frame().is_some() {
            got_bob_side = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(got_bob_side, "Bob should have decoded Alice's encrypted video");

    alice.stop().await;
    bob.stop().await;
}
