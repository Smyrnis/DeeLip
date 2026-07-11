use egui::{Align2, RichText, Ui};

use crate::theme::{self, Palette};

/// Deterministic avatar background color for a contact/peer, hashed from its
/// name+URI across a short fixed set of Darcula-adjacent hues (the app's own
/// signal/ringing colors plus Darcula's own class-name purple and string
/// green) -- reusing real Darcula hues instead of an arbitrary rainbow keeps
/// avatar variety from feeling like an unrelated bolt-on. Shared by
/// Contacts' rows and the Messages window's conversation list.
pub(crate) fn avatar_color(seed: &str) -> egui::Color32 {
    const HUES: [egui::Color32; 4] = [
        egui::Color32::from_rgb(0x68, 0x97, 0xBB), // blue
        egui::Color32::from_rgb(0xCC, 0x78, 0x32), // orange
        egui::Color32::from_rgb(0x98, 0x76, 0xAA), // purple
        egui::Color32::from_rgb(0x6A, 0x87, 0x59), // green
    ];
    let hash: u32 = seed.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    HUES[(hash as usize) % HUES.len()]
}

/// Small circular avatar with the contact's/peer's first initial, painted
/// directly (no glyph/icon dependency, so none of this session's
/// icon-rendering incidents apply here).
pub(crate) fn avatar(ui: &mut Ui, name: &str, uri: &str) -> egui::Response {
    let size = 28.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let initial = name
        .trim()
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let color = avatar_color(if name.trim().is_empty() { uri } else { name });
    let painter = ui.painter();
    painter.circle_filled(rect.center(), size / 2.0, color);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial,
        crate::theme::font_medium(13.0),
        egui::Color32::WHITE,
    );
    response
}

/// DeeLip's app icon, decoded once per call -- used as the OS-level window
/// icon for every genuine second native window this app opens (Settings,
/// Messages), so they don't inherit the default egui/eframe placeholder icon.
pub(crate) fn window_icon() -> egui::IconData {
    const ICON_BYTES: &[u8] = include_bytes!("../../../../assets/icon.png");
    let img = image::load_from_memory(ICON_BYTES)
        .expect("assets/icon.png must be a valid image")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }
}

/// Left-hand side of the bottom status bar -- just the connection dot and
/// status text. Caller wraps this in its own `ui.horizontal()` alongside a
/// right-to-left cluster (voicemail badge / DND toggle / account label) so
/// everything shares one row, MicroSIP-style.
pub(crate) fn status_bar(ui: &mut Ui, palette: &Palette, text: &str, ok: bool, held: bool) {
    let color = if held {
        palette.ringing
    } else if ok {
        palette.signal
    } else {
        palette.ringing
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
        egui::Stroke::new(1.0, palette.border),
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
        .push_id(id_source, |ui| {
            ui.horizontal(|ui| {
                // Full tab width, not just this row's own content width --
                // otherwise the hover-highlight/divider below (and any
                // trailing `right_to_left` layout inside `add_contents`)
                // only spans/aligns against this row's narrow content.
                ui.set_width(ui.available_width());
                add_contents(ui)
            })
        })
        .inner
        .response;
    if row.hovered() {
        ui.painter().set(
            bg_idx,
            egui::Shape::rect_filled(row.rect, 0.0, palette.surface_hover),
        );
    }
    list_row_divider(ui, palette, row.rect);
}

/// Same as `list_row`, but also attaches a right-click context menu to the
/// row background -- History/Contacts use this instead of `list_row` now
/// that their per-row actions (Call/Message/Copy/Delete/etc.) live behind a
/// right-click menu (MicroSIP-style) rather than always-visible inline
/// buttons. `row.interact(Sense::click())` upgrades the row's default
/// hover-only sense so `context_menu` can detect a right click on it, per
/// egui's own documented pattern (`Response::interact`'s doc example) --
/// this doesn't steal clicks from child widgets like the name label, since
/// those sense clicks independently by their own id/rect.
///
/// `menu_contents` must not capture anything `add_contents` also mutably
/// captures -- the two closures are constructed together at the call site
/// but only one runs per frame (the menu only opens on right-click), so
/// borrowing the same `&mut` from both won't compile.
pub(crate) fn list_row_menu(
    ui: &mut Ui,
    palette: &Palette,
    id_source: impl std::hash::Hash,
    add_contents: impl FnOnce(&mut Ui),
    menu_contents: impl FnOnce(&mut Ui),
) {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let row = ui
        .push_id(id_source, |ui| {
            ui.horizontal(|ui| {
                // Same full-width reasoning as `list_row` above.
                ui.set_width(ui.available_width());
                add_contents(ui)
            })
        })
        .inner
        .response;
    if row.hovered() {
        ui.painter().set(
            bg_idx,
            egui::Shape::rect_filled(row.rect, 0.0, palette.surface_hover),
        );
    }
    list_row_divider(ui, palette, row.rect);
    row.interact(egui::Sense::click()).context_menu(menu_contents);
}

/// A row's primary name/number label, double-click-sensing so
/// History/Contacts can trigger `AppConfig::default_list_action` --
/// deliberately just this one label (not the whole row): a plain
/// `ui.label()` senses only hover by default, and upgrading a *whole row*
/// to `Sense::click()` would compete with the row's own quick-action
/// buttons for clicks (egui's hit-testing gives the *last*-added widget at
/// a position priority, and the buttons are added first) -- staying
/// scoped to a single non-overlapping label sidesteps that entirely.
/// Returns whether it was just double-clicked.
pub(crate) fn double_clickable_label(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> bool {
    ui.add(egui::Label::new(text).sense(egui::Sense::click()))
        .double_clicked()
}

/// A Settings field-row's own label (e.g. "Account name:", "Username:") --
/// muted rather than plain `palette.ink`, so it visually recedes behind the
/// actual input text next to it. Without this, both the label and a
/// `TextEdit`'s typed content fall back to the same `override_text_color`
/// (see `theme.rs::apply_style`) and render in the literal same color,
/// making them hard to tell apart at a glance.
pub(crate) fn field_label(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).color(palette.ink_muted));
}

