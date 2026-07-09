use deelip_config::Contact;
use deelip_sip::PresenceState;
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{
    account_status_label, double_clickable_label, empty_state, list_row_menu, save_text_file,
};

impl DeelipApp {
    pub(crate) fn show_contacts(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        ui.add_space(8.0);

        // Search bar
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut self.contact_search)
                    .desired_width(200.0)
                    .hint_text("name or sip URI"),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(egui_phosphor::regular::UPLOAD_SIMPLE)
                    .on_hover_text("Import contacts (CSV or vCard)")
                    .clicked()
                {
                    self.import_contacts();
                }
                if ui
                    .button(format!("{} vCard", egui_phosphor::regular::EXPORT))
                    .on_hover_text("Export as vCard")
                    .clicked()
                {
                    self.export_contacts_vcard();
                }
                if ui
                    .button(format!("{} CSV", egui_phosphor::regular::EXPORT))
                    .on_hover_text("Export as CSV")
                    .clicked()
                {
                    self.export_contacts_csv();
                }
            });
        });
        ui.add_space(4.0);

        let mut call_target: Option<String> = None;
        let mut message_target: Option<String> = None;
        let mut edit_idx: Option<usize> = None;
        let mut delete_idx: Option<usize> = None;
        let mut default_action_target: Option<usize> = None;

        // Contact list
        let results: Vec<(usize, String, String, bool)> = self
            .contacts
            .search(&self.contact_search)
            .into_iter()
            .map(|(i, c)| (i, c.name.clone(), c.sip_uri.clone(), c.watch_presence))
            .collect();

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                if results.is_empty() {
                    empty_state(ui, &self.palette, "No contacts found.");
                }
                for (idx, name, uri, watch_presence) in &results {
                    let palette = self.palette;
                    let presence = self.presence.get(uri).copied();
                    list_row_menu(
                        ui,
                        &palette,
                        *idx,
                        |ui| {
                            let name_text =
                                RichText::new(name).font(crate::theme::font_medium(13.0));
                            if double_clickable_label(ui, name_text) {
                                default_action_target = Some(*idx);
                            }
                            if *watch_presence {
                                let color = match presence {
                                    Some(PresenceState::Available) => palette.signal,
                                    _ => palette.ink_muted,
                                };
                                ui.label(RichText::new("●").color(color)).on_hover_text(
                                    match presence {
                                        Some(PresenceState::Available) => "Available",
                                        Some(PresenceState::Offline) => "Offline",
                                        _ => "Unknown",
                                    },
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(uri)
                                            .font(egui::FontId::new(
                                                11.5,
                                                egui::FontFamily::Monospace,
                                            ))
                                            .color(palette.ink_muted),
                                    );
                                },
                            );
                        },
                        |ui| {
                            if ui.button("Call").clicked() {
                                call_target = Some(uri.clone());
                                ui.close_menu();
                            }
                            if ui.button("Message").clicked() {
                                message_target = Some(uri.clone());
                                ui.close_menu();
                            }
                            if ui.button("Edit").clicked() {
                                edit_idx = Some(*idx);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui
                                .button(RichText::new("Delete").color(palette.danger))
                                .clicked()
                            {
                                delete_idx = Some(*idx);
                                ui.close_menu();
                            }
                        },
                    );
                }
            });

        if let Some(idx) = edit_idx {
            self.editing_contact_idx = Some(idx);
            self.new_contact = self.contacts.contacts[idx].clone();
        }
        if let Some(idx) = delete_idx {
            let removed = self.contacts.contacts.remove(idx);
            self.unsubscribe_contact_presence(&removed);
            if self.editing_contact_idx == Some(idx) {
                self.editing_contact_idx = None;
                self.new_contact = Contact::default();
            }
            let _ = self.contacts.save(&self.db);
        }

        ui.add_space(8.0);
        ui.separator();

        // Add/Edit contact form
        let heading = if self.editing_contact_idx.is_some() {
            "Edit Contact"
        } else {
            "Add Contact"
        };
        ui.label(RichText::new(heading).font(crate::theme::font_heading(14.0)));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add(egui::TextEdit::singleline(&mut self.new_contact.name).desired_width(120.0));
            ui.label("URI:");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_contact.sip_uri)
                    .hint_text("sip:alice@example.com")
                    .font(egui::FontId::new(13.0, egui::FontFamily::Monospace))
                    .desired_width(f32::INFINITY),
            );
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.new_contact.watch_presence, "Watch presence");
            if self.accounts.len() > 1 {
                ui.label("via:");
                let (current_reg_ok, current_text) = match &self.new_contact.presence_account {
                    Some(username) => self
                        .accounts
                        .iter()
                        .find(|a| &a.account.username == username)
                        .map(|a| (a.reg_ok, a.label.clone()))
                        .unwrap_or((false, username.clone())),
                    None => self
                        .accounts
                        .first()
                        .map(|a| (a.reg_ok, format!("{} (default)", a.label)))
                        .unwrap_or_default(),
                };
                let palette = self.palette;
                let selected_label =
                    account_status_label(ui, &palette, current_reg_ok, &current_text);
                egui::ComboBox::from_id_source("contact_presence_account_picker")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for acc in &self.accounts {
                            let is_sel = self.new_contact.presence_account.as_deref()
                                == Some(acc.account.username.as_str());
                            let label = account_status_label(ui, &palette, acc.reg_ok, &acc.label);
                            if ui.add(egui::SelectableLabel::new(is_sel, label)).clicked() {
                                self.new_contact.presence_account =
                                    Some(acc.account.username.clone());
                            }
                        }
                    });
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_save =
                !self.new_contact.name.is_empty() && !self.new_contact.sip_uri.is_empty();
            if ui
                .add_enabled(can_save, egui::Button::new("Save Contact"))
                .clicked()
            {
                let c = std::mem::take(&mut self.new_contact);
                if let Some(idx) = self.editing_contact_idx.take() {
                    let old = self.contacts.contacts[idx].clone();
                    self.contacts.contacts[idx] = c.clone();
                    self.unsubscribe_contact_presence(&old);
                    self.subscribe_contact_presence(&c);
                } else {
                    self.contacts.contacts.push(c.clone());
                    self.subscribe_contact_presence(&c);
                }
                let _ = self.contacts.save(&self.db);
            }
            if self.editing_contact_idx.is_some() && ui.button("Cancel").clicked() {
                self.editing_contact_idx = None;
                self.new_contact = Contact::default();
            }
        });

        if let Some(target) = call_target {
            self.dial_from_list(target);
        }
        if let Some(target) = message_target {
            self.message_from_list(target);
        }
        if let Some(idx) = default_action_target {
            match self.config.default_list_action {
                deelip_config::DefaultListAction::Call => {
                    self.dial_from_list(self.contacts.contacts[idx].sip_uri.clone());
                }
                deelip_config::DefaultListAction::Message => {
                    self.message_from_list(self.contacts.contacts[idx].sip_uri.clone());
                }
                deelip_config::DefaultListAction::Edit => {
                    self.editing_contact_idx = Some(idx);
                    self.new_contact = self.contacts.contacts[idx].clone();
                }
            }
        }
    }

    pub(crate) fn subscribe_contact_presence(&mut self, contact: &Contact) {
        if !contact.watch_presence {
            return;
        }
        if let Some(idx) = self.resolve_presence_account(contact) {
            self.accounts[idx]
                .handle
                .subscribe_presence(contact.sip_uri.clone());
        }
    }

    pub(crate) fn unsubscribe_contact_presence(&mut self, contact: &Contact) {
        if contact.watch_presence {
            if let Some(idx) = self.resolve_presence_account(contact) {
                self.accounts[idx]
                    .handle
                    .unsubscribe_presence(contact.sip_uri.clone());
            }
        }
        self.presence.remove(&contact.sip_uri);
    }

    pub(crate) fn export_contacts_csv(&self) {
        let mut csv = String::from("name,sip_uri\n");
        for c in &self.contacts.contacts {
            csv.push_str(&format!(
                "{},{}\n",
                crate::helpers::csv_escape(&c.name),
                crate::helpers::csv_escape(&c.sip_uri)
            ));
        }
        save_text_file("deelip_contacts.csv", "CSV", "csv", csv);
    }

    pub(crate) fn export_contacts_vcard(&self) {
        let mut vcf = String::new();
        for c in &self.contacts.contacts {
            vcf.push_str("BEGIN:VCARD\r\n");
            vcf.push_str("VERSION:3.0\r\n");
            vcf.push_str(&format!("FN:{}\r\n", c.name));
            vcf.push_str(&format!("IMPP:{}\r\n", c.sip_uri));
            vcf.push_str("END:VCARD\r\n");
        }
        save_text_file("deelip_contacts.vcf", "vCard", "vcf", vcf);
    }

    /// Import contacts from a CSV or vCard file (detected by extension,
    /// falling back to content sniffing). Appended to the existing contact
    /// list with no dedup, matching the manual Add-contact flow's behavior.
    pub(crate) fn import_contacts(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Contacts", &["csv", "vcf"])
            .pick_file()
        else {
            return;
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to read {}: {e}", path.display());
                return;
            }
        };

        let is_vcard = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("vcf"))
            || content.contains("BEGIN:VCARD");

        let imported = if is_vcard {
            parse_vcard(&content)
        } else {
            parse_contacts_csv(&content)
        };

        if imported.is_empty() {
            tracing::warn!("No contacts found in {}", path.display());
            return;
        }

        self.contacts.contacts.extend(imported);
        let _ = self.contacts.save(&self.db);
    }
}

