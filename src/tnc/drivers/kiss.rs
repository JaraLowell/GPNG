//! TAPR KISS modem driver — `FEND` / `FESC` framing over serial.
//!
//! On bring-up, applies a KISSINIT-style section from `KISS.INI` (see
//! `[tnc] kiss_ini` / `kiss_section`) so radios like the TM-D700 enter KISS
//! before we listen for APRS frames.
//!
//! RX frames become [`TncEvent::HostFrame`] with `channel` = KISS port and
//! `code` = [`HostCode::Data`] so existing UI pads keep working. TX
//! [`TncCommand::HostFrame`] encodes a KISS data frame on that port.

use std::time::Duration;

use tokio::sync::mpsc;
use tracing::debug;

use crate::config::{SerialConfig, TncConfig};

use super::super::hostmode::HostCode;
use super::super::kiss_ini::{self, resolve_kiss_ini};
use super::super::serial::SerialLink;
use super::super::{TncCommand, TncError, TncEvent};

const FEND: u8 = 0xC0;
const FESC: u8 = 0xDB;
const TFEND: u8 = 0xDC;
const TFESC: u8 = 0xDD;
const KISS_EXIT: [u8; 3] = [FEND, 0xFF, FEND];

pub fn run(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let mut link = SerialLink::open(&serial)?;
    let _ = events.blocking_send(TncEvent::Status(format!(
        "kiss driver on {} ({} baud)",
        link.port_name, link.baud
    )));

    if let Err(err) = run_kiss_init(&mut link, &tnc_cfg, &events) {
        let _ = events.blocking_send(TncEvent::Warning(format!("KISS.INI: {err}")));
    }
    // Leftover post-RESTART noise only — init TX/RX already logged to Monitor.
    link.flush_input();
    let _ = events.blocking_send(TncEvent::Status(format!(
        "serial OPEN {} @ {} — KISS listen (radio: TNC→PACKET12, STA/CON should have blinked)",
        link.port_name, link.baud
    )));

    let mut rx_buf = [0u8; 512];
    let mut stream = Vec::<u8>::new();

    loop {
        while let Ok(cmd) = commands.try_recv() {
            if handle_command(&mut link, &tnc_cfg, &events, cmd)? {
                return Ok(());
            }
        }

        match commands.try_recv() {
            Err(mpsc::error::TryRecvError::Disconnected) => {
                let _ = link.write_all(&KISS_EXIT);
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
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(n) => {
                let chunk = &rx_buf[..n];
                let _ = events.blocking_send(TncEvent::RawRx(chunk.to_vec()));
                stream.extend_from_slice(chunk);
                drain_kiss_frames(&mut stream, &events);
            }
            Err(err) => {
                let _ = events.blocking_send(TncEvent::LinkDown(err.to_string()));
                return Err(err);
            }
        }
    }
}

fn run_kiss_init(
    link: &mut SerialLink,
    tnc_cfg: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let path = resolve_kiss_ini(&tnc_cfg.kiss_ini);
    let _ = events.blocking_send(TncEvent::Status(format!(
        "kiss init [{}] from {}",
        tnc_cfg.kiss_section,
        path.display()
    )));
    kiss_ini::apply_section(
        link,
        &path,
        &tnc_cfg.kiss_section,
        tnc_cfg.kiss_char_delay_ms,
        tnc_cfg.kiss_line_delay_ms,
        tnc_cfg.kiss_exit_delay_ms,
        Some(events),
    )?;
    let _ = events.blocking_send(TncEvent::Status(
        "kiss init done — listening for frames".into(),
    ));
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
            if let Err(err) = write_kiss_data(link, channel, &data) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::TerminalLine(line) => {
            if let Err(err) = link.write_terminal(&line) {
                let _ = events.blocking_send(TncEvent::Warning(err.to_string()));
            }
        }
        TncCommand::Reinitialize => {
            if let Err(err) = run_kiss_init(link, tnc_cfg, events) {
                let _ = events.blocking_send(TncEvent::Warning(format!("KISS.INI: {err}")));
            }
        }
        TncCommand::Shutdown => {
            // KISS "return" command (C0 FF C0) — radio leaves KISS for command mode.
            if let Err(err) = link.write_all(&KISS_EXIT) {
                let _ = events.blocking_send(TncEvent::Warning(format!("KISS exit: {err}")));
            } else {
                let _ = events.blocking_send(TncEvent::Status("kiss exit sent (C0 FF C0)".into()));
            }
            std::thread::sleep(Duration::from_millis(300));
            let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
            return Ok(true);
        }
    }
    Ok(false)
}

fn write_kiss_data(link: &mut SerialLink, port: u8, payload: &[u8]) -> Result<(), TncError> {
    let mut frame = Vec::with_capacity(payload.len() * 2 + 4);
    frame.push(FEND);
    // Data frame type = (port << 4) | 0x00
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

/// Emit CR/LF-terminated plain-text lines (radio still in TNC/APRS text mode,
/// e.g. `DB0ET>APRS,WIDE1-1:...`) as Monitor lines. Returns bytes consumed.
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
        stream.drain(..=end);

        if raw.is_empty() {
            continue;
        }
        let Some((port, payload)) = unescape_kiss(&raw) else {
            continue;
        };
        let _ = events.blocking_send(TncEvent::HostFrame {
            channel: port,
            code: HostCode::Data,
            payload,
        });
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
        drain_kiss_frames(&mut stream, &tx);
        match rx.try_recv().unwrap() {
            TncEvent::HostFrame { channel, code, payload } => {
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
        drain_kiss_frames(&mut stream, &tx);
        match rx.try_recv().unwrap() {
            TncEvent::HostFrame { payload, .. } => assert_eq!(payload, vec![b'A', FEND]),
            other => panic!("unexpected {other:?}"),
        }
    }
}
