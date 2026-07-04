use deelip_config::{CallStatus, SipAccount};
use deelip_sip::{sdp, AudioCodec};
use egui::{RichText, Ui};

use crate::theme::Palette;

pub(crate) fn status_bar(ui: &mut Ui, palette: &Palette, text: &str, ok: bool, held: bool, new_voicemail: u32) {
    let color = if held {
        palette.warn
    } else if ok {
        palette.accent
    } else {
        palette.warn
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("●").color(color));
        ui.label(text);
        if new_voicemail > 0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{} {new_voicemail}", egui_phosphor::regular::VOICEMAIL))
                    .color(palette.accent));
            });
        }
    });
}

/// A 3x4 phone-style dial pad (1-9,*,0,#), each digit with the classic small
/// letter caption beneath it (2:ABC .. 9:WXYZ) -- shared between the compose
/// keypad and the in-call DTMF keypad, which were previously two near-identical
/// plain-square-button loops.
pub(crate) fn phone_keypad(ui: &mut Ui, palette: Palette, mut on_press: impl FnMut(char)) {
    const ROWS: [[char; 3]; 4] = [['1', '2', '3'], ['4', '5', '6'], ['7', '8', '9'], ['*', '0', '#']];
    ui.vertical_centered(|ui| {
        for row in ROWS {
            ui.horizontal(|ui| {
                for digit in row {
                    let button = egui::Button::new(keypad_button_text(digit, palette))
                        .rounding(egui::Rounding::same(28.0));
                    if ui.add_sized([56.0, 56.0], button).clicked() {
                        on_press(digit);
                    }
                }
            });
        }
    });
}

fn keypad_button_text(digit: char, palette: Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob { halign: egui::Align::Center, ..Default::default() };
    job.append(
        &digit.to_string(),
        0.0,
        egui::TextFormat { font_id: egui::FontId::proportional(20.0), ..Default::default() },
    );
    let letters = digit_letters(digit);
    if !letters.is_empty() {
        job.append(
            &format!("\n{letters}"),
            0.0,
            egui::TextFormat { font_id: egui::FontId::proportional(9.0), color: palette.muted, ..Default::default() },
        );
    }
    job
}

fn digit_letters(digit: char) -> &'static str {
    match digit {
        '2' => "ABC", '3' => "DEF", '4' => "GHI", '5' => "JKL", '6' => "MNO",
        '7' => "PQRS", '8' => "TUV", '9' => "WXYZ",
        _ => "",
    }
}

/// Display label for an account picker — `display_name` if set, else `user@server`.
pub(crate) fn account_label(account: &SipAccount) -> String {
    match account.display_name.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("{}@{}", account.username, account.server),
    }
}

pub(crate) fn status_filter_label(filter: &Option<CallStatus>) -> &'static str {
    match filter {
        None                        => "All",
        Some(CallStatus::Answered) => "Answered",
        Some(CallStatus::Missed)   => "Missed",
        Some(CallStatus::Rejected) => "Rejected",
        Some(CallStatus::Failed)   => "Failed",
    }
}

/// Quote a CSV field if it contains a comma, quote, or newline.
pub(crate) fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Shorten a SIP URI for display: `sip:alice@example.com` → `alice@example.com`.
pub(crate) fn short_uri(uri: &str) -> String {
    uri.strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))
        .unwrap_or(uri)
        .to_string()
}

/// Extract the user/number portion of a SIP URI for blocklist comparison,
/// e.g. `sip:5551234@host;user=phone` -> `"5551234"`. Bare entries (no
/// scheme/`@`) pass through unchanged (lowercased), so a blocklist entry can
/// be typed as either a plain number or a full SIP URI.
pub(crate) fn extract_user_part(uri: &str) -> String {
    let lower = uri.trim().to_ascii_lowercase();
    let stripped = lower.strip_prefix("sip:").or_else(|| lower.strip_prefix("sips:")).unwrap_or(&lower);
    let before_at = stripped.split('@').next().unwrap_or(stripped);
    before_at.split(';').next().unwrap_or(before_at).to_string()
}

/// Convert a `SipAccount::codec_order` entry to its `AudioCodec`. Unknown
/// entries (e.g. a stale name from a future version) are simply skipped by
/// callers via `filter_map`, not treated as an error.
fn codec_from_str(s: &str) -> Option<AudioCodec> {
    match s {
        "opus" => Some(AudioCodec::Opus),
        "g722" => Some(AudioCodec::G722),
        "pcmu" => Some(AudioCodec::Pcmu),
        "pcma" => Some(AudioCodec::Pcma),
        "gsm"  => Some(AudioCodec::Gsm),
        "ilbc" => Some(AudioCodec::Ilbc),
        _ => None,
    }
}

