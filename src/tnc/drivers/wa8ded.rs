//! Classic WA8DED hostmode driver — timed poll (`G0`) after `JHOST1`.
//!
//! Every couple of seconds, while the host queue is idle, the driver also
//! asks the TNC for `@B` (free buffer bytes) and `L` (not displayed, not
//! sent, unack, tries). Those replies become [`TncEvent::Buffers`].
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

use super::super::host_delivery::{HostDelivery, HostKind, Tick};
use super::super::hostmode::{
    apply_command_list, drain_frames, parse_free_buffer, parse_link_status, poll_command,
    write_host_frame, InboundFrame, LinkReport,
};
use super::super::serial::SerialLink;
use super::super::wa8ded_ini::{self, resolve_wa8ded_ini};
use super::super::{BufferReport, ChannelBuffers, TncCommand, TncError, TncEvent};

/// Pause between status sweeps so `G0` still collects traffic.
const STATUS_EVERY: Duration = Duration::from_secs(2);
/// `L` is asked on channel 0 (monitor) through this channel.
const STATUS_CHANNELS: u8 = 10;

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
    let mut delivery = HostDelivery::new();
    let mut status_poll = StatusPoll::new();
    // Ask the TNC what `M` is. The modem already filters its monitor queue;
    // the reply is only so the monitor line shows the live setting.
    let mut asked_monitor = false;
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
                let _ = run_close(&mut link, &tnc_cfg, &events);
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
                take_replies(&mut link, &mut stream, &mut delivery, &events);
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
                    "wa8ded ch{channel}: TNC did not accept the block"
                )));
            }
            None => {}
        }

        if delivery.idle() && last_poll.elapsed() >= Duration::from_millis(poll_ms) {
            let now = Instant::now();
            // A poll occupies the one outstanding host frame, so its reply is
            // not taken as the acknowledgement of operator data.
            let queued = if !asked_monitor {
                let queued = delivery.enqueue_kind(0, b"M".to_vec(), HostKind::Poll, now);
                if queued.is_some() {
                    asked_monitor = true;
                }
                queued
            } else if let Some((channel, data, kind)) = status_poll.next(now) {
                delivery.enqueue_kind(channel, data, kind, now)
            } else {
                delivery.enqueue_kind(0, poll_command().to_vec(), HostKind::Poll, now)
            };
            if let Some(out) = queued {
                if let Err(err) = write_host_frame(&mut link, out.channel, &out.data) {
                    let _ = events.blocking_send(TncEvent::Warning(format!("poll: {err}")));
                }
            }
            last_poll = now;
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
        // Optional timing queries → banner like the Python //P dump.
        emit_timing_banner(link, tnc_cfg, &path, &open_lines, events);
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
        TncCommand::TerminalLine(line) => {
            if let Err(err) = link.write_terminal(&line) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::Reinitialize => {
            let _ = run_open(link, tnc_cfg, events);
        }
        TncCommand::QueryTiming => {
            let path = resolve_wa8ded_ini(&tnc_cfg.wa8ded_ini);
            let open_lines: Vec<String> = if path.exists() {
                wa8ded_ini::section_lines(&path, "open")
                    .into_iter()
                    .map(|l| wa8ded_ini::expand_placeholders(&l, &tnc_cfg.monitor_mode))
                    .collect()
            } else {
                tnc_cfg
                    .init
                    .iter()
                    .map(|l| wa8ded_ini::expand_placeholders(l, &tnc_cfg.monitor_mode))
                    .collect()
            };
            if path.exists() {
                emit_timing_banner(link, tnc_cfg, &path, &open_lines, events);
            } else {
                let call = timing_call(tnc_cfg);
                let banner = wa8ded_ini::format_timing_banner(call, &[], &open_lines);
                publish_timing_banner(events, &banner);
            }
        }
        TncCommand::Shutdown => {
            let _ = run_close(link, tnc_cfg, events);
            let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
            return Ok(true);
        }
    }
    Ok(false)
}

fn timing_call(tnc_cfg: &TncConfig) -> &str {
    if tnc_cfg.runtime_mycall.is_empty() {
        "MYCALL"
    } else {
        tnc_cfg.runtime_mycall.as_str()
    }
}

