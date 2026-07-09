use anyhow::Context;
use tracing::warn;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::EnvFilter;

use deelip_config::{AppConfig, Db};
use deelip_sip::SipStack;
use deelip_ui::DeelipApp;

fn main() -> anyhow::Result<()> {
    // ── Config ────────────────────────────────────────────────────────────────
    // `Db::open_default()` creates `~/.config/deelip/deelip.db` on first run,
    // one-time-importing any existing `config.toml`/`contacts.json`/
    // `history.json` into it (left on disk untouched), or seeding a single
    // default account if there's no legacy data to import either.
    //
    // Deliberately opened before logging is set up below -- `log_to_file`
    // decides the tracing subscriber's writer, and a `tracing_subscriber`
    // global subscriber can only be installed once (`.init()` panics on a
    // second call), so there's no way to defer this decision until after an
    // initial console-only subscriber were already active.
    let db = Db::open_default().context("Opening database")?;
    let config = AppConfig::load(&db).context("Loading config")?;

    // ── Logging ───────────────────────────────────────────────────────────────
    // `_log_guard` (only bound when logging to a file) must live for the
    // whole process -- its `Drop` flushes the non-blocking writer's queue;
    // dropping it early would silently lose buffered log lines on exit.
    let make_filter = || {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("deelip=debug,deelip_sip=debug,deelip_media=debug,deelip_nat=info")
        })
    };
    let _log_guard = if config.log_to_file {
        match deelip_config::log_file_path() {
            Ok(path) => {
                let parent = path.parent().expect("log_file_path always has a parent").to_path_buf();
                let file_name = path.file_name().expect("log_file_path always has a file name").to_owned();
                let appender = tracing_appender::rolling::never(parent, file_name);
                let (non_blocking, guard) = tracing_appender::non_blocking(appender);
                tracing_subscriber::fmt()
                    .with_env_filter(make_filter())
                    .with_writer(std::io::stdout.and(non_blocking))
                    .init();
                Some(guard)
            }
            Err(e) => {
                tracing_subscriber::fmt().with_env_filter(make_filter()).init();
                warn!("Failed to resolve log file path ({e:#}), logging to console only");
                None
            }
        }
    } else {
        tracing_subscriber::fmt().with_env_filter(make_filter()).init();
        None
    };

    tracing::info!("DeeLip v{}", env!("CARGO_PKG_VERSION"));

    if config.crash_reporting_enabled {
        install_crash_hook();
    }

    let enabled_accounts: Vec<_> = config
        .accounts
        .iter()
        .filter(|a| a.enabled)
        .cloned()
        .collect();
    let had_enabled_accounts = !enabled_accounts.is_empty();
    if !had_enabled_accounts {
        // No hand-editable config file to point the user at anymore -- the
        // Settings tab is already a full account editor, so launch the GUI
        // instead of exiting, same as today's zero-accounts-configured state
        // (`refresh_idle_status`'s "No accounts configured" branch) already
        // renders correctly.
        warn!("No enabled accounts configured — launching DeeLip so you can add one in Settings");
    }

    // ── Tokio runtime (background) ────────────────────────────────────────────
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Building tokio runtime")?;

    // ── STUN: discover external IP (optional) ─────────────────────────────────
    let external_ip: Option<String> = if let Some(stun) = &config.stun_server {
        match rt.block_on(deelip_nat::discover_external_addr(stun)) {
            Ok(addr) => {
                let ip = addr.ip().to_string();
                tracing::info!("STUN discovered external IP: {ip} (port {})", addr.port());
                Some(ip)
            }
            Err(e) => {
                warn!("STUN failed ({e}), using local IP");
                None
            }
        }
    } else {
        None
    };

    // ── SIP stacks ────────────────────────────────────────────────────────────
    // Network settings (STUN/TURN/ICE) are process-wide, not per-account --
    // every stack shares this one `NetworkConfig` (SDP construction and
    // STUN/TURN/ICE resolution now happen inside `SipStack` itself, see
    // `deelip_sip::media_setup`).
    let network = deelip_sip::media_setup::NetworkConfig {
        stun_server: config.stun_server.clone(),
        turn_server: config.turn_server.clone(),
        turn_username: config.turn_username.clone().unwrap_or_default(),
        turn_password: config.turn_password.clone().unwrap_or_default(),
        ice_enabled: config.ice_enabled,
        rtp_port_range: config.rtp_port_min.zip(config.rtp_port_max),
        custom_nameserver: config.custom_nameserver.clone(),
        dns_srv_enabled: config.dns_srv_enabled,
    };

    // Each enabled account gets its own independent stack (own transport,
    // own registration loop) on a distinct local port derived from the
    // configured base port — one process-wide UDP/TCP bind can't serve two
    // accounts at once. A stack that fails to start (bad DNS, refused
    // connection, etc.) is logged and skipped rather than aborting the
    // others; the app only fails to start if every account failed.
    let mut account_handles = Vec::new();
    for (i, account) in enabled_accounts.into_iter().enumerate() {
        let local_port = config.local_sip_port + i as u16;
        let username = account.username.clone();
        match rt.block_on(SipStack::spawn(
            account.clone(),
            network.clone(),
            local_port,
            external_ip.clone(),
        )) {
            Ok(handle) => account_handles.push((account, handle)),
            Err(e) => warn!("Account {username} failed to start ({e}), skipping"),
        }
    }
    if account_handles.is_empty() && had_enabled_accounts {
        anyhow::bail!("All configured accounts failed to start");
    }

    // GTK's tray icon (via libappindicator) pulls in libcanberra for UI
    // feedback sounds, which probes every audio backend it knows about
    // (jack, ALSA oss/dmix/dsnoop/route) roughly once a second for as long
    // as the tray's GTK main loop runs — regardless of app/call state. Only
    // matters on systems where none of those backends are actually usable
    // (e.g. no running sound server), where it just spams stderr forever.
    // DeeLip has no use for GTK event sounds anyway; disable canberra's
    // audio output entirely rather than let it keep retrying. Must be set
    // before spawn_tray_icon() below, which is what starts GTK.
    std::env::set_var("CANBERRA_DRIVER", "null");

    // ── eframe (main thread) ──────────────────────────────────────────────────
    let tray = match deelip_ui::tray::spawn_tray_icon() {
        Ok((tray_ids, badge_tx)) => {
            let ctx_slot: deelip_ui::tray::CtxSlot =
                std::sync::Arc::new(std::sync::Mutex::new(None));
            let quit_state = deelip_ui::tray::QuitState {
                calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                pending: std::sync::Arc::new(std::sync::Mutex::new(None)),
                rt: rt.handle().clone(),
            };
            deelip_ui::tray::spawn_tray_event_handlers(
                tray_ids,
                ctx_slot.clone(),
                quit_state.clone(),
            );
            Some((ctx_slot, quit_state, badge_tx))
        }
        Err(e) => {
            warn!("Tray icon failed to start ({e}), continuing without it");
            None
        }
    };

    // ── Force X11/XWayland for the main window only ───────────────────────────
    // winit's native Wayland backend has no protocol-level way for a client to
    // un-minimize/restore its own window (only the compositor can) and its
    // Close command isn't reliably processed while minimized either — both
    // confirmed against winit's own Wayland source. X11 (via XWayland,
    // present on effectively every desktop Wayland session) doesn't have
    // these restrictions; this is the same trick Steam and most older
    // cross-platform Linux apps use. Must happen *after* spawn_tray_icon()
    // returns (which blocks until its GTK thread has already called
    // gtk::init()) so the tray keeps using native Wayland — GTK's own
    // event dispatch for our menu broke when this ran before gtk::init().
    std::env::remove_var("WAYLAND_DISPLAY");

    let rt_handle = rt.handle().clone();
    let app = DeelipApp::new(account_handles, rt_handle, config, db, tray);

    let native_opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeeLip")
            .with_inner_size([500.0, 500.0])
            .with_resizable(true)
            .with_icon(load_window_icon()),
        // NOTE: start-minimized is NOT implemented via `.with_visible()` here --
        // eframe's glutin backend unconditionally creates the window hidden and
        // force-shows it after the first rendered frame regardless of what
        // NativeOptions requests (see glow_integration.rs's "fix white flash on
        // startup" workaround), so any visibility hint set here gets silently
        // overridden. DeelipApp::update() instead sends an explicit
        // ViewportCommand::Visible(false) on its first frame, which runs after
        // eframe's forced show and so actually sticks.
        ..Default::default()
    };

    eframe::run_native(
        "DeeLip",
        native_opts,
        Box::new(|cc| {
            deelip_ui::install_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}

/// Install a panic hook that saves a local crash-report file (see
/// `write_crash_report`) before falling through to the previous hook
/// (still prints to stderr -- chained, not replaced, so console/log
/// behavior is unchanged). Gated on `AppConfig::crash_reporting_enabled`;
/// purely local, nothing here ever transmits anywhere.
fn install_crash_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Err(e) = write_crash_report(info) {
            eprintln!("Failed to write crash report: {e:#}");
        }
        previous_hook(info);
    }));
}

fn write_crash_report(info: &std::panic::PanicHookInfo) -> anyhow::Result<()> {
    let dir = deelip_config::crashes_dir()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".into());
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".into());
    let backtrace = std::backtrace::Backtrace::force_capture();

    let report = format!(
        "DeeLip crash report\n\
         Version: {}\n\
         Unix time: {now}\n\
         OS: {} {}\n\
         \n\
         Panic: {message}\n\
         Location: {location}\n\
         \n\
         Backtrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    std::fs::write(dir.join(format!("crash-{now}.txt")), report)?;
    Ok(())
}

/// The window/taskbar icon (distinct from the tray icon -- see
/// `deelip_ui::tray`'s own embedded asset). egui's own docs recommend a
/// square image "e.g. 256x256 pixels", which is exactly what's embedded here.
fn load_window_icon() -> egui::IconData {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(ICON_BYTES)
        .expect("assets/icon.png must be a valid image")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }
}
