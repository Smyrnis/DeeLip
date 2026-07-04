use std::path::PathBuf;

use anyhow::Context;

mod account;
mod autostart;
mod contacts;
mod db;
mod history;

pub use account::{AppConfig, AudioConfig, DtmfMode, SipAccount, TransportProtocol};
pub use autostart::{is_autostart_enabled, set_autostart};
pub use contacts::{Contact, ContactBook};
pub use db::{default_db_path, Db};
pub use history::{CallDirection, CallHistory, CallRecord, CallStatus};

fn deelip_dir() -> anyhow::Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine user config directory"))?;
    Ok(base.join("deelip"))
}

/// Returns `~/.config/deelip/recordings`, creating it if it doesn't exist yet.
pub fn recordings_dir() -> anyhow::Result<PathBuf> {
    let dir = deelip_dir()?.join("recordings");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Creating recordings dir {}", dir.display()))?;
    Ok(dir)
}
