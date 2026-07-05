use deelip_config::{CallStatus, SipAccount};
use deelip_sip::AudioCodec;
use egui::{RichText, Ui};

use crate::theme::{self, Palette};

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
    ui.painter().hline(
        row_rect.x_range(),
        row_rect.bottom(),
        egui::Stroke::new(1.0, palette.divider),
    );
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
pub(crate) fn list_row(
    ui: &mut Ui,
    palette: &Palette,
    id_source: impl std::hash::Hash,
    add_contents: impl FnOnce(&mut Ui),
) {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let row = ui
        .push_id(id_source, |ui| ui.horizontal(add_contents))
        .inner
        .response;
    if row.hovered() {
        ui.painter().set(
            bg_idx,
            egui::Shape::rect_filled(row.rect, 0.0, palette.row_hover),
        );
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

/// One Settings section: a bold title (with an optional `info_hint` beside
/// it) followed by a `full_width_card`. Every section in `views/settings.rs`
/// repeated this same header+card scaffolding by hand; factored out so the
/// header treatment can't drift between sections (some previously had a
/// hint, some didn't, with no reason for the difference).
pub(crate) fn settings_section<R>(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    hint: Option<&str>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        if let Some(hint) = hint {
            info_hint(ui, palette, hint);
        }
    });
    theme::full_width_card(ui, *palette, add_contents)
}

/// One row of a device-picker `ComboBox` bound to `Option<String>` (`None`
/// = "Default") -- the Settings Audio section had this same shape three
/// times over (input/output/ringtone device), differing only in label,
/// bound field, and candidate list.
pub(crate) fn device_picker(
    ui: &mut Ui,
    id_source: &str,
    label: &str,
    current: &mut Option<String>,
    names: &[String],
) -> bool {
    let mut changed = false;
    ui.label(label);
    let selected = current.clone().unwrap_or_else(|| "Default".into());
    egui::ComboBox::from_id_source(id_source)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "Default").clicked() {
                *current = None;
                changed = true;
            }
            for name in names {
                let is_sel = current.as_deref() == Some(name.as_str());
                if ui.selectable_label(is_sel, name).clicked() {
                    *current = Some(name.clone());
                    changed = true;
                }
            }
        });
    changed
}

/// Muted, small "nothing here" label -- the shared style for every list's
/// empty state (History/Messages/Contacts/Settings' blocklist), so a list
/// that gains this treatment later can't render as a differently-styled
/// plain label by accident.
pub(crate) fn empty_state(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).color(palette.muted).small());
}

/// Prompt for a save location (via `rfd`) and write `content` to it,
/// logging (not surfacing to the UI -- matches this codebase's existing
/// export-failure handling) on error. Shared by History's CSV export and
/// Contacts' CSV/vCard export, which each hand-rolled the same
/// dialog+write+log-on-error sequence.
pub(crate) fn save_text_file(
    default_name: &str,
    filter_name: &str,
    filter_ext: &str,
    content: String,
) {
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter(filter_name, &[filter_ext])
        .save_file()
    else {
        return;
    };
    if let Err(e) = std::fs::write(&path, content) {
        tracing::error!("Failed to write {}: {e}", path.display());
    }
}

/// A registration-status dot (`palette.accent` green when registered,
/// `palette.muted` otherwise) followed by the plain-colored account label,
/// as one `LayoutJob` -- so account pickers read the same "online" green as
/// the main status bar's dot instead of an uncolored `●`/`○` character
/// baked into a plain string.
pub(crate) fn account_status_label(
    ui: &Ui,
    palette: &Palette,
    reg_ok: bool,
    label: &str,
) -> egui::text::LayoutJob {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let dot_color = if reg_ok {
        palette.accent
    } else {
        palette.muted
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "● ",
        0.0,
        egui::TextFormat {
            font_id: font_id.clone(),
            color: dot_color,
            ..Default::default()
        },
    );
    job.append(
        label,
        0.0,
        egui::TextFormat {
            font_id,
            color: ui.visuals().text_color(),
            ..Default::default()
        },
    );
    job
}

