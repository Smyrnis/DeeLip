//! "Start on login" toggle, one native mechanism per OS -- see each
//! platform module below for the exact mechanism. All three expose the same
//! `is_autostart_enabled`/`set_autostart` pair at the bottom of this file,
//! so callers (Settings' checkbox) never need to know which OS they're on.

use std::path::PathBuf;

use anyhow::Context;

/// Linux: a freedesktop.org XDG autostart `.desktop` file, honored by
/// GNOME/KDE/XFCE alike without needing a systemd unit.
#[cfg(target_os = "linux")]
mod os {
    use super::*;

    fn autostart_desktop_path() -> anyhow::Result<PathBuf> {
        let base = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine user config directory"))?;
        Ok(base.join("autostart").join("deelip.desktop"))
    }

    pub fn is_enabled() -> bool {
        autostart_desktop_path().is_ok_and(|p| p.exists())
    }

    /// Takes effect on next login; has no effect on the currently running
    /// process.
    pub fn set(enabled: bool) -> anyhow::Result<()> {
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
}

/// Windows: a string value named "DeeLip" under the per-user
/// `Run` key -- the standard un-elevated autostart mechanism (no installer/
/// service required, matches the Linux XDG approach's "just the current
/// user, no admin rights needed" scope).
#[cfg(target_os = "windows")]
mod os {
    use anyhow::Context;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "DeeLip";

    pub fn is_enabled() -> bool {
        let Ok(run_key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY_PATH) else { return false };
        run_key.get_value::<String, _>(VALUE_NAME).is_ok()
    }

    /// Takes effect on next login; has no effect on the currently running
    /// process.
    pub fn set(enabled: bool) -> anyhow::Result<()> {
        // `create_subkey` (not `open_subkey`, which is read-only) since the
        // `Run` key itself always exists on a real Windows install, but this
        // matches winreg's own documented way to get write access to it.
        let (run_key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY_PATH)
            .context("Opening HKCU...\\Run registry key")?;
        if !enabled {
            return match run_key.delete_value(VALUE_NAME) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e).context("Removing autostart registry value"),
            };
        }

        let exe = std::env::current_exe().context("Resolving current executable path")?;
        // Quoted so a path containing spaces still parses as one command
        // when the shell that reads this value splits it into arguments.
        run_key.set_value(VALUE_NAME, &format!("\"{}\"", exe.display())).context("Writing autostart registry value")
    }
}

/// macOS: a per-user `LaunchAgent` plist, the standard un-elevated
/// autostart mechanism (no installer/system daemon required).
#[cfg(target_os = "macos")]
mod os {
    use super::*;

    fn launch_agent_path() -> anyhow::Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        Ok(home.join("Library/LaunchAgents/com.deelip.app.plist"))
    }

    pub fn is_enabled() -> bool {
        launch_agent_path().is_ok_and(|p| p.exists())
    }

    /// Takes effect on next login; has no effect on the currently running
    /// process.
    pub fn set(enabled: bool) -> anyhow::Result<()> {
        let path = launch_agent_path()?;
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
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.deelip.app</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
            exe.display(),
        );
        std::fs::write(&path, contents).with_context(|| format!("Writing {}", path.display()))
    }
}

/// Anything else (BSD, etc.) -- degrades gracefully rather than failing to
/// compile, matching this codebase's existing "app works fine without
/// platform integration X" convention (see the tray icon/global hotkeys).
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod os {
    pub fn is_enabled() -> bool {
        false
    }

    pub fn set(_enabled: bool) -> anyhow::Result<()> {
        anyhow::bail!("Autostart is not supported on this platform")
    }
}

pub fn is_autostart_enabled() -> bool {
    os::is_enabled()
}

pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    os::set(enabled)
}
