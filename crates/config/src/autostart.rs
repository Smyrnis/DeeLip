use std::path::PathBuf;

use anyhow::Context;

/// `~/.config/autostart/deelip.desktop` — the standard freedesktop.org XDG
/// autostart path, honored by GNOME/KDE/XFCE alike without needing a
/// systemd unit.
fn autostart_desktop_path() -> anyhow::Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine user config directory"))?;
    Ok(base.join("autostart").join("deelip.desktop"))
}

pub fn is_autostart_enabled() -> bool {
    autostart_desktop_path().is_ok_and(|p| p.exists())
}

/// Write or remove the XDG autostart `.desktop` file. Takes effect on next
/// login; has no effect on the currently running process.
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let path = autostart_desktop_path()?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("Removing {}", path.display()))?;
        }
        return Ok(());
    }

    let exe = std::env::current_exe().context("Resolving current executable path")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("Creating {}", parent.display()))?;
    }
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=DeeLip\nExec={}\nX-GNOME-Autostart-enabled=true\n",
        exe.display(),
    );
    std::fs::write(&path, contents).with_context(|| format!("Writing {}", path.display()))
}
