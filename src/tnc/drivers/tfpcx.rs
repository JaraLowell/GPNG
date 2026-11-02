//! TFPCX soft-TNC driver — WA8DED-compatible hostmode framing.
//!
//! TFPCX already presents a hostmode interface to GP, so bring-up skips a hard
//! dependency on hardware `JHOST1` echo. Timed `G0` poll and TNCINI/TNCDEI
//! match the WA8DED path; shutdown uses deinit only (no required `JHOST0`).

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::{SerialConfig, TncConfig};

use super::super::hostmode::{
    apply_command_list, drain_frames, poll_g0, write_host_frame,
};
use super::super::serial::SerialLink;
use super::super::{TncCommand, TncError, TncEvent};

pub fn run(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let poll_ms = serial.poll_interval_ms();
    let mut link = SerialLink::open(&serial)?;
    let _ = events.blocking_send(TncEvent::Status(format!(
        "tfpcx driver on {} ({} baud, poll {} ms)",
        link.port_name, link.baud, poll_ms
    )));

    // Soft-TNC: brief settle, optional best-effort JHOST1, then TNCINI.
    std::thread::sleep(Duration::from_millis(100));
    let _ = link.write_terminal("JHOST1");

    if let Err(err) = apply_command_list(&mut link, &tnc_cfg.init) {
        let _ = events.blocking_send(TncEvent::Warning(format!("TNCINI failed: {err}")));
    } else {
        let _ = events.blocking_send(TncEvent::Status(
            "tfpcx hostmode active — TNCINI applied".into(),
        ));
    }

    let mut rx_buf = [0u8; 512];
    let mut stream = Vec::<u8>::new();
    let mut last_poll = Instant::now()
        .checked_sub(Duration::from_millis(poll_ms))
        .unwrap_or_else(Instant::now);

    loop {
        while let Ok(cmd) = commands.try_recv() {
            if handle_command(&mut link, &tnc_cfg, &events, cmd)? {
                return Ok(());
            }
        }

        match commands.try_recv() {
            Err(mpsc::error::TryRecvError::Disconnected) => {
                let _ = apply_command_list(&mut link, &tnc_cfg.deinit);
                return Ok(());
            }
            Ok(cmd) => {
                if handle_command(&mut link, &tnc_cfg, &events, cmd)? {
                    return Ok(());
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        match link.try_read(&mut rx_buf) {
            Ok(0) => {}
            Ok(n) => {
                let chunk = &rx_buf[..n];
                let _ = events.blocking_send(TncEvent::RawRx(chunk.to_vec()));
                stream.extend_from_slice(chunk);
                drain_frames(&mut stream, &events);
            }
            Err(err) => {
                let _ = events.blocking_send(TncEvent::LinkDown(err.to_string()));
                return Err(err);
            }
        }

        if last_poll.elapsed() >= Duration::from_millis(poll_ms) {
            if let Err(err) = poll_g0(&mut link) {
                let _ = events.blocking_send(TncEvent::Warning(format!("poll G0: {err}")));
            }
            last_poll = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

fn handle_command(
    link: &mut SerialLink,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    cmd: TncCommand,
) -> Result<bool, TncError> {
    match cmd {
        TncCommand::HostFrame { channel, data } => {
            if let Err(err) = write_host_frame(link, channel, &data) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::TerminalLine(line) => {
            if let Err(err) = link.write_terminal(&line) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::Reinitialize => {
            let _ = apply_command_list(link, &tnc_cfg.init);
        }
        TncCommand::Shutdown => {
            let _ = apply_command_list(link, &tnc_cfg.deinit);
            let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
            return Ok(true);
        }
    }
    Ok(false)
}
