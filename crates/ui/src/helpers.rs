use deelip_config::{CallStatus, SipAccount};
use deelip_sip::{sdp, AudioCodec};
use egui::{RichText, Ui};

use crate::theme::Palette;

/// Left-hand side of the bottom status bar -- just the connection dot and
/// status text. Caller wraps this in its own `ui.horizontal()` alongside a
/// right-to-left cluster (voicemail badge / DND toggle / account label) so
/// everything shares one row, MicroSIP-style.
pub(crate) fn status_bar(ui: &mut Ui, palette: &Palette, text: &str, ok: bool, held: bool) {
    let color = if held {
        palette.warn
    } else if ok {
        palette.accent
    } else {
        palette.warn
    };
    ui.label(RichText::new("●").color(color));
    ui.label(text);
}

/// Paint a subtle divider line along a list row's bottom edge, shared by
/// History/Contacts/Messages so all three read as one consistent list
/// design instead of three independently-styled dividers. Row content must
/// be a single widget (e.g. one `ui.horizontal()`) whose response `rect` is
/// passed in here -- a second sibling widget for the divider would add an
/// extra `item_spacing.y` gap that per-row height estimates (needed for
/// `show_rows` virtualization) can't represent.
pub(crate) fn list_row_divider(ui: &Ui, palette: &Palette, row_rect: egui::Rect) {
    ui.painter().hline(row_rect.x_range(), row_rect.bottom(), egui::Stroke::new(1.0, palette.divider));
}

/// Render one list row: `add_contents` inside a single `ui.horizontal`, with
/// a hover-highlight background and a bottom divider -- shared by
/// History/Contacts/Messages so hovering any list row gives the same
/// feedback everywhere. The highlight uses egui's standard "reserve a shape
/// slot before the content, fill it in once the row's rect/hover state are
/// known" trick, since otherwise a background painted *after* the row's own
/// widgets would draw on top of them instead of behind.
///
/// `id_source` must be unique per row (e.g. the row's index): egui derives
/// `ui.horizontal()`'s child id purely from the *parent* ui's id plus the
/// fixed literal "child", so every row rendered from the same virtualized
/// `show_rows` loop would otherwise get the exact same id. `Response::hovered`
/// is a lookup by that id into a per-frame hovered-id set, so with colliding
/// ids, hovering one row marked *every* row hovered simultaneously. Wrapping
/// in `ui.push_id` salts the id per row so only the actual hovered row lights up.
pub(crate) fn list_row(ui: &mut Ui, palette: &Palette, id_source: impl std::hash::Hash, add_contents: impl FnOnce(&mut Ui)) {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let row = ui.push_id(id_source, |ui| ui.horizontal(add_contents)).inner.response;
    if row.hovered() {
        ui.painter().set(bg_idx, egui::Shape::rect_filled(row.rect, 0.0, palette.row_hover));
    }
    list_row_divider(ui, palette, row.rect);
}

/// A small "ⓘ" icon that reveals `text` as a tooltip on hover -- Settings'
/// replacement for always-visible small-gray-text footnotes ("Applies
/// immediately -- no restart needed.", etc.), so each section/field reads as
/// one line with the explanation tucked away instead of a wall of captions.
pub(crate) fn info_hint(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(egui_phosphor::regular::INFO).color(palette.muted))
        .on_hover_text(text);
}

/// A registration-status dot (`palette.accent` green when registered,
/// `palette.muted` otherwise) followed by the plain-colored account label,
/// as one `LayoutJob` -- so account pickers read the same "online" green as
/// the main status bar's dot instead of an uncolored `●`/`○` character
/// baked into a plain string.
pub(crate) fn account_status_label(ui: &Ui, palette: &Palette, reg_ok: bool, label: &str) -> egui::text::LayoutJob {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let dot_color = if reg_ok { palette.accent } else { palette.muted };
    let mut job = egui::text::LayoutJob::default();
    job.append("● ", 0.0, egui::TextFormat { font_id: font_id.clone(), color: dot_color, ..Default::default() });
    job.append(label, 0.0, egui::TextFormat { font_id, color: ui.visuals().text_color(), ..Default::default() });
    job
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
#[path = "../tests/unit/helpers.rs"]
mod tests;
