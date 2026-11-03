//! TAPR KISS modem driver — `FEND` / `FESC` framing.
//!
//! **Serial** (`kiss_transport = "serial"`): Kenwood etc. — applies `KISS.INI`,
//! DTR pulse, and exit sequence.
//!
//! **TCP** (`kiss_transport = "tcp"`): Soundmodem / Direwolf — raw KISS on a
//! socket; no radio init (peer is already in KISS).
//!
//! Connected I-frames are paced here: one outstanding N(S), the next part
//! goes out only after RR or an I-frame whose N(R) acknowledges it. UI frames
//! are not acknowledged and are sent at once. Serial and TCP share this path.
//! The same queue is what the magenta bar shows: free bytes are 600 minus
//! packets still waiting, and tries / unack come from the frame in flight.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::debug;

use crate::config::{SerialConfig, TncConfig};

use super::super::hostmode::HostCode;
use super::super::kiss_delivery::{KissDelivery, Outgoing, Tick};
use super::super::kiss_ini::{self, resolve_kiss_ini};
use super::super::serial::SerialLink;
use super::super::tcp::TcpKissLink;
use super::super::{send_buffers_if_changed, BufferReport, TncCommand, TncError, TncEvent};

const FEND: u8 = 0xC0;
const FESC: u8 = 0xDB;
const TFEND: u8 = 0xDC;
const TFESC: u8 = 0xDD;

enum KissLink {
    Serial(SerialLink),
    Tcp(TcpKissLink),
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

