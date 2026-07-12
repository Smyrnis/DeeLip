//! System tray icon with hide-to-tray behavior. Full picture (why this can't
//! poll from inside `update()`, the GTK-thread requirement, the
//! `Visible`-not-`Minimized` choice): `docs/crates/ui.md`'s "Platform integration"
//! section.

use std::cell::RefCell;
#[cfg(target_os = "linux")]
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
/// `u32::MAX` is never sent; `0` clears the badge.
///
/// Linux: safe to call from any thread (it's a `glib::MainContext` channel,
/// the officially-supported way to hand work to a GTK main loop running on
/// a different thread from the sender — the same category of problem
/// `MenuItem`/`Menu` needing to be *constructed* on the GTK thread already
/// solved for menu setup, just for an ongoing update instead of a one-time
/// handoff).
///
/// Windows/macOS: a plain channel, drained once per frame by
/// `poll_native_tray_badge` (see its doc comment for why there's no
/// GTK-style "attach to a running main loop" step needed there).
#[cfg(target_os = "linux")]
pub type BadgeSender = gtk::glib::Sender<u32>;
#[cfg(not(target_os = "linux"))]
pub type BadgeSender = std::sync::mpsc::Sender<u32>;

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

/// Restore via `Visible(true)`, not `Minimized(false)` -- see `docs/crates/ui.md`'s
/// "Platform integration" section for why.
fn restore_window(ctx_slot: &CtxSlot) {
    if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

/// Tooltip text for a given missed/unread count -- shared by every
/// platform's badge-update path.
fn tooltip_for_count(count: u32) -> String {
    if count > 0 {
        // Pluralization rules are out of scope for now (see
        // `ARCHITECTURE_GAPS.md` item 6) -- the English
        // singular/plural branch stays in Rust, with each
        // branch's fixed text as its own locale key.
        let key = if count == 1 { "tray.tooltip_missed_singular" } else { "tray.tooltip_missed_plural" };
        tf(key, &[("count", &count.to_string())])
    } else {
        t("tray.tooltip_default")
    }
}

/// Spawn the tray icon on a dedicated GTK event-loop thread -- see
/// `docs/crates/ui.md`'s "Platform integration" section for why menu/icon
/// construction and the badge channel both have to happen on this thread.
/// Build failures past the initial channel round-trip are logged, not
/// propagated (the app works fine without a tray icon).
#[cfg(target_os = "linux")]
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
                    if let Err(e) = tray.borrow().set_tooltip(Some(&tooltip_for_count(count))) {
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

/// Windows/macOS: build the tray icon directly (no dedicated event-loop
/// thread, unlike Linux's GTK approach) -- **must** be called after eframe's
/// own winit event loop is already running on this thread, not before. Per
/// `tray-icon`'s own docs: macOS strictly requires the tray to be created
/// once the event loop has started (the earliest safe point is winit's
/// `StartCause::Init`); Windows requires it be built on whichever thread's
/// event loop will pump its hidden window's messages. Both are satisfied by
/// calling this from `DeelipApp`'s first real frame (see
/// `frame.rs::init_lazy_tray`) rather than from `main.rs` before
/// `eframe::run_native`, since by the app's first frame eframe's winit loop
/// is definitely already pumping this thread's message queue -- which is
/// also what keeps `spawn_tray_event_handlers`'s two background threads
/// (unchanged, pure channel-consumers) fed on these platforms, with no
/// GTK-style dedicated main loop needed at all.
///
/// UNVERIFIED on real Windows/macOS hardware -- this sandbox is Linux-only.
#[cfg(not(target_os = "linux"))]
pub fn spawn_tray_icon() -> anyhow::Result<(TrayMenuIds, BadgeSender)> {
    let icon = load_icon(0).context("Tray icon: failed to load icon")?;

    let show_item = MenuItem::new(t("tray.show_item"), true, None);
    let quit_item = MenuItem::new(t("tray.quit_item"), true, None);
    let ids = TrayMenuIds { show: show_item.id().clone(), quit: quit_item.id().clone() };

    let menu = Menu::new();
    menu.append(&show_item).map_err(|e| anyhow::anyhow!("Tray icon: failed to append Show item: {e}"))?;
    menu.append(&quit_item).map_err(|e| anyhow::anyhow!("Tray icon: failed to append Quit item: {e}"))?;

    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("DeeLip")
        .build()
        .context("Tray icon: failed to build")?;

    let (badge_tx, badge_rx) = std::sync::mpsc::channel::<u32>();
    NATIVE_TRAY.with(|slot| *slot.borrow_mut() = Some(NativeTray { tray, badge_rx }));
    Ok((ids, badge_tx))
}

/// Windows/macOS only -- the tray icon built by `spawn_tray_icon`, plus its
/// badge-count channel, kept on the main thread (never sent across threads,
/// so no `Send`/`Sync` bound on `tray_icon::TrayIcon` is needed) for
/// `poll_native_tray_badge` to drain once per frame.
#[cfg(not(target_os = "linux"))]
struct NativeTray {
    tray: tray_icon::TrayIcon,
    badge_rx: std::sync::mpsc::Receiver<u32>,
}

#[cfg(not(target_os = "linux"))]
thread_local! {
    static NATIVE_TRAY: RefCell<Option<NativeTray>> = const { RefCell::new(None) };
}

/// Windows/macOS: apply the latest pending badge-count update (if any) to
/// the tray icon built by `spawn_tray_icon`. Unlike Linux, which attaches a
/// callback directly to the GTK main loop that's already running the tray,
/// there's no separate main loop here to attach to -- the tray was built
/// on the same thread `DeelipApp::update`/`ui` runs on, so this just needs
/// to be polled once per frame instead (see `frame.rs::process_tray_events`,
/// which already runs every frame). No-op if the tray never started (see
/// `spawn_tray_icon`'s doc comment for how construction failure is handled).
/// A no-op on Linux, where the GTK thread's own `badge_rx.attach` callback
/// already handles this.
#[cfg(not(target_os = "linux"))]
pub(crate) fn poll_native_tray_badge() {
    NATIVE_TRAY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else { return };
        // Only the most recent pending count matters -- drain fully rather
        // than acting on every queued update.
        let mut latest = None;
        while let Ok(count) = state.badge_rx.try_recv() {
            latest = Some(count);
        }
        let Some(count) = latest else { return };
        match load_icon(count) {
            Ok(icon) => {
                if let Err(e) = state.tray.set_icon(Some(icon)) {
                    tracing::warn!("Tray: failed to update badge icon: {e}");
                }
                if let Err(e) = state.tray.set_tooltip(Some(&tooltip_for_count(count))) {
                    tracing::warn!("Tray: failed to update tooltip: {e}");
                }
            }
            Err(e) => tracing::warn!("Tray: failed to render badge icon: {e}"),
        }
    });
}

#[cfg(target_os = "linux")]
pub(crate) fn poll_native_tray_badge() {}

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
