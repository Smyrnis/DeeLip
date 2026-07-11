//! Global (system-wide) hotkeys for Answer/Hangup/Mute. See `docs/crates/ui.md`'s
//! "Platform integration" section for the X11-only/tray-restore context and
//! why this needs no GTK-style setup ordering.

use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::platform::tray::CtxSlot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Answer,
    Hangup,
    Mute,
    /// A hardware headset/multimedia "hook" button (`Code::MediaPlayPause`
    /// -- the same physical button many BT/USB headsets use for both media
    /// playback and call answer/hangup, hence the shared code point) --
    /// interpreted by the caller as "Answer if ringing, else Hangup",
    /// matching a real phone's hook-switch behavior. See `Hotkeys::spawn`'s
    /// `media_buttons` parameter.
    MediaHook,
}

/// Owns the manager -- dropping it unregisters every binding -- plus the
/// id -> action mapping needed to interpret `GlobalHotKeyEvent::receiver()`
/// events (which only carry a numeric id, not which action it is).
pub struct Hotkeys {
    _manager: GlobalHotKeyManager,
    /// `None` when `global_hotkeys_enabled` is off but `handle_media_buttons`
    /// is on -- the two toggles are independent, so a user can have one
    /// without the other.
    custom_ids: Option<(u32, u32, u32)>,
    media_hook_id: Option<u32>,
    /// Fed by a forwarding thread (see `docs/crates/ui.md`'s "Platform integration"
    /// section for the shared background-thread-plus-channel idiom).
    event_rx: std::sync::mpsc::Receiver<GlobalHotKeyEvent>,
}

impl Hotkeys {
    /// Parse and register the three custom bindings if `custom` is given,
    /// plus a bare grab of the hardware media "Play/Pause" key if
    /// `media_buttons` is set -- independent of each other, either/both/
    /// neither may be requested. Fails closed: if the manager can't be
    /// created or any binding fails to parse/register, nothing is left
    /// half-registered.
    pub fn spawn(custom: Option<(&str, &str, &str)>, media_buttons: bool, ctx_slot: CtxSlot) -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;

        let custom_ids = match custom {
            Some((answer, hangup, mute)) => {
                let answer_key: HotKey =
                    answer.parse().map_err(|e| anyhow::anyhow!("Parsing answer hotkey {answer:?}: {e}"))?;
                let hangup_key: HotKey =
                    hangup.parse().map_err(|e| anyhow::anyhow!("Parsing hangup hotkey {hangup:?}: {e}"))?;
                let mute_key: HotKey =
                    mute.parse().map_err(|e| anyhow::anyhow!("Parsing mute hotkey {mute:?}: {e}"))?;
                manager.register(answer_key)?;
                manager.register(hangup_key)?;
                manager.register(mute_key)?;
                Some((answer_key.id(), hangup_key.id(), mute_key.id()))
            }
            None => None,
        };

        let media_hook_id = if media_buttons {
            let media_key = HotKey::new(None, Code::MediaPlayPause);
            manager.register(media_key)?;
            Some(media_key.id())
        } else {
            None
        };

        let (tx, event_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                if tx.send(event).is_err() {
                    break;
                }
                if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
        });

        Ok(Self { _manager: manager, custom_ids, media_hook_id, event_rx })
    }

    /// Drain every pending event and return the actions they correspond to.
    /// Key-release events (`HotKeyState::Released`) are ignored -- only the
    /// press should trigger the action, exactly once per press.
    pub fn poll(&self) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if let Some((answer_id, hangup_id, mute_id)) = self.custom_ids {
                if event.id == answer_id {
                    actions.push(HotkeyAction::Answer);
                    continue;
                } else if event.id == hangup_id {
                    actions.push(HotkeyAction::Hangup);
                    continue;
                } else if event.id == mute_id {
                    actions.push(HotkeyAction::Mute);
                    continue;
                }
            }
            if self.media_hook_id == Some(event.id) {
                actions.push(HotkeyAction::MediaHook);
            }
        }
        actions
    }
}