/// Display label for a `SipAccount::codec_order` entry in Settings.
pub(crate) fn codec_label(s: &str) -> &'static str {
    match s {
        "opus" => "Opus",
        "g722" => "G.722",
        "pcmu" => "PCMU (G.711 μ-law)",
        "pcma" => "PCMA (G.711 A-law)",
        "gsm"  => "GSM 06.10",
        "ilbc" => "iLBC",
        _ => "Unknown",
    }
}

/// This account's enabled codecs in preference order, ready to hand to
/// `build_offer`/`parse_sdp`. Falls back to every known codec if the
/// configured list is empty or entirely unrecognized — the Settings UI
/// itself refuses to let the last enabled codec be disabled, so this should
/// be unreachable in practice.
pub(crate) fn account_codecs(acc: &SipAccount) -> Vec<AudioCodec> {
    let codecs: Vec<AudioCodec> = acc.codec_order.iter().filter_map(|s| codec_from_str(s)).collect();
    if codecs.is_empty() { sdp::ALL_CODECS.to_vec() } else { codecs }
}

/// Normalize a dial-box entry into a full SIP URI. Bare numbers/usernames
/// (no scheme, no "@") are dialed against the account's own domain, matching
/// how MicroSIP and other softphones resolve local extensions.
pub(crate) fn normalize_target(raw: &str, domain: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("sip:") || lower.starts_with("sips:") {
        raw.to_string()
    } else if raw.contains('@') {
        format!("sip:{raw}")
    } else {
        format!("sip:{raw}@{domain}")
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn format_duration(secs: u32) -> String {
    if secs < 60 { format!("{secs}s") }
    else         { format!("{}m {:02}s", secs / 60, secs % 60) }
}

pub(crate) fn format_age(ts: u64) -> String {
    let age = unix_now().saturating_sub(ts);
    match age {
        0..=59              => format!("{age}s ago"),
        60..=3599           => format!("{}m ago", age / 60),
        3600..=86399        => format!("{}h ago", age / 3600),
        _                   => format!("{}d ago", age / 86400),
    }
}

pub(crate) fn ctx_key_enter(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

#[cfg(test)]
mod tests {
    use super::{account_codecs, extract_user_part, normalize_target};
    use deelip_config::SipAccount;
    use deelip_sip::{sdp, AudioCodec};

    #[test]
    fn bare_number_gets_domain_appended() {
        assert_eq!(normalize_target("600", "127.0.0.1"), "sip:600@127.0.0.1");
    }

    #[test]
    fn existing_sip_uri_is_untouched() {
        assert_eq!(normalize_target("sip:600@127.0.0.1", "example.com"), "sip:600@127.0.0.1");
    }

    #[test]
    fn sips_uri_is_untouched() {
        assert_eq!(normalize_target("sips:bob@example.com", "example.com"), "sips:bob@example.com");
    }

    #[test]
    fn user_at_host_without_scheme_gets_scheme_added() {
        assert_eq!(normalize_target("bob@example.com", "example.com"), "sip:bob@example.com");
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(normalize_target("  600  ", "127.0.0.1"), "sip:600@127.0.0.1");
    }

    #[test]
    fn extracts_user_from_bare_number() {
        assert_eq!(extract_user_part("5551234"), "5551234");
    }

    #[test]
    fn extracts_user_from_full_uri_with_params() {
        assert_eq!(extract_user_part("sip:5551234@host.example;user=phone"), "5551234");
    }

    #[test]
    fn extract_user_part_is_case_insensitive() {
        assert_eq!(extract_user_part("SIP:Bob@Example.com"), extract_user_part("sip:bob@example.com"));
    }

    #[test]
    fn account_codecs_honors_configured_order() {
        let mut acc = SipAccount::default();
        acc.codec_order = vec!["pcma".into(), "pcmu".into()];
        assert_eq!(account_codecs(&acc), vec![AudioCodec::Pcma, AudioCodec::Pcmu]);
    }

    #[test]
    fn account_codecs_falls_back_when_list_is_empty() {
        let mut acc = SipAccount::default();
        acc.codec_order = vec![];
        assert_eq!(account_codecs(&acc).len(), sdp::ALL_CODECS.len());
    }

    #[test]
    fn account_codecs_skips_unrecognized_entries() {
        let mut acc = SipAccount::default();
        acc.codec_order = vec!["opus".into(), "carrier-pigeon".into()];
        assert_eq!(account_codecs(&acc), vec![AudioCodec::Opus]);
    }
}
