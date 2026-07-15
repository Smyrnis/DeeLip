//! "Directory of Users" -- an optional corporate/LDAP directory search,
//! distinct from local Contacts. Read-only: this never writes back to the
//! directory, only searches it and (optionally) copies a result into local
//! Contacts. See `AppConfig::ldap_server` et al. for the connection
//! settings.

use anyhow::Context as _;
use deelip_config::timeouts::{
    DIRECTORY_CONNECT_TIMEOUT as CONNECT_TIMEOUT, DIRECTORY_SEARCH_TIMEOUT as SEARCH_TIMEOUT,
};
use egui::{RichText, Ui};

use crate::app::DeelipApp;
use crate::helpers::{empty_state, list_row, search_field};
use crate::strings::{t, tf};

/// One directory search result -- just enough to call/message/save it.
pub(crate) struct DirectoryEntry {
    pub name: String,
    pub number: String,
}

/// Directory search UI state, driven by `process_directory_events`.
#[derive(Default)]
pub(crate) enum DirectoryState {
    #[default]
    Idle,
    Searching,
    Results(Vec<DirectoryEntry>),
    Failed(String),
}

pub(crate) enum DirectoryMsg {
    Done(anyhow::Result<Vec<DirectoryEntry>>),
}

/// Attribute names tried, in priority order, for a result's display name /
/// phone number -- covers both common `inetOrgPerson` (OpenLDAP) and Active
/// Directory-style schemas without needing a configurable attribute map.
const NAME_ATTRS: &[&str] = &["displayName", "cn", "name"];
const NUMBER_ATTRS: &[&str] = &["telephoneNumber", "mobile", "mobileTelephoneNumber", "otherTelephone", "homePhone"];

const DEFAULT_FILTER_TEMPLATE: &str =
    "(|(cn=*{query}*)(displayName=*{query}*)(mail=*{query}*)(sn=*{query}*)(givenName=*{query}*))";

/// Escape a search term for safe embedding in an RFC 4515 filter --
/// `\`/`*`/`(`/`)`/NUL are the only characters that can break out of a
/// filter's grammar, so those are all that need escaping here (the fuller
/// spec also allows escaping arbitrary UTF-8 octets, which isn't necessary
/// for correctness).
pub(crate) fn escape_ldap_filter(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\5c"),
            '*' => out.push_str("\\2a"),
            '(' => out.push_str("\\28"),
            ')' => out.push_str("\\29"),
            '\0' => out.push_str("\\00"),
            _ => out.push(c),
        }
    }
    out
}

struct LdapSearchConfig {
    server: String,
    port: u16,
    use_tls: bool,
    base_dn: String,
    bind_dn: Option<String>,
    bind_password: Option<String>,
    filter_template: Option<String>,
}

async fn run_ldap_search(cfg: LdapSearchConfig, query: String) -> anyhow::Result<Vec<DirectoryEntry>> {
    let scheme = if cfg.use_tls { "ldaps" } else { "ldap" };
    let url = format!("{scheme}://{}:{}", cfg.server, cfg.port);

    let (conn, mut ldap) = tokio::time::timeout(CONNECT_TIMEOUT, ldap3::LdapConnAsync::new(&url))
        .await
        .context("Connecting to LDAP server timed out")?
        .context("Connecting to LDAP server")?;
    ldap3::drive!(conn);

    match (&cfg.bind_dn, &cfg.bind_password) {
        (Some(dn), Some(pw)) if !dn.trim().is_empty() => {
            ldap.simple_bind(dn, pw).await.context("LDAP bind")?.success().context("LDAP bind rejected")?;
        }
        _ => {
            // Anonymous bind -- many directories reject search without one,
            // but plenty of read-only setups allow it.
            ldap.simple_bind("", "")
                .await
                .context("LDAP anonymous bind")?
                .success()
                .context("LDAP anonymous bind rejected")?;
        }
    }

    let filter_template =
        cfg.filter_template.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(DEFAULT_FILTER_TEMPLATE);
    let filter = filter_template.replace("{query}", &escape_ldap_filter(&query));

    let mut attrs: Vec<&str> = NAME_ATTRS.to_vec();
    attrs.extend(NUMBER_ATTRS);

    let (results, _res) =
        tokio::time::timeout(SEARCH_TIMEOUT, ldap.search(&cfg.base_dn, ldap3::Scope::Subtree, &filter, attrs))
            .await
            .context("LDAP search timed out")?
            .context("LDAP search")?
            .success()
            .context("LDAP search rejected")?;

    let mut entries = Vec::new();
    for entry in results {
        let se = ldap3::SearchEntry::construct(entry);
        let name = NAME_ATTRS.iter().find_map(|k| se.attrs.get(*k).and_then(|v| v.first())).cloned();
        let number = NUMBER_ATTRS.iter().find_map(|k| se.attrs.get(*k).and_then(|v| v.first())).cloned();
        // Skip entries with no phone number at all -- nothing this softphone
        // could do with them.
        if let (Some(name), Some(number)) = (name, number) {
            entries.push(DirectoryEntry { name, number });
        }
    }
    let _ = ldap.unbind().await;
    Ok(entries)
}

