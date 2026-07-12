use std::path::PathBuf;

use anyhow::Context;

mod account;
mod autostart;
mod contacts;
mod db;
mod dialplan;
mod history;
mod messages;

pub use account::{
    AppConfig, AudioConfig, DefaultListAction, DtmfMode, Language, MediaEncryption, RecordingFormat, SipAccount,
    TransportProtocol, UpdateCheckFrequency,
};
pub use autostart::{is_autostart_enabled, set_autostart};
pub use contacts::{Contact, ContactBook};
pub use db::{Db, default_db_path};
pub use dialplan::{DialPlanRule, apply_dial_plan};
pub use history::{CallDirection, CallHistory, CallRecord, CallStatus};
pub use messages::{Message, MessageDirection, MessageLog};

fn deelip_dir() -> anyhow::Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine user config directory"))?;
    Ok(base.join("deelip"))
}

/// Returns `~/.config/deelip/deelip.log`, creating the parent directory if
/// it doesn't exist yet -- used by `AppConfig::log_to_file`.
pub fn log_file_path() -> anyhow::Result<PathBuf> {
    let dir = deelip_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("Creating config dir {}", dir.display()))?;
    Ok(dir.join("deelip.log"))
}

/// Returns the recordings directory, creating it if it doesn't exist yet --
/// `override_dir` if set and non-empty (`AppConfig::recordings_dir_override`),
/// otherwise the default `~/.config/deelip/recordings`.
pub fn recordings_dir(override_dir: Option<&str>) -> anyhow::Result<PathBuf> {
    let dir = match override_dir.map(str::trim).filter(|s| !s.is_empty()) {
        Some(custom) => PathBuf::from(custom),
        None => deelip_dir()?.join("recordings"),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("Creating recordings dir {}", dir.display()))?;
    Ok(dir)
}

/// Returns `~/.config/deelip/crashes`, creating it if it doesn't exist yet --
/// used by `src/main.rs`'s panic hook (`AppConfig::crash_reporting_enabled`)
/// to save local crash-report files. Never uploaded/transmitted anywhere.
pub fn crashes_dir() -> anyhow::Result<PathBuf> {
    let dir = deelip_dir()?.join("crashes");
    std::fs::create_dir_all(&dir).with_context(|| format!("Creating crashes dir {}", dir.display()))?;
    Ok(dir)
}