/// Parse a CSV contact file with a `name,sip_uri` header, using
/// `parse_csv_line` for each data row.
fn parse_contacts_csv(content: &str) -> Vec<Contact> {
    content
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields = parse_csv_line(line);
            let name = fields.first()?.clone();
            let sip_uri = fields.get(1)?.clone();
            Some(Contact {
                name,
                sip_uri,
                ..Default::default()
            })
        })
        .collect()
}

/// Split one CSV line into fields, honoring double-quoted fields and
/// doubled-quote escaping -- the inverse of `csv_escape`.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut field));
            }
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// Minimal vCard 2.1/3.0 parser: pulls `FN` for the name and the first
/// `TEL`/`IMPP` line (any `;PARAM=...` suffix on the property name is
/// ignored) for the URI, from each `BEGIN:VCARD`/`END:VCARD` block.
fn parse_vcard(content: &str) -> Vec<Contact> {
    let mut contacts = Vec::new();
    let mut name: Option<String> = None;
    let mut uri: Option<String> = None;

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.eq_ignore_ascii_case("BEGIN:VCARD") {
            name = None;
            uri = None;
            continue;
        }
        if line.eq_ignore_ascii_case("END:VCARD") {
            if let (Some(n), Some(u)) = (name.take(), uri.take()) {
                contacts.push(Contact {
                    name: n,
                    sip_uri: u,
                    ..Default::default()
                });
            }
            continue;
        }
        let Some((prop, value)) = line.split_once(':') else {
            continue;
        };
        let prop_name = prop.split(';').next().unwrap_or(prop);
        if name.is_none() && prop_name.eq_ignore_ascii_case("FN") {
            name = Some(value.to_string());
        } else if uri.is_none()
            && (prop_name.eq_ignore_ascii_case("TEL") || prop_name.eq_ignore_ascii_case("IMPP"))
        {
            uri = Some(value.to_string());
        }
    }
    contacts
}
