use super::*;

// `tests/unit/engine.rs` already covers `JitterTracker::observe`'s basic
// loss-gap-counting and huge-gap-ignored behavior (leftover from before
// this struct moved into its own `stats.rs` module) -- these tests add the
// angles that aren't already covered there: the very first observation
// (no prior state to diff against), a backward/duplicate sequence number
// specifically (vs. the existing test's forward-only big jump), and a real
// convergence check of the RFC 3550 §6.4.1 jitter EMA toward a known
// value, which the existing "stays small over 3 packets" test doesn't
// attempt.

fn pkt(sequence: u16, timestamp: u32) -> RtpPacket {
    RtpPacket::new(0, sequence, timestamp, 0xdead_beef, vec![0u8; 160])
}

#[test]
fn first_packet_sets_baseline_without_touching_loss_or_jitter() {
    let mut tracker = JitterTracker::default();
    let mut stats = LegStats::default();
    tracker.observe(&mut stats, &pkt(100, 0), 8000.0);
    assert_eq!(stats.packets_lost, 0);
    assert_eq!(stats.jitter_ms, 0.0, "jitter needs two arrivals before it means anything");
}

#[test]
fn backward_sequence_jump_is_treated_as_reorder_not_loss() {
    let mut tracker = JitterTracker::default();
    let mut stats = LegStats::default();
    tracker.observe(&mut stats, &pkt(1000, 0), 8000.0);
    // A duplicate/very-late/out-of-order packet with a much smaller
    // sequence than expected wraps `gap` (computed via `wrapping_sub`)
    // close to u16::MAX (>= 1000), which `observe` deliberately treats as
    // reordering/restart noise rather than ~64k lost packets.
    tracker.observe(&mut stats, &pkt(1, 160), 8000.0);
    assert_eq!(stats.packets_lost, 0, "a backward sequence jump must not be miscounted as massive loss");
}

#[test]
fn jitter_estimate_converges_to_the_known_interarrival_mismatch() {
    // RFC 3550 §6.4.1's interarrival jitter is an exponential moving
    // average (gain 1/16) of |arrival_diff_ms - rtp_diff_ms|. Driving
    // `observe` back-to-back (no sleep) keeps the real wall-clock arrival
    // delta negligible (microseconds) on every iteration, while the RTP
    // timestamp advances by a fixed 400 samples/call at an 8kHz clock --
    // i.e. rtp_diff_ms is a constant ~50ms every step. That makes the
    // per-step mismatch `d` an effectively constant ~50ms, so the EMA
    // should converge to ~50ms without depending on real-time sleep
    // precision (avoids a flaky, scheduler-dependent test).
    let mut tracker = JitterTracker::default();
    let mut stats = LegStats::default();
    let clock_hz = 8000.0;

    let mut seq = 0u16;
    let mut ts = 0u32;
    for _ in 0..300 {
        tracker.observe(&mut stats, &pkt(seq, ts), clock_hz);
        seq = seq.wrapping_add(1);
        ts = ts.wrapping_add(400); // 400/8000 * 1000 = 50ms per step
    }

    assert!((stats.jitter_ms - 50.0).abs() < 1.0, "expected jitter to converge near 50ms, got {}", stats.jitter_ms);
}
