use anyhow::Context;
use tracing::warn;
use tracing_subscriber::EnvFilter;

use deelip_config::{default_config_path, AppConfig};
use deelip_sip::SipStack;
use deelip_ui::DeelipApp;

fn main() -> anyhow::Result<()> {
    // ── Logging ───────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(
                    "deelip=debug,deelip_sip=debug,deelip_media=debug,deelip_nat=info",
                )),
        )
        .init();

    tracing::info!("DeeLip v{}", env!("CARGO_PKG_VERSION"));

    // ── Config ────────────────────────────────────────────────────────────────
    let config_path = default_config_path().context("Config path")?;
    let config = if config_path.exists() {
        AppConfig::load(&config_path).context("Loading config")?
    } else {
        warn!("No config at {}", config_path.display());
        let default = AppConfig::default();
        default.save(&config_path)?;
        tracing::info!("Default config written to {}", config_path.display());
        tracing::info!("Edit it with your SIP credentials and re-run.");
        return Ok(());
    };

    let enabled_accounts: Vec<_> = config.accounts.iter().filter(|a| a.enabled).cloned().collect();
    if enabled_accounts.is_empty() {
        anyhow::bail!("No enabled accounts in config");
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
    // Each enabled account gets its own independent stack (own transport,
    // own registration loop) on a distinct local port derived from the
    // configured base port — one process-wide UDP/TCP bind can't serve two
    // accounts at once. A stack that fails to start (bad DNS, refused
    // connection, etc.) is logged and skipped rather than aborting the
    // others; the app only fails to start if every account failed.
    let mut account_handles = Vec::new();
    for (i, account) in enabled_accounts.into_iter().enumerate() {
        let local_port = config.local_sip_port + i as u16;
        let username   = account.username.clone();
        match rt.block_on(SipStack::spawn(account.clone(), local_port, external_ip.clone())) {
            Ok(handle) => account_handles.push((account, handle)),
            Err(e) => warn!("Account {username} failed to start ({e}), skipping"),
        }
    }
    if account_handles.is_empty() {
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
        Ok(tray_ids) => {
            let ctx_slot: deelip_ui::tray::CtxSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
            let quit_state = deelip_ui::tray::QuitState {
                calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                pending: std::sync::Arc::new(std::sync::Mutex::new(None)),
                rt: rt.handle().clone(),
            };
            deelip_ui::tray::spawn_tray_event_handlers(tray_ids, ctx_slot.clone(), quit_state.clone());
            Some((ctx_slot, quit_state))
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
    let app       = DeelipApp::new(account_handles, rt_handle, config, config_path, tray);

    let native_opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeeLip")
            .with_inner_size([420.0, 500.0])
            .with_resizable(true),
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
