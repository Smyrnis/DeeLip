//! System tray icon with hide-to-tray behavior.
//!
//! On Linux, `tray-icon` needs a GTK event loop running on the same thread
//! that created the tray icon (not winit's own loop, which is what eframe
//! uses) — so this spawns a dedicated OS thread that runs `gtk::main()` for
//! the lifetime of the process.
//!
//! Tray/menu click handling deliberately does NOT poll from inside
//! `DeelipApp::update()`: eframe/winit pause the render/update loop while
//! the window is hidden (a normal optimization), which means anything
//! that only runs inside `update()` simply never fires while hidden —
//! including "restore" and "quit" clicks, the two actions you most need
//! while hidden. Instead, `spawn_tray_event_handlers` runs dedicated
//! background threads that block on `tray_icon`'s own process-wide event
//! channels and act independently of whether any frame is being drawn:
//! `egui::Context` is thread-safe by design specifically for this
//! (`send_viewport_cmd`/`request_repaint` from any thread), and Quit's
//! hangup logic works off a small piece of state mirrored from `DeelipApp`
//! once per frame while the window is visible (nothing changes it while
//! hidden, so a stale-by-one-frame copy is always correct at hide time).
//!
//! Hiding/restoring uses `ViewportCommand::Visible`, not `Minimized`: window
//! mapping (`Visible`) is baseline ICCCM behavior every X11 window manager
//! gets right, whereas GNOME Shell/Mutter's handling of the WM-level iconify
//! state (`_NET_WM_STATE_HIDDEN`) for an XWayland-forced client is unreliable
//! and could leave "Show DeeLip" doing nothing at all.

use std::sync::{Arc, Mutex};

use anyhow::Context;
use deelip_sip::SipCommand;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{Menu, MenuId, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

const ICON_BYTES: &[u8] = include_bytes!("../../../assets/icon.png");

/// IDs of the tray menu's "Show" and "Quit" items.
#[derive(Clone)]
pub struct TrayMenuIds {
    pub show: MenuId,
    pub quit: MenuId,
}

/// Shared slot `DeelipApp::update()` refreshes every frame so the
/// independent tray-event threads can call `send_viewport_cmd` even while
/// no frame is currently being processed.
pub type CtxSlot = Arc<Mutex<Option<egui::Context>>>;

/// A call's own command sender (which account it's on) plus its call ID —
/// enough for the Quit thread to hang it up independently.
type CallHandle = (UnboundedSender<SipCommand>, String);

/// Just enough call state for the Quit thread to hang up every active call
/// (and reject a pending incoming one) before exiting, without needing the
/// whole `DeelipApp`. Mirrored from `DeelipApp`'s own state once per frame.
/// Each call carries its own account's `cmd_tx` since DeeLip can have
/// several accounts — and now several concurrent calls — at once.
#[derive(Clone)]
pub struct QuitState {
    pub calls: Arc<Mutex<Vec<CallHandle>>>,
    pub pending: Arc<Mutex<Option<CallHandle>>>,
    pub rt: tokio::runtime::Handle,
}

impl QuitState {
    fn hangup_and_exit(&self) {
        let calls = std::mem::take(&mut *self.calls.lock().unwrap());
        let pending = self.pending.lock().unwrap().take();
        let mut sent = false;
        for (tx, call_id) in calls {
            let _ = tx.send(SipCommand::HangUp { call_id });
            sent = true;
        }
        if let Some((tx, call_id)) = pending {
            let _ = tx.send(SipCommand::RejectCall { call_id });
            sent = true;
        }
        if sent {
            self.rt.block_on(tokio::time::sleep(std::time::Duration::from_millis(200)));
        }
        tracing::info!("Tray: Quit selected, exiting");
        std::process::exit(0);
    }
}

/// Spawn the two background threads that handle tray/menu clicks
/// independently of the egui render loop (see module docs for why).
pub fn spawn_tray_event_handlers(tray_ids: TrayMenuIds, ctx_slot: CtxSlot, quit_state: QuitState) {
    std::thread::spawn({
        let ctx_slot = ctx_slot.clone();
        move || {
            while let Ok(event) = tray_icon::TrayIconEvent::receiver().recv() {
                tracing::debug!("Tray: got TrayIconEvent: {event:?}");
                if let tray_icon::TrayIconEvent::Click { .. } = event {
                    restore_window(&ctx_slot);
                }
            }
        }
    });

    std::thread::spawn(move || {
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().recv() {
            tracing::debug!("Tray: got MenuEvent id={:?}", event.id);
            if event.id == tray_ids.show {
                restore_window(&ctx_slot);
            } else if event.id == tray_ids.quit {
                quit_state.hangup_and_exit();
            }
        }
    });
}

/// Restore the window via `Visible(true)`, not `Minimized(false)`. Window
/// mapping (`Visible`) is baseline ICCCM behavior every X11 window manager
/// implements correctly; GNOME Shell/Mutter's handling of the WM-level
/// iconify state (`_NET_WM_STATE_HIDDEN`) for an XWayland-forced client (see
/// `main.rs`'s `WAYLAND_DISPLAY` removal) is unreliable and could leave
/// "Show DeeLip" doing nothing at all.
fn restore_window(ctx_slot: &CtxSlot) {
    if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

/// Spawn the tray icon on a dedicated GTK event-loop thread. Blocks briefly
/// (a channel round-trip, not real work) until the menu items exist so their
/// IDs can be returned — `MenuItem`/`Menu` use `Rc`, so they must be built
/// *on* the GTK thread, not constructed here and moved in. Build failures
/// past that point are logged rather than propagated, matching this
/// codebase's existing pattern for non-fatal background failures (e.g. STUN
/// discovery in `main.rs`).
pub fn spawn_tray_icon() -> anyhow::Result<TrayMenuIds> {
    let (ids_tx, ids_rx) = std::sync::mpsc::channel::<TrayMenuIds>();

    std::thread::spawn(move || {
        if let Err(e) = gtk::init() {
            tracing::error!("Tray icon: gtk::init failed: {e}");
            return;
        }

        let icon = match load_icon() {
            Ok(icon) => icon,
            Err(e) => {
                tracing::error!("Tray icon: failed to load icon: {e}");
                return;
            }
        };

        let show_item = MenuItem::new("Show DeeLip", true, None);
        let quit_item = MenuItem::new("Quit", true, None);
        let _ = ids_tx.send(TrayMenuIds { show: show_item.id().clone(), quit: quit_item.id().clone() });

        let menu = Menu::new();
        if menu.append(&show_item).is_err() || menu.append(&quit_item).is_err() {
            tracing::error!("Tray icon: failed to build menu");
            return;
        }

        let _tray = match TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("DeeLip")
            .build()
        {
            Ok(tray) => tray,
            Err(e) => {
                tracing::error!("Tray icon: failed to build: {e}");
                return;
            }
        };

        gtk::main();
    });

    ids_rx.recv().context("Tray thread failed before creating menu items")
}

fn load_icon() -> anyhow::Result<Icon> {
    let img = image::load_from_memory(ICON_BYTES)?.into_rgba8();
    let (width, height) = img.dimensions();
    Ok(Icon::from_rgba(img.into_raw(), width, height)?)
}
