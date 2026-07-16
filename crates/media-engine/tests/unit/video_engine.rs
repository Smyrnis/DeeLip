use super::*;
use crate::rtp::RtpSender;
use deelip_sip::SrtpSession;
use deelip_sip::sdp::{H264_PAYLOAD_TYPE, SrtpParams, VideoCodec};
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

/// Directly exercises `flush_frame`'s reorder-tolerant reassembly: a real
/// H.264 frame is encoded and fragmented, then fed into the same
/// `BTreeMap<u16, Vec<u8>>` shape `recv_loop` uses -- but inserted in
/// reverse (worst-case out-of-order) sequence, not arrival order. If
/// reassembly were still arrival-order-dependent (the bug this fix
/// addresses), decoding would fail or produce garbage; keying by sequence
/// number means insertion order can't matter.
#[test]
fn flush_frame_reassembles_correctly_despite_reversed_insertion_order() {
    let frame = Yuv420Frame::solid_color(64, 64, 100, 128, 128);
    let mut encoder = VideoEncoder::new(VideoCodec::H264, 300_000).unwrap();
    let bitstream = encoder.encode(&frame).unwrap();
    // A deliberately tiny MTU forces multiple fragments regardless of how
    // small the encoded keyframe happens to be, so this test doesn't
    // depend on the encoder's actual output size.
    let payloads = fragment_video_frame(VideoCodec::H264, &bitstream, 50);
    assert!(payloads.len() >= 3, "test needs several fragments to be meaningful");

    let mut fragments: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    for (seq, payload) in payloads.iter().enumerate().rev() {
        fragments.insert(seq as u16, payload.clone());
    }

    let mut decoder = VideoDecoder::new(VideoCodec::H264).unwrap();
    let latest: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
    flush_frame(VideoCodec::H264, &mut fragments, &mut decoder, &latest);

    let decoded = latest.lock().unwrap().clone().expect("frame should decode despite reversed fragment order");
    assert_eq!(decoded.width, 64);
    assert_eq!(decoded.height, 64);
    assert!(fragments.is_empty(), "flush_frame should clear the buffer");
}

async fn run_pair(
    alice_port: u16, bob_port: u16, alice_srtp: Option<SrtpSession>, bob_srtp: Option<SrtpSession>,
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
        VideoCodec::H264,
        alice_srtp,
        None,
        alice_source.clone(),
        30,
        300_000,
        None,
    )
    .await
    .unwrap();
    let bob = VideoEngine::start(
        bob_port,
        format!("127.0.0.1:{alice_port}").parse().unwrap(),
        VideoCodec::H264,
        bob_srtp,
        None,
        bob_source,
        30,
        300_000,
        None,
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
async fn muted_video_sends_nothing_until_unmuted() {
    let (_alice_source, alice, bob) = run_pair(41120, 41122, None, None).await;
    alice.set_muted(true);
    assert!(alice.is_muted());

    // While muted, Bob should never decode a frame -- give it a real window
    // to prove absence, not just check once.
    for _ in 0..20 {
        assert!(bob.latest_decoded_frame().is_none(), "Bob decoded a frame from a muted camera");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(alice.stats().packets_sent, 0, "Muted send loop should never have sent a packet");

    alice.set_muted(false);
    let mut got_bob_side = false;
    for _ in 0..40 {
        if bob.latest_decoded_frame().is_some() {
            got_bob_side = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(got_bob_side, "Bob should decode a frame once Alice unmutes");

    alice.stop().await;
    bob.stop().await;
}

/// A 3-way local conference: `host` fans one shared camera source out to
/// both `peer1` and `peer2` (its `second_leg`), decoding each of their
/// independent streams back on its own two `recv_loop`s -- mirrors what
/// `media.rs::start_conference` builds from two merged calls' negotiated
/// `VideoMediaReady`s.
#[tokio::test]
async fn conference_fans_out_to_both_legs_and_decodes_both_independently() {
    let host_source: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
    let h = host_source.clone();
    tokio::spawn(async move {
        for i in 0..30u8 {
            *h.lock().unwrap() = Some(Yuv420Frame::solid_color(64, 64, i, 128, 128));
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    });
    let peer1_source: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
    let p1 = peer1_source.clone();
    tokio::spawn(async move {
        for i in 0..30u8 {
            *p1.lock().unwrap() = Some(Yuv420Frame::solid_color(64, 64, 50u8.wrapping_add(i), 64, 64));
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    });
    let peer2_source: Arc<Mutex<Option<Yuv420Frame>>> = Arc::new(Mutex::new(None));
    let p2 = peer2_source.clone();
    tokio::spawn(async move {
        for i in 0..30u8 {
            *p2.lock().unwrap() = Some(Yuv420Frame::solid_color(64, 64, 200u8.wrapping_sub(i), 200, 200));
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    });

    let host_port = 41200;
    let peer1_port = 41202;
    let peer2_port = 41204;

    let peer1 = VideoEngine::start(
        peer1_port,
        format!("127.0.0.1:{host_port}").parse().unwrap(),
        VideoCodec::H264,
        None,
        None,
        peer1_source,
        30,
        300_000,
        None,
    )
    .await
    .unwrap();
    let host_leg2_port = host_port + 1;
    let peer2 = VideoEngine::start(
        peer2_port,
        format!("127.0.0.1:{host_leg2_port}").parse().unwrap(),
        VideoCodec::H264,
        None,
        None,
        peer2_source,
        30,
        300_000,
        None,
    )
    .await
    .unwrap();
    let host = VideoEngine::start(
        host_port,
        format!("127.0.0.1:{peer1_port}").parse().unwrap(),
        VideoCodec::H264,
        None,
        None,
        host_source,
        30,
        300_000,
        Some(VideoConferenceLeg {
            local_rtp_port: host_leg2_port,
            remote_rtp: format!("127.0.0.1:{peer2_port}").parse().unwrap(),
            codec: VideoCodec::H264,
            srtp: None,
            relay: None,
        }),
    )
    .await
    .unwrap();

    let mut got_peer1_side = false;
    let mut got_peer2_side = false;
    let mut got_host_leg1 = false;
    let mut got_host_leg2 = false;
    for _ in 0..40 {
        got_peer1_side |= peer1.latest_decoded_frame().is_some();
        got_peer2_side |= peer2.latest_decoded_frame().is_some();
        got_host_leg1 |= host.latest_decoded_frame().is_some();
        got_host_leg2 |= host.latest_decoded_frame_leg2().is_some();
        if got_peer1_side && got_peer2_side && got_host_leg1 && got_host_leg2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(got_peer1_side, "Peer1 should have decoded the host's fanned-out video");
    assert!(got_peer2_side, "Peer2 should have decoded the host's fanned-out video");
    assert!(got_host_leg1, "Host should have decoded peer1's video on leg 1");
    assert!(got_host_leg2, "Host should have decoded peer2's video on leg 2");

    host.stop().await;
    peer1.stop().await;
    peer2.stop().await;
}

#[tokio::test]
async fn encrypted_video_round_trips_over_real_udp() {
    let alice_key = SrtpParams::generate();
    let bob_key = SrtpParams::generate();
    // Each side encrypts with its own key and decrypts with the other's --
    // same RFC 4568 convention `SrtpSession` already encodes elsewhere.
    let alice_srtp = SrtpSession { local: alice_key.clone(), remote: bob_key.clone() };
    let bob_srtp = SrtpSession { local: bob_key, remote: alice_key };

    let (_alice_source, alice, bob) = run_pair(41110, 41112, Some(alice_srtp), Some(bob_srtp)).await;

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
