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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use deelip_sip::SipCommand;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{Menu, MenuId, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::strings::{t, tf};

const ICON_BYTES: &[u8] = include_bytes!("../../../../assets/Deelip-tray.png");

/// Sends an updated missed-call/unread count to the tray icon's badge —
/// `u32::MAX` is never sent; `0` clears the badge. Safe to call from any
/// thread (it's a `glib::MainContext` channel, the officially-supported
/// way to hand work to a GTK main loop running on a different thread from
/// the sender — the same category of problem `MenuItem`/`Menu` needing to
/// be *constructed* on the GTK thread already solved for menu setup, just
/// for an ongoing update instead of a one-time handoff).
pub type BadgeSender = gtk::glib::Sender<u32>;

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
///
/// Also returns a `BadgeSender` for updating the missed-call/unread count
/// overlay — `TrayIcon::set_icon` (like `MenuItem`/`Menu` construction) must
/// run on this same GTK thread, so a `glib::MainContext` channel is set up
/// and attached *before* `gtk::main()` starts, and the returned sender is the
/// only thread-safe way in from the outside.
#[allow(deprecated)] // `MainContext::channel` -- the suggested async-channel + spawn_future_local
                     // replacement doesn't fit this thread's plain `gtk::main()` loop; this is
                     // still the documented way to feed a synchronous cross-thread channel into
                     // a classic (non-async) GLib main loop.
pub fn spawn_tray_icon() -> anyhow::Result<(TrayMenuIds, BadgeSender)> {
    let (ids_tx, ids_rx) = std::sync::mpsc::channel::<TrayMenuIds>();
    let (badge_tx, badge_rx) = gtk::glib::MainContext::channel::<u32>(gtk::glib::Priority::default());
    let badge_tx_ret = badge_tx.clone();

    std::thread::spawn(move || {
        if let Err(e) = gtk::init() {
            tracing::error!("Tray icon: gtk::init failed: {e}");
            return;
        }

        let icon = match load_icon(0) {
            Ok(icon) => icon,
            Err(e) => {
                tracing::error!("Tray icon: failed to load icon: {e}");
                return;
            }
        };

        let show_item = MenuItem::new(t("tray.show_item"), true, None);
        let quit_item = MenuItem::new(t("tray.quit_item"), true, None);
        let _ = ids_tx.send(TrayMenuIds { show: show_item.id().clone(), quit: quit_item.id().clone() });

        let menu = Menu::new();
        if menu.append(&show_item).is_err() || menu.append(&quit_item).is_err() {
            tracing::error!("Tray icon: failed to build menu");
            return;
        }

        let tray = match TrayIconBuilder::new().with_icon(icon).with_menu(Box::new(menu)).with_tooltip("DeeLip").build()
        {
            Ok(tray) => tray,
            Err(e) => {
                tracing::error!("Tray icon: failed to build: {e}");
                return;
            }
        };

        // `Rc`, not `Arc` -- never leaves this thread; `badge_rx`'s closure
        // below runs on this same GTK main loop, not a separate thread.
        let tray = Rc::new(RefCell::new(tray));
        badge_rx.attach(None, move |count| {
            match load_icon(count) {
                Ok(icon) => {
                    if let Err(e) = tray.borrow_mut().set_icon(Some(icon)) {
                        tracing::warn!("Tray: failed to update badge icon: {e}");
                    }
                    let tooltip = if count > 0 {
                        // Pluralization rules are out of scope for now (see
                        // `ARCHITECTURE_GAPS.md` item 6) -- the English
                        // singular/plural branch stays in Rust, with each
                        // branch's fixed text as its own locale key.
                        let key =
                            if count == 1 { "tray.tooltip_missed_singular" } else { "tray.tooltip_missed_plural" };
                        tf(key, &[("count", &count.to_string())])
                    } else {
                        t("tray.tooltip_default")
                    };
                    if let Err(e) = tray.borrow().set_tooltip(Some(&tooltip)) {
                        tracing::warn!("Tray: failed to update tooltip: {e}");
                    }
                }
                Err(e) => tracing::warn!("Tray: failed to render badge icon: {e}"),
            }
            gtk::glib::ControlFlow::Continue
        });

        gtk::main();
    });

    let ids = ids_rx.recv().context("Tray thread failed before creating menu items")?;
    Ok((ids, badge_tx_ret))
}

/// Load the base tray icon, compositing a small red badge with `count`
/// (capped at a single digit, "9" for anything ≥9 -- a badge this size has
/// no room for two digits) in the bottom-right corner if `count > 0`.
fn load_icon(count: u32) -> anyhow::Result<Icon> {
    let mut img = image::load_from_memory(ICON_BYTES)?.into_rgba8();
    if count > 0 {
        draw_badge(&mut img, count.min(9) as u8);
    }
    let (width, height) = img.dimensions();
    Ok(Icon::from_rgba(img.into_raw(), width, height)?)
}

/// Minimal 5x7 bitmap font for digits 0-9 (each row a 5-bit pattern, MSB =
/// leftmost pixel) -- hand-rolled rather than pulling in a font-rendering
/// dependency for this one small fixed glyph set, matching this codebase's
/// existing preference for hand-rolled parsing/rendering over new deps for
/// simple, fixed-shape needs (see `sdp.rs`/`message.rs`/`auth.rs`).
const DIGIT_FONT: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11110, 0b00001, 0b00001, 0b00110, 0b00001, 0b00001, 0b11110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

/// Draw a filled red circle with a white digit in the bottom-right corner
/// of a 64x64-ish RGBA icon. `digit` must be 0-9.
fn draw_badge(img: &mut image::RgbaImage, digit: u8) {
    let (w, h) = img.dimensions();
    let radius: i32 = (w.min(h) as i32) * 15 / 64; // scales with icon size, tuned for the 64x64 asset
    let cx = w as i32 - radius - 2;
    let cy = h as i32 - radius - 2;
    let red = image::Rgba([220u8, 40, 40, 255]);
    let white = image::Rgba([255u8, 255, 255, 255]);

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let (x, y) = (cx + dx, cy + dy);
            if x < 0 || y < 0 || x as u32 >= w || y as u32 >= h {
                continue;
            }
            img.put_pixel(x as u32, y as u32, red);
        }
    }

    let glyph = &DIGIT_FONT[digit as usize];
    let scale = (radius * 2 / 7).max(1); // font is 5x7, fit within the circle's diameter
    let glyph_w = 5 * scale;
    let glyph_h = 7 * scale;
    let ox = cx - glyph_w / 2;
    let oy = cy - glyph_h / 2;
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) == 0 {
                continue;
            }
            for py in 0..scale {
                for px in 0..scale {
                    let (x, y) = (ox + col * scale + px, oy + row as i32 * scale + py);
                    if x < 0 || y < 0 || x as u32 >= w || y as u32 >= h {
                        continue;
                    }
                    img.put_pixel(x as u32, y as u32, white);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tray.rs"]
mod tests;
