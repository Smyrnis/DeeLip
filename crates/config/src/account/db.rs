//! `AppConfig::load`/`save` -- the SQL/rusqlite row marshaling between the
//! typed config structs (`app_config.rs`/`sip_account.rs`) and the
//! `accounts` table + key-value `settings` table. Pure persistence glue, no
//! domain logic of its own.

use anyhow::Context;

use super::app_config::{
    default_hotkey_answer, default_hotkey_hangup, default_hotkey_mute, default_ldap_port, AppConfig,
};
use super::enums::{
    default_list_action_from_str, default_list_action_to_str, dtmf_mode_from_str, dtmf_mode_to_str, language_from_str,
    language_to_str, media_encryption_from_str, media_encryption_to_str, recording_format_from_str,
    recording_format_to_str, transport_from_str, transport_to_str, update_check_frequency_from_str,
    update_check_frequency_to_str,
};
use super::sip_account::{
    default_codec_order, default_frame_ms, default_ringtone_volume, default_sample_rate, default_sip_port, AudioConfig,
    SipAccount,
};
use crate::db::{bool_to_sql, sql_int_to_bool, sql_to_bool};
use crate::Db;

impl AppConfig {
    pub fn load(db: &Db) -> anyhow::Result<Self> {
        let get = |key: &str| db.get_setting(key);
        let get_bool = |key: &str, default: bool| get(key).map(|v| sql_to_bool(&v)).unwrap_or(default);
        let get_u32 = |key: &str, default: u32| get(key).and_then(|v| v.parse().ok()).unwrap_or(default);
        let get_f32 = |key: &str, default: f32| get(key).and_then(|v| v.parse().ok()).unwrap_or(default);

        let mut stmt = db.conn.prepare(
            "SELECT username, password, server, port, display_name, transport, enabled, \
                    tls_insecure_skip_verify, no_answer_forward, no_answer_timeout_secs, dnd, \
                    forward_always, forward_on_busy, codec_order, dtmf_mode, auto_answer_enabled, \
                    auto_answer_secs, mailbox, account_name, sip_proxy, domain, auth_username, \
                    dialing_prefix, hide_caller_id, register_expires, keepalive_secs, \
                    media_encryption, public_address, ice_enabled, force_incoming_codec, \
                    vad_enabled, publish_presence, allow_ip_rewrite, dial_plan, \
                    session_timers_enabled, auto_answer_control_button, \
                    deny_incoming_control_button, local_account, video_enabled \
             FROM accounts ORDER BY sort_order",
        )?;
        let accounts = stmt
            .query_map([], |row| {
                let codec_order_json: String = row.get("codec_order")?;
                let transport_str: String = row.get("transport")?;
                let dtmf_mode_str: String = row.get("dtmf_mode")?;
                let media_encryption_str: String = row.get("media_encryption")?;
                let ice_enabled: Option<i64> = row.get("ice_enabled")?;
                let dial_plan_json: String = row.get("dial_plan")?;
                Ok(SipAccount {
                    username: row.get("username")?,
                    password: row.get("password")?,
                    server: row.get("server")?,
                    port: row.get("port")?,
                    display_name: row.get("display_name")?,
                    transport: transport_from_str(&transport_str),
                    enabled: sql_int_to_bool(row.get("enabled")?),
                    tls_insecure_skip_verify: sql_int_to_bool(row.get("tls_insecure_skip_verify")?),
                    no_answer_forward: row.get("no_answer_forward")?,
                    no_answer_timeout_secs: row.get("no_answer_timeout_secs")?,
                    dnd: sql_int_to_bool(row.get("dnd")?),
                    forward_always: row.get("forward_always")?,
                    forward_on_busy: row.get("forward_on_busy")?,
                    codec_order: serde_json::from_str(&codec_order_json).unwrap_or_else(|_| default_codec_order()),
                    dtmf_mode: dtmf_mode_from_str(&dtmf_mode_str),
                    auto_answer_enabled: sql_int_to_bool(row.get("auto_answer_enabled")?),
                    auto_answer_secs: row.get("auto_answer_secs")?,
                    mailbox: row.get("mailbox")?,
                    account_name: row.get("account_name")?,
                    sip_proxy: row.get("sip_proxy")?,
                    domain: row.get("domain")?,
                    auth_username: row.get("auth_username")?,
                    dialing_prefix: row.get("dialing_prefix")?,
                    hide_caller_id: sql_int_to_bool(row.get("hide_caller_id")?),
                    register_expires: row.get("register_expires")?,
                    keepalive_secs: row.get("keepalive_secs")?,
                    media_encryption: media_encryption_from_str(&media_encryption_str),
                    public_address: row.get("public_address")?,
                    ice_enabled: ice_enabled.map(sql_int_to_bool),
                    force_incoming_codec: row.get("force_incoming_codec")?,
                    vad_enabled: sql_int_to_bool(row.get("vad_enabled")?),
                    publish_presence: sql_int_to_bool(row.get("publish_presence")?),
                    allow_ip_rewrite: sql_int_to_bool(row.get("allow_ip_rewrite")?),
                    dial_plan: serde_json::from_str(&dial_plan_json).unwrap_or_default(),
                    session_timers_enabled: sql_int_to_bool(row.get("session_timers_enabled")?),
                    auto_answer_control_button: sql_int_to_bool(row.get("auto_answer_control_button")?),
                    deny_incoming_control_button: sql_int_to_bool(row.get("deny_incoming_control_button")?),
                    local_account: sql_int_to_bool(row.get("local_account")?),
                    video_enabled: sql_int_to_bool(row.get("video_enabled")?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Reading accounts from database")?;

        Ok(AppConfig {
            accounts,
            audio: AudioConfig {
                input_device: get("audio.input_device"),
                output_device: get("audio.output_device"),
                sample_rate: get_u32("audio.sample_rate", default_sample_rate()),
                frame_size_ms: get_u32("audio.frame_size_ms", default_frame_ms()),
                echo_cancellation: get_bool("audio.echo_cancellation", false),
                ringtone_device: get("audio.ringtone_device"),
                ringtone_file: get("audio.ringtone_file"),
                ringtone_volume: get_f32("audio.ringtone_volume", default_ringtone_volume()),
                agc_enabled: get_bool("audio.agc_enabled", false),
                camera_device: get("audio.camera_device"),
            },
            local_sip_port: get_u32("local_sip_port", default_sip_port() as u32) as u16,
            stun_server: get("stun_server"),
            turn_server: get("turn_server"),
            turn_username: get("turn_username"),
            turn_password: get("turn_password"),
            rtp_port_min: get("rtp_port_min").and_then(|v| v.parse().ok()),
            rtp_port_max: get("rtp_port_max").and_then(|v| v.parse().ok()),
            custom_nameserver: get("custom_nameserver"),
            dns_srv_enabled: get_bool("dns_srv_enabled", false),
            single_call_mode: get_bool("single_call_mode", false),
            dark_mode: get_bool("dark_mode", true),
            notifications_enabled: get_bool("notifications_enabled", true),
            ringtone_enabled: get_bool("ringtone_enabled", true),
            recording_enabled: get_bool("recording_enabled", false),
            recording_format: get("recording_format").as_deref().map(recording_format_from_str).unwrap_or_default(),
            recordings_dir_override: get("recordings_dir_override"),
            start_minimized: get_bool("start_minimized", false),
            log_to_file: get_bool("log_to_file", false),
            crash_reporting_enabled: get_bool("crash_reporting_enabled", true),
            blocklist: get("blocklist").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default(),
            ice_enabled: get_bool("ice_enabled", false),
            global_hotkeys_enabled: get_bool("global_hotkeys_enabled", false),
            hotkey_answer: get("hotkey_answer").unwrap_or_else(default_hotkey_answer),
            hotkey_hangup: get("hotkey_hangup").unwrap_or_else(default_hotkey_hangup),
            hotkey_mute: get("hotkey_mute").unwrap_or_else(default_hotkey_mute),
            handle_media_buttons: get_bool("handle_media_buttons", false),
            auto_update_enabled: get_bool("auto_update_enabled", false),
            update_skip_version: get("update_skip_version"),
            update_check_frequency: get("update_check_frequency")
                .as_deref()
                .map(update_check_frequency_from_str)
                .unwrap_or_default(),
            last_update_check: get("last_update_check").and_then(|v| v.parse().ok()),
            default_list_action: get("default_list_action")
                .as_deref()
                .map(default_list_action_from_str)
                .unwrap_or_default(),
            language: get("language").as_deref().map(language_from_str).unwrap_or_default(),
            random_popup_position: get_bool("random_popup_position", false),
            zrtp_zid: get("zrtp_zid"),
            ldap_server: get("ldap_server"),
            ldap_port: get_u32("ldap_port", default_ldap_port() as u32) as u16,
            ldap_use_tls: get_bool("ldap_use_tls", false),
            ldap_base_dn: get("ldap_base_dn"),
            ldap_bind_dn: get("ldap_bind_dn"),
            ldap_bind_password: get("ldap_bind_password"),
            ldap_search_filter: get("ldap_search_filter"),
        })
    }

    pub fn save(&self, db: &Db) -> anyhow::Result<()> {
        db.conn.execute("DELETE FROM accounts", []).context("Clearing accounts table")?;
        for (i, acc) in self.accounts.iter().enumerate() {
            db.conn
                .execute(
                    "INSERT INTO accounts (sort_order, username, password, server, port, display_name, \
                    transport, enabled, tls_insecure_skip_verify, no_answer_forward, \
                    no_answer_timeout_secs, dnd, forward_always, forward_on_busy, codec_order, \
                    dtmf_mode, auto_answer_enabled, auto_answer_secs, mailbox, account_name, \
                    sip_proxy, domain, auth_username, dialing_prefix, hide_caller_id, \
                    register_expires, keepalive_secs, media_encryption, public_address, \
                    ice_enabled, force_incoming_codec, vad_enabled, publish_presence, \
                    allow_ip_rewrite, dial_plan, session_timers_enabled, \
                    auto_answer_control_button, deny_incoming_control_button, local_account, \
                    video_enabled) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
                    ?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40)",
                    rusqlite::params![
                        i as i64,
                        acc.username,
                        acc.password,
                        acc.server,
                        acc.port,
                        acc.display_name,
                        transport_to_str(&acc.transport),
                        bool_to_sql(acc.enabled),
                        bool_to_sql(acc.tls_insecure_skip_verify),
                        acc.no_answer_forward,
                        acc.no_answer_timeout_secs,
                        bool_to_sql(acc.dnd),
                        acc.forward_always,
                        acc.forward_on_busy,
                        serde_json::to_string(&acc.codec_order)?,
                        dtmf_mode_to_str(acc.dtmf_mode),
                        bool_to_sql(acc.auto_answer_enabled),
                        acc.auto_answer_secs,
                        acc.mailbox,
                        acc.account_name,
                        acc.sip_proxy,
                        acc.domain,
                        acc.auth_username,
                        acc.dialing_prefix,
                        bool_to_sql(acc.hide_caller_id),
                        acc.register_expires,
                        acc.keepalive_secs,
                        media_encryption_to_str(acc.media_encryption),
                        acc.public_address,
                        acc.ice_enabled.map(bool_to_sql),
                        acc.force_incoming_codec,
                        bool_to_sql(acc.vad_enabled),
                        bool_to_sql(acc.publish_presence),
                        bool_to_sql(acc.allow_ip_rewrite),
                        serde_json::to_string(&acc.dial_plan)?,
                        bool_to_sql(acc.session_timers_enabled),
                        bool_to_sql(acc.auto_answer_control_button),
                        bool_to_sql(acc.deny_incoming_control_button),
                        bool_to_sql(acc.local_account),
                        bool_to_sql(acc.video_enabled),
                    ],
                )
                .with_context(|| format!("Inserting account {}", acc.username))?;
        }

        db.set_setting_opt("audio.input_device", &self.audio.input_device)?;
        db.set_setting_opt("audio.output_device", &self.audio.output_device)?;
        db.set_setting("audio.sample_rate", &self.audio.sample_rate.to_string())?;
        db.set_setting("audio.frame_size_ms", &self.audio.frame_size_ms.to_string())?;
        db.set_setting("audio.echo_cancellation", bool_to_sql(self.audio.echo_cancellation))?;
        db.set_setting_opt("audio.ringtone_device", &self.audio.ringtone_device)?;
        db.set_setting_opt("audio.ringtone_file", &self.audio.ringtone_file)?;
        db.set_setting("audio.ringtone_volume", &self.audio.ringtone_volume.to_string())?;
        db.set_setting("audio.agc_enabled", bool_to_sql(self.audio.agc_enabled))?;
        db.set_setting_opt("audio.camera_device", &self.audio.camera_device)?;

        db.set_setting("local_sip_port", &self.local_sip_port.to_string())?;
        db.set_setting_opt("stun_server", &self.stun_server)?;
        db.set_setting_opt("turn_server", &self.turn_server)?;
        db.set_setting_opt("turn_username", &self.turn_username)?;
        db.set_setting_opt("turn_password", &self.turn_password)?;
        db.set_setting_opt("rtp_port_min", &self.rtp_port_min.map(|v| v.to_string()))?;
        db.set_setting_opt("rtp_port_max", &self.rtp_port_max.map(|v| v.to_string()))?;
        db.set_setting_opt("custom_nameserver", &self.custom_nameserver)?;
        db.set_setting("dns_srv_enabled", bool_to_sql(self.dns_srv_enabled))?;
        db.set_setting("single_call_mode", bool_to_sql(self.single_call_mode))?;
        db.set_setting("dark_mode", bool_to_sql(self.dark_mode))?;
        db.set_setting("notifications_enabled", bool_to_sql(self.notifications_enabled))?;
        db.set_setting("ringtone_enabled", bool_to_sql(self.ringtone_enabled))?;
        db.set_setting("recording_enabled", bool_to_sql(self.recording_enabled))?;
        db.set_setting("recording_format", recording_format_to_str(self.recording_format))?;
        db.set_setting_opt("recordings_dir_override", &self.recordings_dir_override)?;
        db.set_setting("start_minimized", bool_to_sql(self.start_minimized))?;
        db.set_setting("log_to_file", bool_to_sql(self.log_to_file))?;
        db.set_setting("crash_reporting_enabled", bool_to_sql(self.crash_reporting_enabled))?;
        db.set_setting("blocklist", &serde_json::to_string(&self.blocklist)?)?;
        db.set_setting("ice_enabled", bool_to_sql(self.ice_enabled))?;
        db.set_setting("global_hotkeys_enabled", bool_to_sql(self.global_hotkeys_enabled))?;
        db.set_setting("hotkey_answer", &self.hotkey_answer)?;
        db.set_setting("hotkey_hangup", &self.hotkey_hangup)?;
        db.set_setting("hotkey_mute", &self.hotkey_mute)?;
        db.set_setting("handle_media_buttons", bool_to_sql(self.handle_media_buttons))?;
        db.set_setting("auto_update_enabled", bool_to_sql(self.auto_update_enabled))?;
        db.set_setting_opt("update_skip_version", &self.update_skip_version)?;
        db.set_setting("update_check_frequency", update_check_frequency_to_str(self.update_check_frequency))?;
        db.set_setting_opt("last_update_check", &self.last_update_check.map(|v| v.to_string()))?;
        db.set_setting("default_list_action", default_list_action_to_str(self.default_list_action))?;
        db.set_setting("language", language_to_str(self.language))?;
        db.set_setting("random_popup_position", bool_to_sql(self.random_popup_position))?;
        db.set_setting_opt("zrtp_zid", &self.zrtp_zid)?;
        db.set_setting_opt("ldap_server", &self.ldap_server)?;
        db.set_setting("ldap_port", &self.ldap_port.to_string())?;
        db.set_setting("ldap_use_tls", bool_to_sql(self.ldap_use_tls))?;
        db.set_setting_opt("ldap_base_dn", &self.ldap_base_dn)?;
        db.set_setting_opt("ldap_bind_dn", &self.ldap_bind_dn)?;
        db.set_setting_opt("ldap_bind_password", &self.ldap_bind_password)?;
        db.set_setting_opt("ldap_search_filter", &self.ldap_search_filter)?;
        Ok(())
    }
}
