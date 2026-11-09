//! TFPCX soft-TNC driver — WA8DED-compatible hostmode framing.
//!
//! TFPCX already presents a hostmode interface to GP, so bring-up skips a hard
//! dependency on hardware `JHOST1` echo. Timed `G0` poll and TNCINI/TNCDEI
//! match the WA8DED path; shutdown uses deinit only (no required `JHOST0`).
//! Buffer counters are built here from the host queue (TFPCX does not send
//! the WA8DED `L` / `@B` report).
//!
//! Timing (`P` / `W` / `T` / `O` / `F` from `[tnc] init`) is sent as hostmode
//! text — TFPCX is the soft modem, so CSMA/TXDELAY live in that process, not
//! as KISS param frames. `//P` reports the configured values (no live `@T2`).

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::{SerialConfig, TncConfig};

use super::super::host_delivery::{HostDelivery, HostKind, Tick};
use super::super::hostmode::{apply_command_list, drain_frames, poll_command, write_host_frame};
use super::super::serial::SerialLink;
use super::super::timing::timing_from_tnc_config;
use super::super::wa8ded_ini;
use super::super::{send_buffers_if_changed, BufferReport, TncCommand, TncError, TncEvent};

pub fn run(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let poll_ms = serial.poll_interval_ms();
    let mut link = SerialLink::open(&serial)?;
    link.watch(events.clone());
    let _ = events.blocking_send(TncEvent::Status(format!(
        "tfpcx driver on {} ({} baud, poll {} ms)",
        link.port_name, link.baud, poll_ms
    )));

    // Soft-TNC: brief settle, optional best-effort JHOST1, then TNCINI.
    std::thread::sleep(Duration::from_millis(100));
    let _ = link.write_terminal("JHOST1");

    let init_cmds: Vec<String> = tnc_cfg
        .init
        .iter()
        .map(|c| wa8ded_ini::expand_placeholders(c, &tnc_cfg.monitor_mode))
        .collect();
    if let Err(err) = apply_command_list(&mut link, &init_cmds) {
        let _ = events.blocking_send(TncEvent::Warning(format!("TNCINI failed: {err}")));
    } else {
        let _ = events.blocking_send(TncEvent::Status(
            "tfpcx hostmode active — TNCINI applied".into(),
        ));
    }
    publish_tfpcx_timing_banner(&tnc_cfg, &events);

    let mut rx_buf = [0u8; 512];
    let mut stream = Vec::<u8>::new();
    let mut delivery = HostDelivery::new();
    let mut last_buffers: Option<BufferReport> = None;
    let mut last_poll = Instant::now()
        .checked_sub(Duration::from_millis(poll_ms))
        .unwrap_or_else(Instant::now);

    loop {
        while let Ok(cmd) = commands.try_recv() {
            if handle_command(&mut link, &mut delivery, &tnc_cfg, &events, cmd)? {
                return Ok(());
            }
        }

        match commands.try_recv() {
            Err(mpsc::error::TryRecvError::Disconnected) => {
                let _ = apply_command_list(&mut link, &tnc_cfg.deinit);
                return Ok(());
            }
            Ok(cmd) => {
                if handle_command(&mut link, &mut delivery, &tnc_cfg, &events, cmd)? {
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
                let replies = drain_frames(&mut stream);
                if !replies.is_empty() {
                    if let Some(out) = delivery.on_tnc_frame(Instant::now()) {
                        let _ = write_host_frame(&mut link, out.channel, &out.data);
                    }
                    for reply in replies {
                        let _ = events.blocking_send(TncEvent::HostFrame {
                            channel: reply.channel,
                            code: reply.code,
                            payload: reply.payload,
                        });
                    }
                }
            }
            Err(err) => {
                let _ = events.blocking_send(TncEvent::LinkDown(err.to_string()));
                return Err(err);
            }
        }

        match delivery.tick(Instant::now()) {
            Some(Tick::Resend(out)) => {
                let _ = write_host_frame(&mut link, out.channel, &out.data);
            }
            Some(Tick::GaveUp { channel }) => {
                let _ = events.blocking_send(TncEvent::Warning(format!(
                    "tfpcx ch{channel}: TNC did not accept the block"
                )));
            }
            None => {}
        }

        send_buffers_if_changed(&events, &delivery.tx_snapshots(), &mut last_buffers);

        if delivery.idle() && last_poll.elapsed() >= Duration::from_millis(poll_ms) {
            if let Some(out) =
                delivery.enqueue_kind(0, poll_command().to_vec(), HostKind::Poll, Instant::now())
            {
                if let Err(err) = write_host_frame(&mut link, out.channel, &out.data) {
                    let _ = events.blocking_send(TncEvent::Warning(format!("poll G0: {err}")));
                }
            }
            last_poll = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

fn handle_command(
    link: &mut SerialLink,
    delivery: &mut HostDelivery,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    cmd: TncCommand,
) -> Result<bool, TncError> {
    match cmd {
        TncCommand::LocalAck { .. } => {}
        TncCommand::HostFrame { channel, data } => {
            if let Some(out) = delivery.enqueue(channel, data, Instant::now()) {
                if let Err(err) = write_host_frame(link, out.channel, &out.data) {
                    let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
                }
            }
        }
        TncCommand::KissParam { .. } => {}
        TncCommand::TerminalLine(line) => {
            if let Err(err) = link.write_terminal(&line) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::Reinitialize => {
            let cmds: Vec<String> = tnc_cfg
                .init
                .iter()
                .map(|c| wa8ded_ini::expand_placeholders(c, &tnc_cfg.monitor_mode))
                .collect();
            let _ = apply_command_list(link, &cmds);
            publish_tfpcx_timing_banner(tnc_cfg, events);
        }
        TncCommand::QueryTiming => {
            publish_tfpcx_timing_banner(tnc_cfg, events);
        }
        TncCommand::Shutdown => {
            let _ = apply_command_list(link, &tnc_cfg.deinit);
            let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
            return Ok(true);
        }
    }
    Ok(false)
}

fn publish_tfpcx_timing_banner(tnc_cfg: &TncConfig, events: &mpsc::Sender<TncEvent>) {
    let timing = timing_from_tnc_config(tnc_cfg);
    let call = if tnc_cfg.runtime_mycall.is_empty() {
        "MYCALL"
    } else {
        tnc_cfg.runtime_mycall.as_str()
    };
    let banner = wa8ded_ini::format_timing_banner(
        call,
        &timing.as_status_replies(),
        &timing.as_open_lines(),
    );
    let _ = events.blocking_send(TncEvent::TimingBanner(banner.clone()));
    for line in banner.split('\r').filter(|l| !l.is_empty()) {
        let _ = events.blocking_send(TncEvent::Status(format!("*** {line}")));
    }
}
