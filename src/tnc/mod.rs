//! TNC / modem interface — config-selected drivers (WA8DED, TFPCX, KISS).
//!
//! Only **one** driver is constructed at startup from `[tnc] protocol`.
//! Shared wire helpers live in [`serial`] and [`hostmode`]; each driver owns
//! its bring-up and poll loop.
//!
//! ```text
//!   wa8ded / jhost  → timed WA8DED hostmode poll (JHOST1/JHOST0)
//!   tfpcx           → soft-TNC, same hostmode frames
//!   kiss            → TAPR KISS (FEND/FESC)
//! ```

mod drivers;
mod hostmode;
mod kiss_ini;
mod serial;

use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::{HostProtocol, SerialConfig, TncConfig};

pub use hostmode::HostCode;

// ---------------------------------------------------------------------------
// Public types (stable UI surface)
// ---------------------------------------------------------------------------

/// Events emitted by the serial RX task toward the UI / session layer.
#[derive(Debug, Clone)]
pub enum TncEvent {
    /// Raw bytes observed on the wire (pre-decoder / debug).
    RawRx(Vec<u8>),
    /// A decoded hostmode / KISS frame (channel + payload).
    HostFrame {
        channel: u8,
        code: HostCode,
        payload: Vec<u8>,
    },
    /// Human-readable status for the terminal pane.
    Status(String),
    /// Non-fatal I/O hiccup; the loop may retry.
    Warning(String),
    /// Serial link closed or unrecoverable error.
    LinkDown(String),
}

/// Commands the UI / session layer may send toward the TNC task.
#[derive(Debug, Clone)]
#[allow(dead_code)] // TerminalLine / Reinitialize reserved for upcoming UI actions.
pub enum TncCommand {
    /// Send a data / command payload on a channel (hostmode) or KISS port.
    HostFrame {
        channel: u8,
        /// Command / data byte stream (e.g. `b"I NL0MSK"`).
        data: Vec<u8>,
    },
    /// Issue a terminal-mode ASCII line (CR-terminated), e.g. before JHOST1.
    TerminalLine(String),
    /// Re-run the configured `TNCINI` sequence.
    Reinitialize,
    /// Graceful shutdown: deinit (+ leave hostmode when applicable).
    Shutdown,
}

#[derive(Debug, Error)]
pub enum TncError {
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Protocol(String),
}

/// Channels bridging the blocking serial worker and async UI tasks.
pub struct TncChannels {
    pub events: mpsc::Receiver<TncEvent>,
    pub commands: mpsc::Sender<TncCommand>,
}

/// Spawn the blocking serial worker; constructs **only** the configured driver.
pub fn spawn_tnc_task(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
) -> (TncChannels, tokio::task::JoinHandle<()>) {
    let (event_tx, event_rx) = mpsc::channel::<TncEvent>(256);
    let (cmd_tx, cmd_rx) = mpsc::channel::<TncCommand>(64);

    let channels = TncChannels {
        events: event_rx,
        commands: cmd_tx,
    };

    let kind = tnc_cfg.protocol_kind();
    let join = tokio::task::spawn_blocking(move || {
        info!(?kind, "loading modem driver");
        let result = match kind {
            HostProtocol::Wa8ded => {
                drivers::wa8ded::run(serial, tnc_cfg, event_tx.clone(), cmd_rx)
            }
            HostProtocol::Tfpcx => {
                drivers::tfpcx::run(serial, tnc_cfg, event_tx.clone(), cmd_rx)
            }
            HostProtocol::Kiss => drivers::kiss::run(serial, tnc_cfg, event_tx.clone(), cmd_rx),
        };
        if let Err(err) = result {
            warn!("TNC loop exited: {err}");
            let _ = event_tx.blocking_send(TncEvent::LinkDown(err.to_string()));
        }
    });

    (channels, join)
}
