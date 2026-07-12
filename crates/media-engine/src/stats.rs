//! Call statistics (`LegStats`/`CallStatsSnapshot`) and the loss/jitter
//! tracker that feeds them, for `engine::MediaEngine`. Split out of
//! `engine.rs` purely for file size (same precedent as `views/settings/`,
//! `views/dialer/`, `sip-core/src/call/lifecycle/`), not a behavior change.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::rtp::RtpPacket;

/// Local-only RTP stats for one leg — there's no RTCP in this codebase, so
/// loss/jitter reflect what *we* observe receiving, not what the remote
/// reports observing from us (the usual "local stats panel" scope, same as
/// what most softphones show without a full RTCP implementation).
#[derive(Debug, Clone, Default)]
pub struct LegStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub packets_received: u64,
    pub bytes_received: u64,
    /// Best-effort count of missing RTP sequence numbers on the receive
    /// side (gaps > 1000 are treated as reordering/restart noise, not loss).
    pub packets_lost: u64,
    /// RFC 3550 §6.4.1 interarrival jitter estimate, in milliseconds.
    pub jitter_ms: f64,
}

#[derive(Debug, Clone, Default)]
pub struct CallStatsSnapshot {
    pub leg1: LegStats,
    /// Only `Some` in conference mode (mirrors `MediaEngine::recv_task2`).
    pub leg2: Option<LegStats>,
}

pub(crate) type SharedStats = Arc<Mutex<CallStatsSnapshot>>;

/// Per-recv-task running state for loss/jitter calculation — deliberately
/// not part of the shared/lockable `LegStats` since only the owning recv
/// task ever touches it.
#[derive(Default)]
pub(crate) struct JitterTracker {
    last_seq: Option<u16>,
    last_arrival: Option<Instant>,
    last_rtp_ts: Option<u32>,
}

impl JitterTracker {
    /// Update loss/jitter running state from a newly-received voice packet
    /// and fold the results into `stats`.
    pub(crate) fn observe(&mut self, stats: &mut LegStats, pkt: &RtpPacket, clock_hz: f64) {
        if let Some(prev) = self.last_seq {
            let expected = prev.wrapping_add(1);
            if pkt.sequence != expected {
                let gap = pkt.sequence.wrapping_sub(expected);
                if gap < 1000 {
                    stats.packets_lost += gap as u64;
                }
            }
        }
        self.last_seq = Some(pkt.sequence);

        let now = Instant::now();
        if let (Some(prev_arrival), Some(prev_ts)) = (self.last_arrival, self.last_rtp_ts) {
            let arrival_diff_ms = now.duration_since(prev_arrival).as_secs_f64() * 1000.0;
            let rtp_diff_ms = (pkt.timestamp as i64 - prev_ts as i64).unsigned_abs() as f64 / clock_hz * 1000.0;
            let d = (arrival_diff_ms - rtp_diff_ms).abs();
            stats.jitter_ms += (d - stats.jitter_ms) / 16.0;
        }
        self.last_arrival = Some(now);
        self.last_rtp_ts = Some(pkt.timestamp);
    }
}
