//! Raw TCP byte pipe for soft TNCs (Soundmodem / Direwolf KISS TCP).
//!
//! Same TAPR KISS framing as serial; the socket already speaks KISS — no
//! KISS.INI / DTR / exit sequence.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::info;

use super::{TncError, TncEvent};

/// Blocking TCP client with short read timeouts (matches [`super::serial::SerialLink`]).
pub struct TcpKissLink {
    stream: TcpStream,
    pub addr: String,
    activity: Option<mpsc::Sender<TncEvent>>,
    last_tx_note: Option<Instant>,
    last_rx_note: Option<Instant>,
}

impl TcpKissLink {
    pub fn connect(host: &str, port: u16) -> Result<Self, TncError> {
        let host = host.trim();
        let addr = format!("{host}:{port}");
        info!(%addr, "opening KISS TCP");
        let stream = TcpStream::connect(&addr)
            .map_err(|e| TncError::Protocol(format!("KISS TCP connect {addr}: {e}")))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        Ok(Self {
            stream,
            addr,
            activity: None,
            last_tx_note: None,
            last_rx_note: None,
        })
    }

    pub fn watch(&mut self, events: mpsc::Sender<TncEvent>) {
        self.activity = Some(events);
    }

    fn note_link(&mut self, tx: bool) {
        let now = Instant::now();
        let recent = if tx {
            self.last_tx_note
        } else {
            self.last_rx_note
        };
        if recent.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(40)) {
            return;
        }
        if tx {
            self.last_tx_note = Some(now);
        } else {
            self.last_rx_note = Some(now);
        }
        if let Some(events) = &self.activity {
            let _ = events.try_send(TncEvent::LinkIo { tx });
        }
    }

    pub fn write_all(&mut self, buf: &[u8]) -> Result<(), TncError> {
        self.stream.write_all(buf)?;
        self.stream.flush()?;
        if !buf.is_empty() {
            self.note_link(true);
        }
        Ok(())
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, TncError> {
        match self.stream.read(buf) {
            Ok(0) => Err(TncError::Protocol(format!(
                "KISS TCP peer closed ({})",
                self.addr
            ))),
            Ok(n) => {
                self.note_link(false);
                Ok(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(TncError::Io(e)),
        }
    }

    pub fn flush_input(&mut self) {
        let mut buf = [0u8; 256];
        for _ in 0..32 {
            match self.try_read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    }
}