/// A small "(i)" marker that reveals `text` as a tooltip on hover --
/// Settings' replacement for always-visible small-gray-text footnotes
/// ("Applies immediately -- no restart needed.", etc.), so each
/// section/field reads as one line with the explanation tucked away
/// instead of a wall of captions. Plain text, not
/// `egui_phosphor::regular::INFO` -- that codepoint is one of the broken
/// ones in the bundled icon font (see `theme.rs`'s module doc); it
/// silently rendered as scattered dots instead of a circled "i".
pub(crate) fn info_hint(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(
        RichText::new("(i)")
            .font(egui::FontId::new(10.5, egui::FontFamily::Monospace))
            .color(palette.ink_muted),
    )
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
        ui.label(RichText::new(title).font(theme::font_heading(13.5)));
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
    ui.label(RichText::new(text).color(palette.ink_muted).small());
}

/// Scopes `visuals.selection.bg_fill` to `palette.link` (blue) for whatever
/// `add_contents` adds -- `egui::TextEdit`'s own selected-text-range
/// highlight reads this same field the tab-bar/list "selected" chrome does
/// (`theme::apply_style` sets it to `palette.surface_hover`, grey, per the
/// v3.1 "grey chrome" decision), so this is scoped to just the text field
/// rather than changed globally -- `ui.scope` automatically restores
/// whatever the style was before once `add_contents` returns, so there's no
/// separate "reset" value to keep in sync with `apply_style`'s own.
pub(crate) fn text_edit_scope<R>(
    ui: &mut Ui,
    palette: &Palette,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.scope(|ui| {
        ui.visuals_mut().selection.bg_fill = palette.link;
        add_contents(ui)
    })
    .inner
}

/// `egui::Slider` draws its rail using `visuals.widgets.inactive.bg_fill` --
/// this theme sets that to `palette.surface` (plain white) so ordinary
/// buttons/comboboxes read as flat chrome, but that leaves every slider's
/// rail invisible against a `surface`-colored card (just a bare circle
/// handle, no track). Scoped to just the slider so it doesn't touch any
/// other widget sharing the same `ui`. Shared by the Dialer's in/out gain
/// sliders and Settings' ringtone-volume slider.
pub(crate) fn styled_slider(ui: &mut Ui, palette: &Palette, slider: egui::Slider<'_>) -> egui::Response {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.bg_fill = palette.border;
        ui.add(slider)
    })
    .inner
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

/// A registration-status dot (`palette.signal` when registered,
/// `palette.ink_muted` otherwise) followed by the plain-colored account label,
/// as one `LayoutJob` -- so account pickers read the same "online" color as
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
        palette.signal
    } else {
        palette.ink_muted
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
    // v2: a rounded-square calculator-style key, not a circle -- one of
    // the concrete "less playful" changes -- and smaller, for the denser
    // v2 layout. Bumped back up in v4 (2026-07-11) -- user feedback that
    // the keys read too small to comfortably tap. Bumped again same day --
    // "make the dialer bigger" -- shared by the idle dial pad, the in-call
    // DTMF window, and the Transfer window's keypad, so all three grow
    // together.
    const BUTTON: f32 = 64.0;
    // `ui.vertical_centered` only centers single fixed-size children -- a
    // nested `ui.horizontal` row reports its own min_rect starting flush at
    // the container's left edge, so relying on it left every row jammed
    // against the left edge instead of centered. Centering each row's
    // exact known width (3 buttons + 2 gaps) via an explicit leading
    // `add_space` is the robust way to center a *group* of widgets.
    let row_width = 3.0 * BUTTON + 2.0 * ui.spacing().item_spacing.x;
    for row in ROWS {
        ui.horizontal(|ui| {
            let margin = ((ui.available_width() - row_width) / 2.0).max(0.0);
            ui.add_space(margin);
            for digit in row {
                let button = egui::Button::new(keypad_button_text(digit, palette))
                    .rounding(egui::Rounding::same(6.0));
                if ui.add_sized([BUTTON, BUTTON], button).clicked() {
                    on_press(digit);
                }
            }
        });
    }
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
            font_id: theme::font_mono_medium(24.0),
            color: palette.ink,
            ..Default::default()
        },
    );
    let letters = digit_letters(digit);
    if !letters.is_empty() {
        job.append(
            &format!("\n{letters}"),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::new(10.0, egui::FontFamily::Proportional),
                color: palette.ink_muted,
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

pub(crate) fn ctx_key_enter(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}
