//! TNC / modem interface — WA8DED TNC2, TFPCX, KISS and FT232R BayCom.
//!
//! Only **one** driver is loaded, from `[tnc] protocol`. The pixel UI starts
//! that driver when the operator presses Use this; `--tui` starts it immediately.
//! Shared wire helpers live in [`serial`] and [`hostmode`]; each driver owns
//! its bring-up and poll loop.
//! The session service sits between UI and driver: host AX.25 for KISS/BayCom,
//! channel-command translation for WA8DED (whose TNC owns AX.25). TFPCX routes
//! each logical port to a KISS or WA8DED plugin.
//!
//! ```text
//!   wa8ded / jhost  → TNC2 NORD><LINK TheFirmware hostmode (JHOST1/JHOST0)
//!   tfpcx           → switch: up to three KISS or WA8DED plugins
//!   kiss            → TAPR KISS over serial or TCP (FEND/FESC)
//! ```

pub mod ax25;
pub mod baycom;
mod drivers;
mod known_hosts;
#[allow(dead_code)] // Hostmode pacing kept beside WA8DED. The switch calls wa8ded::run.
mod host_delivery;
mod hostmode;
mod kiss_ini;
mod monitor_filter;
mod payload;
mod serial;
mod session;
mod session_service;
mod tcp;
mod timing;
mod wa8ded_ini;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::config::{HostProtocol, SerialConfig, TncConfig};

pub use ax25::{Ax25Frame, Ax25FrameType};
pub use drivers::baycom::detect_ports as detect_baycom_ports;
pub use hostmode::HostCode;
pub use kiss_ini::{
    apply_configured_paclen, load_paclen as load_kiss_paclen, load_params as load_kiss_params,
    open_lines as kiss_param_lines, parse_esc as parse_kiss_esc, payload_len as kiss_payload_len,
    resolve_kiss_ini, store_paclen as store_kiss_paclen, store_param as store_kiss_param, EscKiss,
};
pub use monitor_filter::{normalize_call, parse_heard, FrameClass, MonitorFilter, MonitorInput};
pub use payload::{
    flush_rx_line, normalize_newlines, pack_text_payloads, payload_lines, take_rx_lines,
    take_rx_lines_bytes,
};
pub use serial::{detect_serial_ports, DetectedPort};
pub use session::{ChannelConfig, SessionCommand, SessionEvent};
pub use wa8ded_ini::{
    resolve_wa8ded_ini, section_lines as wa8ded_section_lines, timing_banner_fallback,
};

// ---------------------------------------------------------------------------
// Public types (stable UI surface)
// ---------------------------------------------------------------------------

/// Synthetic buffer-display reference for KISS/TFPCX; not measured modem RAM.
/// WA8DED instead reports the firmware-native `@B` count.
pub const TNC_RAM: u32 = 600;

/// One channel's slice of the WA8DED `L` report.
///
/// `tries` is the firmware's attempt counter (KISS reports retransmissions).
/// `unack` is how many frames are out waiting for RR.
/// With a window of one, unack is 0 or 1 and stays 1 while tries climbs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBuffers {
    pub channel: u8,
    /// Peer label only. `channel`, not this callsign, identifies the row.
    pub peer: String,
    /// Receive frames not yet given to the host (`L` field b).
    pub not_disp: u32,
    /// Frames accepted but not yet transmitted (`L` field c).
    pub not_sent: u32,
    /// Firmware attempt counter (`L` field e), or KISS retransmissions.
    pub tries: u32,
    /// Frames transmitted and not yet acknowledged (`L` field d).
    pub unack: u32,
}

/// Uniform buffer line every driver sends. The GUI only paints this.
///
/// `None` leaves the previous value alone (a WA8DED `L` answer and an `@B`
/// answer arrive as separate polls). `replace_rows` clears every channel
/// before applying `rows`, which is how a synthesized snapshot drops a peer
/// that has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferReport {
    /// Firmware-native hostmode buffer count, or synthetic KISS/TFPCX capacity.
    pub free: Option<u32>,
    /// Monitor "packets not yet displayed" (channel 0, `L` field b).
    pub not_displayed: Option<u32>,
    pub replace_rows: bool,
    pub rows: Vec<ChannelBuffers>,
}

