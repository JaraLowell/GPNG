//! Operator UI for Graphic Packet.
//!
//! Default: **egui** pixel window with real `ICONS.GP` bitmaps and an analog
//! clock (classic BGI look). Optional `--tui` uses ratatui text mode.

pub mod ansi;
pub mod app;
mod beacon;
pub mod editor;
pub mod gui;
pub mod icons;
pub mod layout;
pub mod mheard;
pub mod padlog;
pub mod retro;
pub mod session;
pub mod setup;
pub mod theme;

use std::path::PathBuf;

use crate::config::Config;
use crate::tnc::{TncCommand, TncEvent};
use tokio::sync::mpsc;

/// Text UI. The caller has already opened the radio.
pub fn run_tui(
    cfg: Config,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
) -> anyhow::Result<()> {
    app::run(cfg, events, cmd_tx)
}

/// Pixel UI. The radio stays closed until the startup card's Use this.
pub fn run_gui(
    cfg: Config,
    config_path: PathBuf,
    radio: crate::tnc::RadioLink,
) -> anyhow::Result<()> {
    gui::run(cfg, config_path, radio)
}
