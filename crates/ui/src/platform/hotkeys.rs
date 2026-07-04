//! Global (system-wide) hotkeys for Answer/Hangup/Mute.
//!
//! Linux support in the underlying `global-hotkey` crate is X11-only --
//! fine here since `main.rs` already forces DeeLip's main window onto
//! X11/XWayland for unrelated tray-restore reasons (native Wayland gives
//! clients no way to reliably restore their own window). Unlike the tray
//! icon, this crate's Linux backend spawns and owns its own dedicated X11
//! connection/event-loop thread internally (see its `platform_impl/x11`
//! module) -- no GTK-style setup-ordering constraint to worry about here,
//! it can be created at any point once an X server is reachable.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Answer,
    Hangup,
    Mute,
}

/// Owns the manager -- dropping it unregisters every binding -- plus the
/// id -> action mapping needed to interpret `GlobalHotKeyEvent::receiver()`
/// events (which only carry a numeric id, not which of our 3 actions it is).
pub struct Hotkeys {
    _manager: GlobalHotKeyManager,
    answer_id: u32,
    hangup_id: u32,
    mute_id: u32,
}

impl Hotkeys {
    /// Parse and register all three bindings (e.g. "Ctrl+Alt+A" syntax).
    /// Fails closed -- if the manager itself can't be created, or a binding
    /// fails to parse/register, no hotkeys are left half-registered; the
    /// caller should log the error and continue without hotkeys rather than
    /// fail the whole app over a misconfigured binding.
    pub fn spawn(answer: &str, hangup: &str, mute: &str) -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;

        let answer_key: HotKey = answer.parse()
            .map_err(|e| anyhow::anyhow!("Parsing answer hotkey {answer:?}: {e}"))?;
        let hangup_key: HotKey = hangup.parse()
            .map_err(|e| anyhow::anyhow!("Parsing hangup hotkey {hangup:?}: {e}"))?;
        let mute_key: HotKey = mute.parse()
            .map_err(|e| anyhow::anyhow!("Parsing mute hotkey {mute:?}: {e}"))?;

        manager.register(answer_key)?;
        manager.register(hangup_key)?;
        manager.register(mute_key)?;

        Ok(Self {
            _manager: manager,
            answer_id: answer_key.id(),
            hangup_id: hangup_key.id(),
            mute_id: mute_key.id(),
        })
    }

    /// Drain every pending event and return the actions they correspond to.
    /// Key-release events (`HotKeyState::Released`) are ignored -- only the
    /// press should trigger the action, exactly once per press.
    pub fn poll(&self) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if event.id == self.answer_id {
                actions.push(HotkeyAction::Answer);
            } else if event.id == self.hangup_id {
                actions.push(HotkeyAction::Hangup);
            } else if event.id == self.mute_id {
                actions.push(HotkeyAction::Mute);
            }
        }
        actions
    }
}
