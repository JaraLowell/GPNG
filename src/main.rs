//! Graphic Packet — Tokio serial worker + egui pixel UI (icons + analog clock).

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use graphic_packet::config::Config;
use graphic_packet::tnc::{spawn_tnc_task, TncCommand};
use graphic_packet::ui;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();

    let tui = std::env::args().any(|a| a == "--tui");

    info!("Graphic Packet (modern) — {}", if tui { "TUI" } else { "pixel UI" });

    let config_path = resolve_config_path();
    let cfg = Config::load(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    // eframe owns the main thread; drive Tokio in the background.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;

    let (tnc_channels, tnc_join) = {
        let serial = cfg.serial.clone();
        let tnc = cfg.tnc.clone();
        rt.block_on(async { spawn_tnc_task(serial, tnc) })
    };

    let cmd_tx = tnc_channels.commands.clone();
    let events = tnc_channels.events;
    let shutdown_tx = cmd_tx.clone();

    let ui_result = ui::run(cfg, events, cmd_tx, tui);

    let _ = rt.block_on(shutdown_tx.send(TncCommand::Shutdown));
    tnc_join.abort();
    if let Err(err) = ui_result {
        error!("UI error: {err}");
        return Err(err);
    }

    info!("Graphic Packet stopped");
    Ok(())
}

fn resolve_config_path() -> PathBuf {
    for name in ["Config.toml", "config.toml", "CONFIG.GP", "config.gp"] {
        let p = PathBuf::from(name);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("Config.toml")
}