/// Transmit queue as the driver sees it, before it becomes a [`BufferReport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TxSnap {
    pub channel: u8,
    pub peer: String,
    pub not_sent: u32,
    pub unack: u32,
    pub tries: u32,
    pub waiting_bytes: u32,
}

pub(crate) fn buffers_from_tx(snaps: &[TxSnap]) -> BufferReport {
    let waiting = snaps.iter().map(|s| s.waiting_bytes).sum();
    BufferReport {
        free: Some(TNC_RAM.saturating_sub(waiting)),
        not_displayed: Some(0),
        replace_rows: true,
        rows: snaps
            .iter()
            .map(|s| ChannelBuffers {
                channel: s.channel,
                peer: s.peer.clone(),
                not_disp: 0,
                not_sent: s.not_sent,
                tries: s.tries,
                unack: s.unack,
            })
            .collect(),
    }
}

#[allow(dead_code)] // Buffer snapshots for a hostmode poller. WA8DED reports its own.
pub(crate) fn send_buffers_if_changed(
    events: &mpsc::Sender<TncEvent>,
    snaps: &[TxSnap],
    last: &mut Option<BufferReport>,
) {
    let report = buffers_from_tx(snaps);
    if last.as_ref() == Some(&report) {
        return;
    }
    *last = Some(report.clone());
    let _ = events.blocking_send(TncEvent::Buffers(report));
}

/// Events emitted by the serial RX task toward the UI / session layer.
#[derive(Debug, Clone)]
pub enum TncEvent {
    Session(SessionEvent),
    /// Raw bytes observed on the wire (pre-decoder / debug).
    RawRx(Vec<u8>),
    /// Bytes moved on the COM port or KISS TCP link, before any decode.
    /// `tx` is host → modem (send). `false` is modem → host (receive).
    LinkIo {
        tx: bool,
    },
    /// A decoded hostmode frame or raw KISS/BayCom AX.25 (radio port + payload).
    HostFrame {
        channel: u8,
        code: HostCode,
        payload: Vec<u8>,
    },
    /// Software AX.25 TX (legacy name): KISS write or completed BayCom burst.
    /// Local completion only, not an RF acknowledgement.
    KissTx {
        /// Physical radio: KISS port nibble or BayCom port 0.
        port: u8,
        /// Unescaped AX.25 frame bytes (not KISS-wrapped).
        frame: Vec<u8>,
    },
    /// Free RAM and the channel counters (not displayed / not sent / tries / unack).
    Buffers(BufferReport),
    /// Human-readable status for the terminal pane.
    Status(String),
    /// WA8DED timing dump (`//PARameters` / open `[status]` banner).
    TimingBanner(String),
    /// Non-fatal I/O hiccup; the loop may retry.
    Warning(String),
    /// Serial link closed or unrecoverable error.
    LinkDown(String),
}

