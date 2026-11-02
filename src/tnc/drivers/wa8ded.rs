//! Classic WA8DED hostmode driver — timed poll (`G0`) after `JHOST1`.
//!
//! Bring-up:
//!   1. terminal `JHOST1`
//!   2. `WA8DED.INI` `[open]` (or Config.toml `init` fallback)
//!   3. optional `[status]` queries → timing banner on Monitor
//! Shutdown:
//!   `WA8DED.INI` `[close]` (MN / U1Away! / K2 / JHOST0) or Config `deinit`

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::info;

use crate::config::{SerialConfig, TncConfig};

use super::super::hostmode::{apply_command_list, drain_frames, poll_g0, write_host_frame};
use super::super::serial::SerialLink;
use super::super::wa8ded_ini::{self, resolve_wa8ded_ini};
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
        "wa8ded driver on {} ({} baud, poll {} ms)",
        link.port_name, link.baud, poll_ms
    )));

    if let Err(err) = enter_hostmode(&mut link) {
        let _ = events.blocking_send(TncEvent::Warning(format!(
            "JHOST1 failed (TNC may already be in hostmode?): {err}"
        )));
    }

    if let Err(err) = run_open(&mut link, &tnc_cfg, &events) {
        let _ = events.blocking_send(TncEvent::Warning(format!("WA8DED open: {err}")));
    } else {
        let _ = events.blocking_send(TncEvent::Status(
            "wa8ded hostmode active — open sequence done".into(),
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
                let _ = run_close(&mut link, &tnc_cfg, &events);
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

fn run_open(
    link: &mut SerialLink,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let path = resolve_wa8ded_ini(&tnc_cfg.wa8ded_ini);
    if path.exists() {
        let open_lines: Vec<String> = wa8ded_ini::section_lines(&path, "open")
            .into_iter()
            .map(|l| wa8ded_ini::expand_placeholders(&l, &tnc_cfg.monitor_mode))
            .collect();
        wa8ded_ini::apply_section(
            link,
            &path,
            "open",
            tnc_cfg.wa8ded_line_delay_ms,
            &tnc_cfg.monitor_mode,
            Some(events),
        )?;
        // Optional timing queries → banner like the Python debug block.
        if let Ok(replies) = wa8ded_ini::apply_section(
            link,
            &path,
            "status",
            tnc_cfg.wa8ded_line_delay_ms,
            &tnc_cfg.monitor_mode,
            Some(events),
        ) {
            let call = if tnc_cfg.runtime_mycall.is_empty() {
                "MYCALL"
            } else {
                tnc_cfg.runtime_mycall.as_str()
            };
            let banner = wa8ded_ini::format_timing_banner(call, &replies, &open_lines);
            for line in banner.lines() {
                let _ = events.blocking_send(TncEvent::Status(format!("*** {line}")));
            }
        }
        Ok(())
    } else {
        info!(?path, "WA8DED.INI missing — using Config.toml init[]");
        let cmds: Vec<String> = tnc_cfg
            .init
            .iter()
            .map(|c| wa8ded_ini::expand_placeholders(c, &tnc_cfg.monitor_mode))
            .collect();
        apply_command_list(link, &cmds)
    }
}

fn run_close(
    link: &mut SerialLink,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let path = resolve_wa8ded_ini(&tnc_cfg.wa8ded_ini);
    if path.exists() {
        // [close] includes JHOST0 — do not double-leave afterward.
        wa8ded_ini::apply_section(
            link,
            &path,
            "close",
            tnc_cfg.wa8ded_line_delay_ms,
            &tnc_cfg.monitor_mode,
            Some(events),
        )?;
    } else {
        apply_command_list(link, &tnc_cfg.deinit)?;
        leave_hostmode(link)?;
    }
    Ok(())
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
            let _ = run_open(link, tnc_cfg, events);
        }
        TncCommand::Shutdown => {
            let _ = run_close(link, tnc_cfg, events);
            let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
            return Ok(true);
        }
    }
    Ok(false)
}

fn enter_hostmode(link: &mut SerialLink) -> Result<(), TncError> {
    info!("entering hostmode (JHOST1)");
    link.write_terminal("JHOST1")?;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut buf = [0u8; 64];
    let mut acc = Vec::new();
    while Instant::now() < deadline {
        match link.try_read(&mut buf) {
            Ok(0) => std::thread::sleep(Duration::from_millis(20)),
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&acc);
                if s.to_ascii_uppercase().contains("JHOST1") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

fn leave_hostmode(link: &mut SerialLink) -> Result<(), TncError> {
    info!("leaving hostmode (JHOST0)");
    let _ = write_host_frame(link, 0, b"JHOST0");
    link.write_terminal("JHOST0")?;
    Ok(())
}
