//! TAPR KISS modem driver — `FEND` / `FESC` framing.
//!
//! **Serial** (`kiss_transport = "serial"`): Kenwood etc. — applies `KISS.INI`,
//! DTR pulse, and exit sequence. After 12 quiet hours the session layer asks
//! for a refresh: leave KISS, drop the COM handle, open it again, and rerun
//! `KISS.INI`. TCP is left alone.
//!
//! **TCP** (`kiss_transport = "tcp"`): Soundmodem / Direwolf — raw KISS on a
//! socket; no radio command-mode init (peer is already in KISS).
//!
//! After bring-up, `KISS.INI` `[params]` is sent as TAPR command frames
//! (`FEND | cmd | arg | FEND`). Esc `T` / `P` / `W` (the letter on each line)
//! updates that section and sends the one frame. The session layer uses
//! configured Frack, immediate acknowledgements, T3=300 s, and window=1.
//! A file with no `[params]` still falls back to
//! `T`/`P`/`W` from `WA8DED.INI` `[open]` or `[tnc] init`.
//!
//! AX.25 state, retransmissions and channel counters belong to the session
//! layer. This driver only frames and transports packets on a radio port.

use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{SerialConfig, TncConfig};

use super::super::hostmode::HostCode;
use super::super::kiss_ini::{self, resolve_kiss_ini, KissParam};
use super::super::serial::SerialLink;
use super::super::tcp::TcpKissLink;
use super::super::timing::timing_from_tnc_config;
use super::super::wa8ded_ini;
use super::super::{TncCommand, TncError, TncEvent};

const FEND: u8 = 0xC0;
const FESC: u8 = 0xDB;
const TFEND: u8 = 0xDC;
const TFESC: u8 = 0xDD;

enum KissLink {
    Serial(SerialLink),
    Tcp(TcpKissLink),
    /// Holds no OS handle, so a new `open` can take the same COM name.
    Idle,
}

impl KissLink {
    fn open(serial: &SerialConfig, tnc_cfg: &TncConfig) -> Result<Self, TncError> {
        if tnc_cfg.kiss_use_tcp() {
            Ok(Self::Tcp(TcpKissLink::connect(
                &tnc_cfg.kiss_tcp_host,
                tnc_cfg.kiss_tcp_port,
            )?))
        } else {
            Ok(Self::Serial(SerialLink::open(serial)?))
        }
    }

    fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    fn watch(&mut self, events: mpsc::Sender<TncEvent>) {
        match self {
            Self::Serial(s) => s.watch(events),
            Self::Tcp(t) => t.watch(events),
            Self::Idle => {}
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Serial(s) => format!("{} @ {}", s.port_name, s.baud),
            Self::Tcp(t) => t.addr.clone(),
            Self::Idle => "closed".into(),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), TncError> {
        match self {
            Self::Serial(s) => s.write_all(buf),
            Self::Tcp(t) => t.write_all(buf),
            Self::Idle => Err(port_closed()),
        }
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, TncError> {
        match self {
            Self::Serial(s) => s.try_read(buf),
            Self::Tcp(t) => t.try_read(buf),
            Self::Idle => Err(port_closed()),
        }
    }

    fn flush_input(&mut self) {
        match self {
            Self::Serial(s) => s.flush_input(),
            Self::Tcp(t) => t.flush_input(),
            Self::Idle => {}
        }
    }

    fn write_terminal(&mut self, line: &str) -> Result<(), TncError> {
        match self {
            Self::Serial(s) => s.write_terminal(line),
            Self::Tcp(t) => {
                let mut buf = line.as_bytes().to_vec();
                if !buf.ends_with(&[b'\r']) {
                    buf.push(b'\r');
                }
                t.write_all(&buf)
            }
            Self::Idle => Err(port_closed()),
        }
    }

    fn leave_kiss_for_commands(&mut self) {
        if let Self::Serial(s) = self {
            s.pulse_dtr();
            let _ = s.kiss_exit();
            std::thread::sleep(Duration::from_millis(1500));
            s.flush_input();
        }
    }

    fn prepare_close(&mut self) {
        if let Self::Serial(s) = self {
            s.prepare_close();
        }
    }

    fn kiss_exit(&mut self) -> Result<(), TncError> {
        match self {
            Self::Serial(s) => s.kiss_exit(),
            Self::Tcp(_) | Self::Idle => Ok(()),
        }
    }
}

fn port_closed() -> TncError {
    TncError::Protocol("serial port is closed".into())
}

enum Drive {
    Stay,
    Stop,
    /// The COM handle was replaced. Drop any half-read KISS bytes.
    Recycled,
}

pub fn run(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let label = kiss_badge(&serial, &tnc_cfg);
    let mut failures = 0u8;
    loop {
        match run_link(
            &serial,
            &tnc_cfg,
            &events,
            &mut commands,
            &label,
            &mut failures,
        ) {
            super::super::reconnect::Attempt::Stop => return Ok(()),
            super::super::reconnect::Attempt::Again(err) => {
                if !super::super::reconnect::fail_link(
                    &events,
                    &mut commands,
                    &label,
                    &mut failures,
                    &err,
                ) {
                    return Ok(());
                }
            }
        }
    }
}

fn kiss_badge(serial: &SerialConfig, tnc_cfg: &TncConfig) -> String {
    if tnc_cfg.kiss_use_tcp() {
        return "TCP".into();
    }
    let port = serial.port.trim();
    if port.is_empty() {
        "COM".into()
    } else {
        port.to_string()
    }
}

fn run_link(
    serial: &SerialConfig,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    commands: &mut mpsc::Receiver<TncCommand>,
    label: &str,
    failures: &mut u8,
) -> super::super::reconnect::Attempt {
    let mut link = match KissLink::open(serial, tnc_cfg) {
        Ok(link) => link,
        Err(err) => return super::super::reconnect::Attempt::Again(err),
    };
    link.watch(events.clone());

    if !link.is_tcp() {
        // Soft-restart recovery: radio may still be in KISS from last run.
        link.leave_kiss_for_commands();
        if let Err(err) = run_kiss_init(&mut link, tnc_cfg) {
            let _ = events.blocking_send(TncEvent::Warning(format!("KISS.INI: {err}")));
        }
        link.flush_input();
    }
    // Modem is in KISS now (serial after INI, TCP already). Push [params].
    let timing = load_kiss_params(tnc_cfg);
    apply_configured_params(&mut link, tnc_cfg, &timing, events);

    let status = if link.is_tcp() {
        format!("TCP OPEN {} — KISS listen", link.label())
    } else {
        format!("serial OPEN {} — KISS listen", link.label())
    };
    *failures = 0;
    let _ = events.blocking_send(TncEvent::Modem {
        index: 0,
        label: label.to_string(),
        phase: crate::tnc::LinkPhase::Open,
    });
    let _ = events.blocking_send(TncEvent::Status(status));
    publish_kiss_timing_banner(tnc_cfg, &timing, events);

    let mut rx_buf = [0u8; 512];
    let mut stream = Vec::<u8>::new();

    loop {
        while let Ok(cmd) = commands.try_recv() {
            match accept_command(&mut link, serial, tnc_cfg, events, &mut stream, cmd) {
                Ok(true) => return super::super::reconnect::Attempt::Stop,
                Ok(false) => {}
                Err(err) => return super::super::reconnect::Attempt::Again(err),
            }
        }

        match commands.try_recv() {
            Err(mpsc::error::TryRecvError::Disconnected) => {
                leave_kiss_and_close(&mut link, tnc_cfg, events);
                return super::super::reconnect::Attempt::Stop;
            }
            Ok(cmd) => match accept_command(&mut link, serial, tnc_cfg, events, &mut stream, cmd) {
                Ok(true) => return super::super::reconnect::Attempt::Stop,
                Ok(false) => {}
                Err(err) => return super::super::reconnect::Attempt::Again(err),
            },
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        match link.try_read(&mut rx_buf) {
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(n) => {
                let chunk = &rx_buf[..n];
                stream.extend_from_slice(chunk);
                drain_kiss_frames(&mut stream, events);
            }
            Err(err) => return super::super::reconnect::Attempt::Again(err),
        }
    }
}

fn leave_kiss_and_close(link: &mut KissLink, tnc_cfg: &TncConfig, events: &mpsc::Sender<TncEvent>) {
    if link.is_tcp() {
        link.flush_input();
        let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
        return;
    }
    release_serial(link, tnc_cfg);
    let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
}

/// KISS close section, then drop DTR. Does not tell the UI the link died.
fn release_serial(link: &mut KissLink, tnc_cfg: &TncConfig) {
    let path = resolve_kiss_ini(&tnc_cfg.kiss_ini);
    let close_sec = if tnc_cfg.kiss_close_section.is_empty() {
        "close"
    } else {
        tnc_cfg.kiss_close_section.as_str()
    };
    if path.exists() {
        if apply_kiss_ini_serial(link, tnc_cfg, &path, close_sec).is_err() {
            let _ = link.kiss_exit();
        }
    } else {
        let _ = link.kiss_exit();
    }
    std::thread::sleep(Duration::from_millis(800));
    link.prepare_close();
}

/// Close the radio, release the COM name, open it again, and rerun KISS.INI.
fn recycle_serial(
    link: &mut KissLink,
    serial: &SerialConfig,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    info!(port = %serial.port, "refreshing serial KISS");
    release_serial(link, tnc_cfg);
    // Drop the old handle before open. Windows will not share the COM name.
    drop(std::mem::replace(link, KissLink::Idle));
    *link = KissLink::open(serial, tnc_cfg)?;
    link.watch(events.clone());
    link.leave_kiss_for_commands();
    if let Err(err) = run_kiss_init(link, tnc_cfg) {
        let _ = events.blocking_send(TncEvent::Warning(format!("KISS.INI: {err}")));
    }
    link.flush_input();
    let timing = load_kiss_params(tnc_cfg);
    apply_configured_params(link, tnc_cfg, &timing, events);
    let _ = events.blocking_send(TncEvent::Modem {
        index: 0,
        label: kiss_badge(serial, tnc_cfg),
        phase: super::super::LinkPhase::Open,
    });
    let _ = events.blocking_send(TncEvent::Status(format!(
        "serial OPEN {} — serial refresh",
        link.label()
    )));
    publish_kiss_timing_banner(tnc_cfg, &timing, events);
    Ok(())
}

fn accept_command(
    link: &mut KissLink,
    serial: &SerialConfig,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    stream: &mut Vec<u8>,
    cmd: TncCommand,
) -> Result<bool, TncError> {
    let stop = match handle_command(link, serial, tnc_cfg, events, cmd)? {
        Drive::Stop => true,
        Drive::Stay => false,
        Drive::Recycled => {
            stream.clear();
            false
        }
    };
    Ok(stop)
}

fn apply_kiss_ini_serial(
    link: &mut KissLink,
    tnc_cfg: &TncConfig,
    path: &std::path::Path,
    section: &str,
) -> Result<(), TncError> {
    let KissLink::Serial(serial) = link else {
        return Ok(());
    };
    kiss_ini::apply_section(
        serial,
        path,
        section,
        tnc_cfg.kiss_char_delay_ms,
        tnc_cfg.kiss_line_delay_ms,
        500,
        &tnc_cfg.monitor_mode,
        None,
        None,
    )
}

fn run_kiss_init(link: &mut KissLink, tnc_cfg: &TncConfig) -> Result<(), TncError> {
    let KissLink::Serial(serial) = link else {
        return Ok(());
    };
    let path = resolve_kiss_ini(&tnc_cfg.kiss_ini);
    let paclen = kiss_ini::load_paclen(&path);
    kiss_ini::apply_section(
        serial,
        &path,
        &tnc_cfg.kiss_section,
        tnc_cfg.kiss_char_delay_ms,
        tnc_cfg.kiss_line_delay_ms,
        tnc_cfg.kiss_exit_delay_ms,
        &tnc_cfg.monitor_mode,
        None,
        paclen,
    )?;
    Ok(())
}

fn handle_command(
    link: &mut KissLink,
    serial: &SerialConfig,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    cmd: TncCommand,
) -> Result<Drive, TncError> {
    match cmd {
        TncCommand::HostFrame { channel, data } => {
            write_kiss_data(link, channel, &data, events)?;
        }
        TncCommand::Session(_) => {
            return Err(TncError::Protocol("unadapted session command".into()))
        }
        TncCommand::HostData { .. } => {
            return Err(TncError::Protocol(
                "hostmode data sent to KISS driver".into(),
            ))
        }
        TncCommand::TerminalLine(line) => {
            if let Err(err) = link.write_terminal(&line) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::Reinitialize => {
            if link.is_tcp() {
                *link = KissLink::open(serial, tnc_cfg)?;
                link.watch(events.clone());
                let _ = events.blocking_send(TncEvent::Status(format!(
                    "TCP OPEN {} — KISS listen",
                    link.label()
                )));
            } else {
                link.leave_kiss_for_commands();
                if let Err(err) = run_kiss_init(link, tnc_cfg) {
                    let _ = events.blocking_send(TncEvent::Warning(format!("KISS.INI: {err}")));
                }
            }
            let timing = load_kiss_params(tnc_cfg);
            apply_configured_params(link, tnc_cfg, &timing, events);
            publish_kiss_timing_banner(tnc_cfg, &timing, events);
        }
        TncCommand::RefreshSerial { .. } => {
            if !link.is_tcp() {
                recycle_serial(link, serial, tnc_cfg, events)?;
                return Ok(Drive::Recycled);
            }
        }
        TncCommand::KissParam { cmd, arg } => {
            match write_kiss_command(link, tnc_cfg.kiss_port, cmd, arg) {
                Ok(()) => {}
                Err(err) => {
                    let name = kiss_ini::cmd_label(cmd);
                    let _ = events.blocking_send(TncEvent::Warning(format!("kiss {name}: {err}")));
                }
            }
        }
        TncCommand::QueryTiming => {
            let timing = load_kiss_params(tnc_cfg);
            publish_kiss_timing_banner(tnc_cfg, &timing, events);
        }
        TncCommand::Shutdown => {
            leave_kiss_and_close(link, tnc_cfg, events);
            return Ok(Drive::Stop);
        }
    }
    Ok(Drive::Stay)
}

fn write_kiss_data(
    link: &mut KissLink,
    port: u8,
    payload: &[u8],
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let frame = encode_kiss(port, 0, payload)?;
    link.write_all(&frame)?;
    debug!(port, len = payload.len(), "tx kiss frame");
    let _ = events.blocking_send(TncEvent::KissTx {
        port,
        frame: payload.to_vec(),
    });
    Ok(())
}

/// TAPR host→TNC parameter frame: `FEND | (port<<4)|cmd | arg | FEND`.
fn write_kiss_command(link: &mut KissLink, port: u8, cmd: u8, arg: u8) -> Result<(), TncError> {
    let frame = encode_kiss(port, cmd, &[arg])?;
    link.write_all(&frame)?;
    debug!(port, cmd, arg, "tx kiss param");
    Ok(())
}

fn encode_kiss(port: u8, command: u8, payload: &[u8]) -> Result<Vec<u8>, TncError> {
    if port > 15 || command > 15 {
        return Err(TncError::Protocol(
            "KISS port and command must be 0..15".into(),
        ));
    }
    let mut frame = vec![FEND];
    for b in std::iter::once((port << 4) | command).chain(payload.iter().copied()) {
        match b {
            FEND => frame.extend_from_slice(&[FESC, TFEND]),
            FESC => frame.extend_from_slice(&[FESC, TFESC]),
            b => frame.push(b),
        }
    }
    frame.push(FEND);
    Ok(frame)
}

/// `[params]` when that section exists. Otherwise the old T/P/W lines.
fn load_kiss_params(tnc_cfg: &TncConfig) -> Vec<KissParam> {
    let path = resolve_kiss_ini(&tnc_cfg.kiss_ini);
    match kiss_ini::load_params(&path) {
        Some(params) => params,
        None => {
            warn!(
                file = %path.display(),
                "KISS.INI [params] missing — T/P/W from WA8DED.INI or [tnc] init"
            );
            timing_from_tnc_config(tnc_cfg)
                .kiss_set_commands()
                .into_iter()
                .filter_map(|(cmd, value)| KissParam::known(cmd, value))
                .collect()
        }
    }
}

fn apply_kiss_params(
    link: &mut KissLink,
    port: u8,
    params: &[KissParam],
    events: &mpsc::Sender<TncEvent>,
) {
    for param in params {
        match write_kiss_command(link, port, param.cmd, param.value) {
            Ok(()) => {
                let _ = events.blocking_send(TncEvent::Status(format!(
                    "kiss {}={} (Esc {}, FEND cmd {})",
                    param.name, param.value, param.letter, param.cmd
                )));
            }
            Err(err) => {
                let _ =
                    events.blocking_send(TncEvent::Warning(format!("kiss {}: {err}", param.name)));
            }
        }
    }
}

fn apply_configured_params(
    link: &mut KissLink,
    config: &TncConfig,
    params: &[KissParam],
    events: &mpsc::Sender<TncEvent>,
) {
    let ports: std::collections::BTreeSet<_> = std::iter::once(config.kiss_port)
        .chain(config.channel_ports.iter().copied())
        .collect();
    for port in ports {
        apply_kiss_params(link, port, params, events);
    }
}

fn publish_kiss_timing_banner(
    tnc_cfg: &TncConfig,
    params: &[KissParam],
    events: &mpsc::Sender<TncEvent>,
) {
    let call = if tnc_cfg.runtime_mycall.is_empty() {
        "MYCALL"
    } else {
        tnc_cfg.runtime_mycall.as_str()
    };
    let settings = super::super::session::Settings::from_config(tnc_cfg);
    let mut lines = kiss_ini::open_lines(params);
    lines.push("O 1".into());
    let replies = vec![
        ("F".into(), settings.t1.as_secs().to_string()),
        ("@T2".into(), "0".into()),
        ("@T3".into(), settings.t3.as_secs().to_string()),
    ];
    let banner = wa8ded_ini::format_timing_banner(call, &replies, &lines);
    let _ = events.blocking_send(TncEvent::TimingBanner(banner.clone()));
    for line in banner.split('\r').filter(|l| !l.is_empty()) {
        let _ = events.blocking_send(TncEvent::Status(format!("*** {line}")));
    }
}

/// Max unframed text kept while waiting for a line terminator.
const MAX_TEXT_PENDING: usize = 1024;

/// Emit CR/LF-terminated plain-text lines as Monitor lines. Returns bytes consumed.
fn drain_text_lines(buf: &[u8], events: &mpsc::Sender<TncEvent>) -> usize {
    let mut consumed = 0;
    while let Some(pos) = buf[consumed..]
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
    {
        let line = &buf[consumed..consumed + pos];
        consumed += pos + 1;
        if line.iter().any(|b| b.is_ascii_graphic()) {
            let _ = events.blocking_send(TncEvent::HostFrame {
                channel: 0,
                code: HostCode::Monitor,
                payload: line.to_vec(),
            });
        }
    }
    consumed
}

/// Pull complete KISS frames (`FEND … FEND`) from the stream.
fn drain_kiss_frames(stream: &mut Vec<u8>, events: &mpsc::Sender<TncEvent>) {
    loop {
        if let Some(start) = stream.iter().position(|&b| b == FEND) {
            if start > 0 {
                let mut prefix = stream[..start].to_vec();
                prefix.push(b'\r');
                drain_text_lines(&prefix, events);
                stream.drain(..start);
            }
        } else {
            let used = drain_text_lines(stream, events);
            stream.drain(..used);
            if stream.len() > MAX_TEXT_PENDING {
                stream.clear();
            }
            return;
        }
        if stream.len() < 2 {
            return;
        }
        let Some(end) = stream[1..].iter().position(|&b| b == FEND) else {
            return;
        };
        let end = end + 1;
        let raw = stream[1..end].to_vec();
        stream.drain(..end);

        if raw.is_empty() {
            continue;
        }
        let (port, payload) = match unescape_kiss(&raw) {
            Ok(Some(packet)) => packet,
            Ok(None) => continue,
            Err(reason) => {
                let _ = events.blocking_send(TncEvent::Warning(format!(
                    "malformed KISS frame: {reason}; {}B {raw:02X?}",
                    raw.len()
                )));
                continue;
            }
        };
        let _ = events.blocking_send(TncEvent::HostFrame {
            channel: port,
            code: HostCode::Data,
            payload,
        });
    }
}

fn unescape_kiss(raw: &[u8]) -> Result<Option<(u8, Vec<u8>)>, &'static str> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            FESC => {
                i += 1;
                match raw.get(i) {
                    Some(&TFEND) => out.push(FEND),
                    Some(&TFESC) => out.push(FESC),
                    _ => return Err("invalid or incomplete escape"),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    let (&type_byte, payload) = out.split_first().ok_or("missing type byte")?;
    if type_byte & 0x0F != 0 {
        return Ok(None);
    }
    Ok(Some((type_byte >> 4, payload.to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiss_multiport_type_and_parameter_bytes_are_escaped() {
        let encoded = encode_kiss(12, 0, &[FEND, FESC, 7]).unwrap();
        assert_eq!(
            encoded,
            [FEND, FESC, TFEND, FESC, TFEND, FESC, TFESC, 7, FEND]
        );
        assert_eq!(
            unescape_kiss(&encoded[1..encoded.len() - 1]),
            Ok(Some((12, vec![FEND, FESC, 7])))
        );
        assert_eq!(
            encode_kiss(2, 1, &[FEND]).unwrap(),
            [FEND, 0x21, FESC, TFEND, FEND]
        );
        assert!(encode_kiss(16, 0, b"").is_err());
    }

    #[test]
    fn kiss_shared_delimiter_and_fragmented_frames_preserve_radio_ports() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut stream = encode_kiss(12, 0, b"one").unwrap();
        stream.extend_from_slice(&encode_kiss(3, 0, b"two").unwrap()[1..]);
        let tail = stream.split_off(stream.len() - 2);
        drain_kiss_frames(&mut stream, &tx);
        assert!(
            matches!(rx.try_recv().unwrap(), TncEvent::HostFrame { channel: 12, payload, .. } if payload == b"one")
        );
        assert!(rx.try_recv().is_err());
        stream.extend_from_slice(&tail);
        drain_kiss_frames(&mut stream, &tx);
        assert!(
            matches!(rx.try_recv().unwrap(), TncEvent::HostFrame { channel: 3, payload, .. } if payload == b"two")
        );
    }

    #[test]
    fn kiss_parameter_replies_are_not_delivered_as_ax25_packets() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut stream = encode_kiss(1, 2, b"not data").unwrap();
        stream.extend_from_slice(&encode_kiss(1, 0, b"data").unwrap());
        drain_kiss_frames(&mut stream, &tx);
        assert!(
            matches!(rx.try_recv().unwrap(), TncEvent::HostFrame { channel: 1, payload, .. } if payload == b"data")
        );
        assert!(rx.try_recv().is_err());
        assert!(unescape_kiss(&[0, FESC, 0]).is_err());
    }

    #[tokio::test]
    async fn kiss_tcp_preserves_startup_rx_bursts_and_reports_each_successful_tx() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let packet = crate::tnc::ax25::build_ui("CQ", "N1TEST", &[], b"early\xDB\xC0");
        let outgoing = packet.clone();
        let incoming = packet.clone();
        let peer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut burst = Vec::new();
            for index in 0..72 {
                burst.extend_from_slice(&encode_kiss(index % 16, 0, &incoming).unwrap());
            }
            burst.extend_from_slice(&[FEND, 0, FESC, 1, FEND]);
            burst.extend_from_slice(&encode_kiss(12, 0, b"undecodable").unwrap());
            socket.write_all(&burst).unwrap();
            let mut written = Vec::new();
            socket.read_to_end(&mut written).unwrap();
            written
        });
        let config = TncConfig {
            protocol: "kiss".into(),
            kiss_transport: "tcp".into(),
            kiss_tcp_host: "127.0.0.1".into(),
            kiss_tcp_port: port,
            ..TncConfig::default()
        };
        let (events, mut rx) = mpsc::channel(8);
        let (commands, commands_rx) = mpsc::channel(8);
        let driver = tokio::task::spawn_blocking(move || {
            run(SerialConfig::default(), config, events, commands_rx)
        });
        for _ in 0..2 {
            commands
                .send(TncCommand::HostFrame {
                    channel: 12,
                    data: outgoing.clone(),
                })
                .await
                .unwrap();
        }
        let mut received = Vec::new();
        let mut transmitted = Vec::new();
        let mut warnings = Vec::new();
        let completed = tokio::time::timeout(Duration::from_secs(3), async {
            while received.len() < 73 || transmitted.len() < 2 || warnings.is_empty() {
                match rx.recv().await.unwrap() {
                    TncEvent::HostFrame {
                        channel,
                        code: HostCode::Data,
                        payload,
                    } => {
                        received.push((channel, payload));
                    }
                    TncEvent::KissTx { port, frame } => transmitted.push((port, frame)),
                    TncEvent::Warning(reason) => warnings.push(reason),
                    _ => {}
                }
            }
        })
        .await;
        commands.send(TncCommand::Shutdown).await.unwrap();
        // Drain shutdown events too: a full observer queue must not stall cleanup.
        while rx.recv().await.is_some() {}
        driver.await.unwrap().unwrap();
        let written = peer.join().unwrap();

        assert!(
            completed.is_ok(),
            "RX={}, TX={}, warnings={warnings:?}",
            received.len(),
            transmitted.len()
        );
        let mut expected: Vec<_> = (0..72).map(|index| (index % 16, packet.clone())).collect();
        expected.push((12, b"undecodable".to_vec()));
        assert_eq!(received, expected);
        assert_eq!(
            transmitted,
            vec![(12, outgoing.clone()), (12, outgoing.clone())]
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("malformed KISS frame"));
        let written_packets: Vec<_> = written
            .split(|b| *b == FEND)
            .filter(|raw| !raw.is_empty())
            .filter_map(|raw| unescape_kiss(raw).unwrap())
            .collect();
        assert_eq!(
            written_packets,
            vec![(12, outgoing.clone()), (12, outgoing)]
        );
    }
}
