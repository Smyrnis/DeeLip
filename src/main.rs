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

    let account = config
        .accounts
        .into_iter()
        .find(|a| a.enabled)
        .ok_or_else(|| anyhow::anyhow!("No enabled accounts in config"))?;

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

    // ── SIP stack ─────────────────────────────────────────────────────────────
    let sip_handle = rt
        .block_on(SipStack::spawn(account, config.local_sip_port, external_ip))
        .context("Starting SIP stack")?;

    // ── eframe (main thread) ──────────────────────────────────────────────────
    let turn = config.turn_server.clone().map(|server| (
        server,
        config.turn_username.clone().unwrap_or_default(),
        config.turn_password.clone().unwrap_or_default(),
    ));
    let rt_handle = rt.handle().clone();
    let app       = DeelipApp::new(sip_handle, rt_handle, turn);

    let native_opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeeLip")
            .with_inner_size([400.0, 480.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "DeeLip",
        native_opts,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
