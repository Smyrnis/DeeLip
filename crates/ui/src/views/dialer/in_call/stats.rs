//! Renders one leg's RTP stats inside the "Call statistics" collapsing
//! section.

use egui::{RichText, Ui};

use deelip_sip::AudioCodec;

use crate::helpers::audio_codec_label;
use crate::strings::tf;

/// Render one leg's RTP stats as a small label grid inside a "Call
/// statistics" collapsing section.
pub(super) fn show_leg_stats(
    ui: &mut Ui, label: &str, codec: AudioCodec, stats: &deelip_media::LegStats, muted: egui::Color32,
) {
    let codec_name = audio_codec_label(codec);
    ui.label(RichText::new(format!("{label} — {codec_name}")).strong());
    ui.label(
        RichText::new(tf(
            "dialer.stats_sent_received",
            &[
                ("sent_pkts", &stats.packets_sent.to_string()),
                ("sent_bytes", &format_bytes(stats.bytes_sent)),
                ("recv_pkts", &stats.packets_received.to_string()),
                ("recv_bytes", &format_bytes(stats.bytes_received)),
            ],
        ))
        .color(muted)
        .small(),
    );
    let loss_pct = if stats.packets_received + stats.packets_lost > 0 {
        100.0 * stats.packets_lost as f64 / (stats.packets_received + stats.packets_lost) as f64
    } else {
        0.0
    };
    ui.label(
        RichText::new(tf(
            "dialer.stats_loss_jitter",
            &[
                ("lost", &stats.packets_lost.to_string()),
                ("pct", &format!("{loss_pct:.1}")),
                ("jitter", &format!("{:.1}", stats.jitter_ms)),
            ],
        ))
        .color(muted)
        .small(),
    );
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} B") } else { format!("{:.1} KB", bytes as f64 / 1024.0) }
}