impl DeelipApp {
    /// Kicks off a background directory search -- no-op if no LDAP server
    /// is configured or the search box is empty. Same background-thread +
    /// one-shot-channel idiom as `update.rs`'s update check, since `ldap3`'s
    /// calls are `async` rather than the updater's plain blocking HTTP:
    /// `self.rt` (a `tokio::runtime::Handle`) drives them via `block_on`
    /// from that background thread instead of `rt.spawn`, so nothing here
    /// ever runs on (or blocks) the UI thread.
    pub(crate) fn start_directory_search(&mut self) {
        let query = self.directory_ui.directory_query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(server) = self.config.ldap_server.clone().filter(|s| !s.trim().is_empty()) else {
            return;
        };
        let cfg = LdapSearchConfig {
            server,
            port: self.config.ldap_port,
            use_tls: self.config.ldap_use_tls,
            base_dn: self.config.ldap_base_dn.clone().unwrap_or_default(),
            bind_dn: self.config.ldap_bind_dn.clone(),
            bind_password: self.config.ldap_bind_password.clone(),
            filter_template: self.config.ldap_search_filter.clone(),
        };
        let rt = self.rt.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.directory_ui.directory_rx = Some(rx);
        self.directory_ui.directory_state = DirectoryState::Searching;
        let ctx_slot = self.ctx_slot.clone();
        std::thread::spawn(move || {
            let result = rt.block_on(run_ldap_search(cfg, query));
            let _ = tx.send(DirectoryMsg::Done(result));
            if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }

    /// Drains the directory-search channel, called once per frame.
    pub(crate) fn process_directory_events(&mut self) {
        let Some(rx) = &self.directory_ui.directory_rx else {
            return;
        };
        let messages: Vec<DirectoryMsg> = rx.try_iter().collect();
        for msg in messages {
            match msg {
                DirectoryMsg::Done(Ok(entries)) => self.directory_ui.directory_state = DirectoryState::Results(entries),
                DirectoryMsg::Done(Err(e)) => {
                    self.directory_ui.directory_state = DirectoryState::Failed(format!("{e:#}"))
                }
            }
        }
    }

    pub(crate) fn show_directory(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        ui.add_space(8.0);
        if self.config.ldap_server.as_deref().filter(|s| !s.trim().is_empty()).is_none() {
            empty_state(ui, &self.palette, &t("directory.configure_ldap"));
            return;
        }

        let palette = self.palette;
        ui.horizontal(|ui| {
            let resp =
                search_field(ui, &palette, &mut self.directory_ui.directory_query, &t("directory.search_hint"), 200.0);
            let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let search_clicked = ui.button(t("directory.search_button")).clicked();
            if search_clicked || enter_pressed {
                self.start_directory_search();
            }
        });
        ui.add_space(4.0);

        let mut call_target: Option<String> = None;
        let mut message_target: Option<String> = None;
        let mut add_contact: Option<(String, String)> = None;

        match &self.directory_ui.directory_state {
            DirectoryState::Idle => {
                empty_state(ui, &self.palette, &t("directory.search_above"));
            }
            DirectoryState::Searching => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t("directory.searching"));
                });
            }
            DirectoryState::Failed(reason) => {
                ui.colored_label(self.palette.danger, tf("directory.search_failed", &[("reason", reason)]));
            }
            DirectoryState::Results(entries) => {
                if entries.is_empty() {
                    empty_state(ui, &self.palette, &t("directory.no_matches"));
                }
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for (idx, entry) in entries.iter().enumerate() {
                        let palette = self.palette;
                        list_row(ui, &palette, idx, |ui| {
                            ui.label(RichText::new(&entry.name).font(crate::theme::font_medium(13.0)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(egui_phosphor::regular::PHONE).clicked() {
                                    call_target = Some(entry.number.clone());
                                }
                                if ui.small_button(egui_phosphor::regular::CHAT_CIRCLE).clicked() {
                                    message_target = Some(entry.number.clone());
                                }
                                if ui
                                    .small_button(egui_phosphor::regular::USER_PLUS)
                                    .on_hover_text(t("directory.add_to_contacts"))
                                    .clicked()
                                {
                                    add_contact = Some((entry.name.clone(), entry.number.clone()));
                                }
                                ui.label(
                                    RichText::new(&entry.number)
                                        .font(egui::FontId::new(11.5, egui::FontFamily::Monospace))
                                        .color(palette.ink_muted),
                                );
                            });
                        });
                    }
                });
            }
        }

        if let Some(target) = call_target {
            self.dial_from_list(target);
        }
        if let Some(target) = message_target {
            self.message_from_list(target);
        }
        if let Some((name, sip_uri)) = add_contact {
            self.contacts.contacts.push(deelip_config::Contact { name, sip_uri, ..Default::default() });
            let _ = self.contacts.save(&self.db);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/directory.rs"]
mod tests;