    fn label(&self) -> String {
        match self {
            Self::Serial(s) => format!("{} @ {}", s.port_name, s.baud),
            Self::Tcp(t) => t.addr.clone(),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), TncError> {
        match self {
            Self::Serial(s) => s.write_all(buf),
            Self::Tcp(t) => t.write_all(buf),
        }
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, TncError> {
        match self {
            Self::Serial(s) => s.try_read(buf),
            Self::Tcp(t) => t.try_read(buf),
        }
    }

    fn flush_input(&mut self) {
        match self {
            Self::Serial(s) => s.flush_input(),
            Self::Tcp(t) => t.flush_input(),
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
            Self::Tcp(_) => Ok(()),
        }
    }
}

pub fn run(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let mut link = KissLink::open(&serial, &tnc_cfg)?;

    if !link.is_tcp() {
        // Soft-restart recovery: radio may still be in KISS from last run.
        link.leave_kiss_for_commands();
        if let Err(err) = run_kiss_init(&mut link, &tnc_cfg) {
            let _ = events.blocking_send(TncEvent::Warning(format!("KISS.INI: {err}")));
        }
    }
    link.flush_input();

    let status = if link.is_tcp() {
        format!("TCP OPEN {} — KISS listen", link.label())
    } else {
        format!("serial OPEN {} — KISS listen", link.label())
    };
    let _ = events.blocking_send(TncEvent::Status(status));

    let mut rx_buf = [0u8; 512];
    let mut stream = Vec::<u8>::new();
    let mut delivery = KissDelivery::new();
    let mut last_buffers: Option<BufferReport> = None;

    loop {
        while let Ok(cmd) = commands.try_recv() {
            if handle_command(&mut link, &mut delivery, &serial, &tnc_cfg, &events, cmd)? {
                return Ok(());
            }
        }

        match commands.try_recv() {
            Err(mpsc::error::TryRecvError::Disconnected) => {
                leave_kiss_and_close(&mut link, &tnc_cfg, &events);
                return Ok(());
            }
            Ok(cmd) => {
                if handle_command(&mut link, &mut delivery, &serial, &tnc_cfg, &events, cmd)? {
                    return Ok(());
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        match link.try_read(&mut rx_buf) {
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(n) => {
                let chunk = &rx_buf[..n];
                stream.extend_from_slice(chunk);
                let send = drain_kiss_frames(&mut stream, &mut delivery, &events);
                for out in send {
                    let _ = write_kiss_data(&mut link, out.port, &out.frame);
                }
            }
            Err(err) => {
                let _ = events.blocking_send(TncEvent::LinkDown(err.to_string()));
                return Err(err);
            }
        }

        match delivery.tick(Instant::now()) {
            Some(Tick::Resend(out)) => {
                let _ = write_kiss_data(&mut link, out.port, &out.frame);
            }
            Some(Tick::GaveUp { port }) => {
                let _ = events.blocking_send(TncEvent::Warning(format!(
                    "KISS port {port}: I-frame not acknowledged after {tries} tries",
                    tries = super::super::kiss_delivery::N2
                )));
            }
            None => {}
        }

        send_buffers_if_changed(&events, &delivery.tx_snapshots(), &mut last_buffers);
    }
}

fn leave_kiss_and_close(
    link: &mut KissLink,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) {
    if link.is_tcp() {
        link.flush_input();
        let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
        return;
    }

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
    let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
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
    )
}

fn run_kiss_init(link: &mut KissLink, tnc_cfg: &TncConfig) -> Result<(), TncError> {
    let KissLink::Serial(serial) = link else {
        return Ok(());
    };
    let path = resolve_kiss_ini(&tnc_cfg.kiss_ini);
    kiss_ini::apply_section(
        serial,
        &path,
        &tnc_cfg.kiss_section,
        tnc_cfg.kiss_char_delay_ms,
        tnc_cfg.kiss_line_delay_ms,
        tnc_cfg.kiss_exit_delay_ms,
        &tnc_cfg.monitor_mode,
        None,
    )?;
    Ok(())
}

fn handle_command(
    link: &mut KissLink,
    delivery: &mut KissDelivery,
    serial: &SerialConfig,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    cmd: TncCommand,
) -> Result<bool, TncError> {
    match cmd {
        TncCommand::HostFrame { channel, data } => {
            if let Some(out) = delivery.push(channel, data, Instant::now()) {
                if let Err(err) = write_kiss_data(link, out.port, &out.frame) {
                    let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
                }
            }
        }
        TncCommand::LocalAck { port, frame } => {
            if let Err(err) = write_kiss_data(link, port, &frame) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
            if let Some(out) = delivery.on_rx(&frame, Instant::now()) {
                if let Err(err) = write_kiss_data(link, out.port, &out.frame) {
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
            if link.is_tcp() {
                *link = KissLink::open(serial, tnc_cfg)?;
                link.flush_input();
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
        }
        TncCommand::Shutdown => {
            leave_kiss_and_close(link, tnc_cfg, events);
            return Ok(true);
        }
    }
    Ok(false)
}

fn write_kiss_data(link: &mut KissLink, port: u8, payload: &[u8]) -> Result<(), TncError> {
    let mut frame = Vec::with_capacity(payload.len() * 2 + 4);
    frame.push(FEND);
    frame.push(port << 4);
    for &b in payload {
        match b {
            FEND => {
                frame.push(FESC);
                frame.push(TFEND);
            }
            FESC => {
                frame.push(FESC);
                frame.push(TFESC);
            }
            other => frame.push(other),
        }
    }
    frame.push(FEND);
    link.write_all(&frame)?;
    debug!(port, len = payload.len(), "tx kiss frame");
    Ok(())
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
/// Returned frames are I-frames released by an acknowledgement.
fn drain_kiss_frames(
    stream: &mut Vec<u8>,
    delivery: &mut KissDelivery,
    events: &mpsc::Sender<TncEvent>,
) -> Vec<Outgoing> {
    let mut released = Vec::new();
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
            return released;
        }
        if stream.len() < 2 {
            return released;
        }
        let Some(end) = stream[1..].iter().position(|&b| b == FEND) else {
            return released;
        };
        let end = end + 1;
        let raw = stream[1..end].to_vec();
        stream.drain(..=end);

        if raw.is_empty() {
            continue;
        }
        let Some((port, payload)) = unescape_kiss(&raw) else {
            continue;
        };
        let next = delivery.on_rx(&payload, Instant::now());
        let _ = events.blocking_send(TncEvent::HostFrame {
            channel: port,
            code: HostCode::Data,
            payload,
        });
        if let Some(out) = next {
            released.push(out);
        }
    }
}

fn unescape_kiss(raw: &[u8]) -> Option<(u8, Vec<u8>)> {
    let type_byte = *raw.first()?;
    let port = type_byte >> 4;
    let mut out = Vec::with_capacity(raw.len().saturating_sub(1));
    let mut i = 1;
    while i < raw.len() {
        match raw[i] {
            FESC => {
                i += 1;
                match raw.get(i) {
                    Some(&TFEND) => out.push(FEND),
                    Some(&TFESC) => out.push(FESC),
                    _ => return None,
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    Some((port, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_lines_become_monitor_frames() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut stream = b"DB0ET>U3RTS5,WIDE1-1:'}Xzl\rDB0".to_vec();
        let mut delivery = KissDelivery::new();
        drain_kiss_frames(&mut stream, &mut delivery, &tx);
        match rx.try_recv().unwrap() {
            TncEvent::HostFrame {
                channel,
                code,
                payload,
            } => {
                assert_eq!(channel, 0);
                assert!(matches!(code, HostCode::Monitor));
                assert_eq!(payload, b"DB0ET>U3RTS5,WIDE1-1:'}Xzl");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(stream, b"DB0");
    }

    #[test]
    fn kiss_frame_still_decodes() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut stream = vec![FEND, 0x00, b'A', FESC, TFEND, FEND];
        let mut delivery = KissDelivery::new();
        drain_kiss_frames(&mut stream, &mut delivery, &tx);
        match rx.try_recv().unwrap() {
            TncEvent::HostFrame { payload, .. } => assert_eq!(payload, vec![b'A', FEND]),
            other => panic!("unexpected {other:?}"),
        }
    }
}