/// Commands the UI / session layer may send toward the TNC task.
#[derive(Debug, Clone)]
#[allow(dead_code)] // TerminalLine / Reinitialize reserved for upcoming UI actions.
pub enum TncCommand {
    Session(SessionCommand),
    /// Send a hostmode command, or raw AX.25 on a KISS/BayCom radio port.
    HostFrame {
        channel: u8,
        /// Command / data byte stream (e.g. `b"I NL0MSK"`).
        data: Vec<u8>,
    },
    /// Send connected/unproto information, never interpreted as a TNC command.
    HostData {
        channel: u8,
        data: Vec<u8>,
    },
    /// Issue a terminal-mode ASCII line (CR-terminated), e.g. before JHOST1.
    TerminalLine(String),
    /// One TAPR KISS parameter on the configured default radio port.
    /// The UI writes the new arg into `KISS.INI` `[params]` before sending.
    KissParam {
        cmd: u8,
        arg: u8,
    },
    /// Re-run the configured `TNCINI` sequence.
    Reinitialize,
    /// WA8DED: re-query `F` / `@T2` / `@T3` and emit [`TncEvent::TimingBanner`].
    QueryTiming,
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

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Signal drivers to abort long bring-up waits (KISS.INI pauses) so the
/// queued [`TncCommand::Shutdown`] is handled promptly.
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

pub(crate) fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Spawn the blocking serial worker; constructs **only** the configured driver.
pub fn spawn_tnc_task(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
) -> (TncChannels, tokio::task::JoinHandle<()>) {
    let (event_tx, event_rx) = mpsc::channel::<TncEvent>(256);
    // The session layer queues application packets independently of the
    // transport worker and releases them as AX.25 acknowledgements arrive.
    let (cmd_tx, cmd_rx) = mpsc::channel::<TncCommand>(256);
    let (driver_event_tx, driver_event_rx) = mpsc::channel(256);
    let (driver_cmd_tx, driver_cmd_rx) = mpsc::channel(256);

    let channels = TncChannels {
        events: event_rx,
        commands: cmd_tx,
    };

    // Fresh run — clear sticky abort from a previous in-process stop.
    SHUTDOWN.store(false, Ordering::SeqCst);

    let kind = tnc_cfg.protocol_kind();
    let session_config = tnc_cfg.clone();
    let driver_join = tokio::task::spawn_blocking(move || {
        info!(?kind, "loading modem driver");
        let result = match kind {
            HostProtocol::Baycom => {
                drivers::baycom::run(serial, tnc_cfg, driver_event_tx.clone(), driver_cmd_rx)
            }
            HostProtocol::Wa8ded => {
                drivers::wa8ded::run(serial, tnc_cfg, driver_event_tx.clone(), driver_cmd_rx)
            }
            HostProtocol::Tfpcx => {
                drivers::tfpcx::run(serial, tnc_cfg, driver_event_tx.clone(), driver_cmd_rx)
            }
            HostProtocol::Kiss => {
                drivers::kiss::run(serial, tnc_cfg, driver_event_tx.clone(), driver_cmd_rx)
            }
        };
        if let Err(err) = result {
            warn!("TNC loop exited: {err}");
            let _ = driver_event_tx.blocking_send(TncEvent::LinkDown(err.to_string()));
        }
    });
    let join = tokio::spawn(async move {
        session_service::run(
            session_config,
            cmd_rx,
            event_tx,
            driver_cmd_tx,
            driver_event_rx,
        )
        .await;
        if let Err(err) = driver_join.await {
            error!("TNC driver task failed: {err}");
        }
    });

    (channels, join)
}

struct LiveDriver {
    shutdown: mpsc::Sender<TncCommand>,
    join: JoinHandle<()>,
}

/// Modem driver slot. Empty until [`RadioLink::open`]; the pixel UI calls that
/// from the startup card, after the operator has chosen the radio.
#[derive(Clone)]
pub struct RadioLink {
    rt: Handle,
    live: Arc<Mutex<Option<LiveDriver>>>,
}

impl RadioLink {
    pub fn new(rt: Handle) -> Self {
        Self {
            rt,
            live: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_open(&self) -> bool {
        self.live.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Start the one configured driver. Call once; a second call is ignored
    /// and returns a disconnected command sender.
    pub fn open(
        &self,
        serial: SerialConfig,
        tnc_cfg: TncConfig,
    ) -> (mpsc::Receiver<TncEvent>, mpsc::Sender<TncCommand>) {
        if self.is_open() {
            warn!("radio link already open");
            let (cmd_tx, _rx) = mpsc::channel(1);
            let (_tx, events) = mpsc::channel(1);
            return (events, cmd_tx);
        }
        let _guard = self.rt.enter();
        let (channels, join) = spawn_tnc_task(serial, tnc_cfg);
        let shutdown = channels.commands.clone();
        if let Ok(mut slot) = self.live.lock() {
            *slot = Some(LiveDriver { shutdown, join });
        }
        (channels.events, channels.commands)
    }

    /// Deinit and drop the COM port or TCP socket, if one was opened.
    /// Returns whether a driver was actually running.
    pub fn shutdown(&self) -> bool {
        let live = self.live.lock().ok().and_then(|mut g| g.take());
        let Some(live) = live else {
            return false;
        };
        request_shutdown();
        let _ = self.rt.block_on(live.shutdown.send(TncCommand::Shutdown));
        if self
            .rt
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(8), live.join).await
            })
            .is_err()
        {
            error!("TNC driver did not stop within 8 s — COM may still be held");
        }
        true
    }
}