fn publish_timing_banner(events: &mpsc::Sender<TncEvent>, banner: &str) {
    let _ = events.blocking_send(TncEvent::TimingBanner(banner.to_string()));
    for line in banner.split('\r').filter(|l| !l.is_empty()) {
        let _ = events.blocking_send(TncEvent::Status(format!("*** {line}")));
    }
}

fn emit_timing_banner(
    link: &mut SerialLink,
    tnc_cfg: &TncConfig,
    path: &std::path::Path,
    open_lines: &[String],
    events: &mpsc::Sender<TncEvent>,
) {
    let call = timing_call(tnc_cfg);
    let replies = wa8ded_ini::apply_section(
        link,
        path,
        "status",
        tnc_cfg.wa8ded_line_delay_ms,
        &tnc_cfg.monitor_mode,
        Some(events),
    )
    .unwrap_or_default();
    let banner = wa8ded_ini::format_timing_banner(call, &replies, open_lines);
    publish_timing_banner(events, &banner);
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

/// Round-robin `@B` then `L` on channels 0..=[`STATUS_CHANNELS`].
struct StatusPoll {
    /// 0 = between sweeps. 1 = `@B`. 2+ = `L` on channel `step - 2`.
    step: u8,
    next_at: Instant,
}

impl StatusPoll {
    fn new() -> Self {
        Self {
            step: 0,
            next_at: Instant::now(),
        }
    }

    /// Next status query, or `None` when this slot should be a `G0`.
    fn next(&mut self, now: Instant) -> Option<(u8, Vec<u8>, HostKind)> {
        if self.step == 0 {
            if now < self.next_at {
                return None;
            }
            self.step = 1;
        }
        let query = if self.step == 1 {
            (0, b"@B".to_vec(), HostKind::FreeBuffer)
        } else {
            (self.step - 2, b"L".to_vec(), HostKind::LinkStatus)
        };
        self.step += 1;
        if self.step > STATUS_CHANNELS + 2 {
            self.step = 0;
            self.next_at = now + STATUS_EVERY;
        }
        Some(query)
    }
}

fn take_replies(
    link: &mut SerialLink,
    stream: &mut Vec<u8>,
    delivery: &mut HostDelivery,
    events: &mpsc::Sender<TncEvent>,
) {
    let kind = delivery.inflight_kind();
    let replies = drain_frames(stream);
    if replies.is_empty() {
        return;
    }
    if let Some(out) = delivery.on_tnc_frame(Instant::now()) {
        let _ = write_host_frame(link, out.channel, &out.data);
    }
    for reply in &replies {
        dispatch_reply(events, kind, reply);
    }
}

fn dispatch_reply(events: &mpsc::Sender<TncEvent>, kind: Option<HostKind>, reply: &InboundFrame) {
    match kind {
        Some(HostKind::LinkStatus) => {
            if let Some(parsed) = parse_link_status(&reply.payload) {
                send_link(events, reply.channel, &parsed);
            }
        }
        Some(HostKind::FreeBuffer) => {
            if let Some(free) = parse_free_buffer(&reply.payload) {
                let _ = events.blocking_send(TncEvent::Buffers(BufferReport {
                    free: Some(free),
                    not_displayed: None,
                    replace_rows: false,
                    rows: Vec::new(),
                }));
            }
        }
        _ => {
            let _ = events.blocking_send(TncEvent::HostFrame {
                channel: reply.channel,
                code: reply.code,
                payload: reply.payload.clone(),
            });
        }
    }
}

fn send_link(events: &mpsc::Sender<TncEvent>, channel: u8, parsed: &LinkReport) {
    if channel == 0 {
        let _ = events.blocking_send(TncEvent::Buffers(BufferReport {
            free: None,
            not_displayed: Some(parsed.not_disp),
            replace_rows: false,
            rows: Vec::new(),
        }));
        return;
    }
    if !parsed.full {
        return;
    }
    let _ = events.blocking_send(TncEvent::Buffers(BufferReport {
        free: None,
        not_displayed: None,
        replace_rows: false,
        rows: vec![ChannelBuffers {
            channel,
            peer: String::new(),
            not_disp: parsed.not_disp,
            not_sent: parsed.not_sent,
            tries: parsed.tries,
            unack: parsed.unack,
        }],
    }));
}
