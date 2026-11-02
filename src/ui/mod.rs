//! Operator UI for Graphic Packet.
//!
//! Default: **egui** pixel window with real `ICONS.GP` bitmaps and an analog
//! clock (classic BGI look). Optional `--tui` uses ratatui text mode.

pub mod app;
pub mod editor;
pub mod gui;
pub mod icons;
pub mod layout;
pub mod mheard;
pub mod session;
pub mod theme;

use crate::config::Config;
use crate::tnc::{TncCommand, TncEvent};
use tokio::sync::mpsc;

/// Start the operator UI. Pass `tui = true` for the text fallback.
pub fn run(
    cfg: Config,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
    tui: bool,
) -> anyhow::Result<()> {
    if tui {
        app::run(cfg, events, cmd_tx)
    } else {
        gui::run(cfg, events, cmd_tx)
    }
}
