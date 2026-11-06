//! Graphic Packet — Tokio serial worker + egui pixel UI (icons + analog clock).

// `--release` does not hide a console by itself. Rust links a CONSOLE-subsystem
// exe unless told otherwise, and Windows then allocates a terminal on launch.
// Debug builds stay console apps so `cargo run` still shows logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

use anyhow::{Context, Result};
use graphic_packet::config::Config;
use graphic_packet::shelllog;
use graphic_packet::tnc::RadioLink;
use graphic_packet::ui;
use graphic_packet::version::{self, APP_NAME};
use tracing::{error, info};

fn main() -> Result<()> {
    let tui = std::env::args().any(|a| a == "--tui");
    // WINDOWS-subsystem release builds have no console. The text UI needs one.
    #[cfg(all(windows, not(debug_assertions)))]
    if tui {
        attach_console();
    }

    shelllog::init();

    info!(
        "{APP_NAME} v{} — {}",
        version::APP_VERSION,
        if tui { "TUI" } else { "pixel UI" }
    );
    info!(
        "shell log {}",
        graphic_packet::paths::data_dir()
            .join("log")
            .join("gpng.log")
            .display()
    );

    let config_path = resolve_config_path();
    let cfg = Config::load(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;
    if let Err(err) = graphic_packet::transfer::sevenplus::ensure_user_dir(&cfg.paths.user_dir) {
        tracing::warn!("USER dir {}: {err}", cfg.paths.user_dir.display());
    }
    let mail_dir = graphic_packet::remote::mail::mail_dir();
    if let Err(err) = graphic_packet::remote::mail::ensure_mail_dir(&mail_dir) {
        tracing::warn!("mail dir {}: {err}", mail_dir.display());
    }

    // eframe owns the main thread; drive Tokio in the background.
    // The pixel UI opens the radio from the startup card. `--tui` opens it now.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;

    let radio = RadioLink::new(rt.handle().clone());
    let ui_result = if tui {
        let (events, cmd_tx) = radio.open(cfg.serial.clone(), cfg.tnc.clone());
        ui::run_tui(cfg, events, cmd_tx)
    } else {
        ui::run_gui(cfg, config_path, radio.clone())
    };

    let stopped_driver = radio.shutdown();
    if stopped_driver {
        // Brief pause so Windows releases the COM handle before a quick relaunch.
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    if let Err(err) = ui_result {
        error!("UI error: {err}");
        return Err(err);
    }

    info!("Graphic Packet stopped");
    Ok(())
}

/// Release builds are GUI-subsystem executables, so they start with no console.
/// `--tui` attaches to the parent terminal, or opens one when launched from Explorer.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_console() {
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
        fn AllocConsole() -> i32;
    }
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            let _ = AllocConsole();
        }
    }
}

fn resolve_config_path() -> PathBuf {
    for name in ["Config.toml", "config.toml", "CONFIG.GP", "config.gp"] {
        if let Some(path) = graphic_packet::paths::find_file(name) {
            return path;
        }
    }
    PathBuf::from("Config.toml")
}