/// A 3x4 phone-style dial pad (1-9,*,0,#), each digit with the classic small
/// letter caption beneath it (2:ABC .. 9:WXYZ) -- shared between the compose
/// keypad and the in-call DTMF keypad, which were previously two near-identical
/// plain-square-button loops.
pub(crate) fn phone_keypad(ui: &mut Ui, palette: Palette, mut on_press: impl FnMut(char)) {
    const ROWS: [[char; 3]; 4] = [
        ['1', '2', '3'],
        ['4', '5', '6'],
        ['7', '8', '9'],
        ['*', '0', '#'],
    ];
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
    let mut job = egui::text::LayoutJob {
        halign: egui::Align::Center,
        ..Default::default()
    };
    job.append(
        &digit.to_string(),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(20.0),
            ..Default::default()
        },
    );
    let letters = digit_letters(digit);
    if !letters.is_empty() {
        job.append(
            &format!("\n{letters}"),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(9.0),
                color: palette.muted,
                ..Default::default()
            },
        );
    }
    job
}

fn digit_letters(digit: char) -> &'static str {
    match digit {
        '2' => "ABC",
        '3' => "DEF",
        '4' => "GHI",
        '5' => "JKL",
        '6' => "MNO",
        '7' => "PQRS",
        '8' => "TUV",
        '9' => "WXYZ",
        _ => "",
    }
}

/// Display label for an account picker — `display_name` if set, else `user@server`.
pub(crate) fn account_label(account: &SipAccount) -> String {
    match account
        .account_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| account.display_name.as_deref().filter(|s| !s.is_empty()))
    {
        Some(name) => name.to_string(),
        None => format!("{}@{}", account.username, account.server),
    }
}

pub(crate) fn status_filter_label(filter: &Option<CallStatus>) -> &'static str {
    match filter {
        None => "All",
        Some(CallStatus::Answered) => "Answered",
        Some(CallStatus::Missed) => "Missed",
        Some(CallStatus::Rejected) => "Rejected",
        Some(CallStatus::Failed) => "Failed",
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
    let stripped = lower
        .strip_prefix("sip:")
        .or_else(|| lower.strip_prefix("sips:"))
        .unwrap_or(&lower);
    let before_at = stripped.split('@').next().unwrap_or(stripped);
    before_at.split(';').next().unwrap_or(before_at).to_string()
}

/// Display label for a `SipAccount::codec_order` entry in Settings.
pub(crate) fn codec_label(s: &str) -> &'static str {
    match s {
        "opus" => "Opus",
        "g722" => "G.722",
        "pcmu" => "PCMU (G.711 μ-law)",
        "pcma" => "PCMA (G.711 A-law)",
        "gsm" => "GSM 06.10",
        "ilbc" => "iLBC",
        "g729" => "G.729",
        _ => "Unknown",
    }
}

/// Same table as `codec_label`, keyed by `AudioCodec` directly -- for
/// displaying an already-negotiated codec (e.g. call statistics) rather
/// than a `SipAccount::codec_order` entry.
pub(crate) fn audio_codec_label(codec: AudioCodec) -> &'static str {
    codec_label(match codec {
        AudioCodec::Opus => "opus",
        AudioCodec::G722 => "g722",
        AudioCodec::Pcmu => "pcmu",
        AudioCodec::Pcma => "pcma",
        AudioCodec::Gsm => "gsm",
        AudioCodec::Ilbc => "ilbc",
        AudioCodec::G729 => "g729",
    })
}

/// Normalize a dial-box entry into a full SIP URI. Bare numbers/usernames
/// (no scheme, no "@") are dialed against the account's own domain, matching
/// how MicroSIP and other softphones resolve local extensions.
pub(crate) fn normalize_target(raw: &str, domain: &str) -> String {
    normalize_target_with_prefix(raw, domain, "")
}

/// Same as `normalize_target`, but auto-prepends `prefix` (e.g. "9" for an
/// outside line) to a bare number before appending the domain -- only the
/// bare-number case gets it, since a full SIP URI or an explicit `user@host`
/// entry is already a specific destination, not a local extension to dial
/// out from.
pub(crate) fn normalize_target_with_prefix(raw: &str, domain: &str, prefix: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("sip:") || lower.starts_with("sips:") {
        raw.to_string()
    } else if raw.contains('@') {
        format!("sip:{raw}")
    } else {
        format!("sip:{prefix}{raw}@{domain}")
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn format_duration(secs: u32) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

pub(crate) fn format_age(ts: u64) -> String {
    let age = unix_now().saturating_sub(ts);
    match age {
        0..=59 => format!("{age}s ago"),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86399 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86400),
    }
}

pub(crate) fn ctx_key_enter(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

#[cfg(test)]
#[path = "../tests/unit/helpers.rs"]
mod tests;
