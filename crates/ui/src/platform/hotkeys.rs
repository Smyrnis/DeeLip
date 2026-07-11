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
    /// Fed by a forwarding thread (spawned in `spawn`) that blocks on
    /// `GlobalHotKeyEvent::receiver()` -- that receiver is process-wide and
    /// owned by the `global-hotkey` crate itself, so we can't hook a repaint
    /// request into its send side directly. Forwarding through our own
    /// channel lets that thread call `ctx.request_repaint()` right after
    /// each event, the same "wake the UI thread instead of making it poll"
    /// idiom `platform::tray`'s event threads already use, instead of
    /// `poll` only ever noticing a press whenever the idle repaint timer
    /// next happens to fire.
    event_rx: std::sync::mpsc::Receiver<GlobalHotKeyEvent>,
}

impl Hotkeys {
    /// Parse and register the three custom bindings (e.g. "Ctrl+Alt+A"
    /// syntax) if `custom` is given, plus, if `media_buttons` is set, a
    /// bare (no-modifier) grab of the hardware media "Play/Pause" key --
    /// `global-hotkey`'s X11 backend maps `Code::MediaPlayPause` straight
    /// to the `XF86AudioPlay` keysym, so this needs no separate MPRIS/evdev
    /// mechanism, just a second registration on the same manager. Both are
    /// independent -- either, both, or (return `Ok` with nothing
    /// registered, harmless) neither may be requested.
    ///
    /// Fails closed -- if the manager itself can't be created, or a
    /// binding fails to parse/register, no hotkeys are left
    /// half-registered; the caller should log the error and continue
    /// without hotkeys rather than fail the whole app over a misconfigured
    /// binding.
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
