//! Shared serial-port helpers for all modem drivers.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use serialport::SerialPort;
use tracing::{info, warn};

use crate::config::SerialConfig;

use super::TncError;

/// KISS return-to-command-mode sequence (KISSINIT `\k`).
pub const KISS_EXIT: [u8; 3] = [0xC0, 0xFF, 0xC0];

/// Thin wrapper around a configured OS serial handle.
pub struct SerialLink {
    port: Box<dyn SerialPort>,
    pub port_name: String,
    pub baud: u32,
}

impl SerialLink {
    pub fn open(serial: &SerialConfig) -> Result<Self, TncError> {
        info!(
            port = %serial.port,
            baud = serial.baud,
            "opening modem serial port"
        );
        let mut port = serialport::new(&serial.port, serial.baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            // Read timeout. On Windows this is also WriteTotalTimeoutConstant:
            // a write that cannot finish inside it returns 0 bytes.
            .timeout(Duration::from_millis(50))
            .open()?;
        // Assert modem-control lines (physical unplug/replug toggles these).
        let _ = port.write_data_terminal_ready(true);
        let _ = port.write_request_to_send(true);
        Ok(Self {
            port,
            port_name: serial.port.clone(),
            baud: serial.baud,
        })
    }

    /// Write `buf`, waiting out a full driver buffer instead of failing.
    ///
    /// `Write::write_all` treats a 0-byte write as fatal ("failed to write
    /// whole buffer"). Windows does that when the UART cannot take the whole
    /// slice inside the 50 ms timeout — typical when a file is pushed faster
    /// than the radio drains it. Short slices are retried until they land.
    pub fn write_all(&mut self, buf: &[u8]) -> Result<(), TncError> {
        let chunk = tx_chunk_len(self.baud);
        let mut offset = 0;
        let mut last_progress = Instant::now();
        while offset < buf.len() {
            if last_progress.elapsed() > Duration::from_secs(2) {
                return Err(TncError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "serial transmit buffer stayed full",
                )));
            }
            let end = (offset + chunk).min(buf.len());
            let requested = end - offset;
            match self.port.write(&buf[offset..end]) {
                Ok(0) => {
                    std::thread::sleep(wire_time(self.baud, requested.max(1)));
                }
                Ok(n) => {
                    last_progress = Instant::now();
                    offset += n;
                    if n < requested {
                        std::thread::sleep(wire_time(self.baud, n.max(1)));
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    std::thread::sleep(wire_time(self.baud, 1));
                }
                Err(e) => return Err(TncError::Io(e)),
            }
        }
        self.port.flush()?;
        Ok(())
    }

    /// CR-terminated terminal-mode line (pre-hostmode ASCII).
    pub fn write_terminal(&mut self, line: &str) -> Result<(), TncError> {
        let mut buf = line.as_bytes().to_vec();
        if !buf.ends_with(&[b'\r']) {
            buf.push(b'\r');
        }
        self.write_all(&buf)
    }

    /// Non-blocking-ish read (port timeout → `Ok(0)`).
    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, TncError> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(TncError::Io(e)),
        }
    }

    /// Discard whatever is waiting in the UART (e.g. KISS.INI command echo).
    pub fn flush_input(&mut self) {
        let mut buf = [0u8; 256];
        for _ in 0..32 {
            match self.try_read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    }

    /// Leave KISS (may already be in command mode — harmless).
    pub fn kiss_exit(&mut self) -> Result<(), TncError> {
        self.write_all(&KISS_EXIT)?;
        // Second poke — some Kenwood firmwares ignore the first if busy.
        std::thread::sleep(Duration::from_millis(200));
        self.write_all(&KISS_EXIT)?;
        Ok(())
    }

    /// Pulse DTR like a cable reconnect (helps stubborn USB-serial / TNC state).
    pub fn pulse_dtr(&mut self) {
        if let Err(e) = self.port.write_data_terminal_ready(false) {
            warn!("DTR low failed: {e}");
        }
        std::thread::sleep(Duration::from_millis(150));
        if let Err(e) = self.port.write_data_terminal_ready(true) {
            warn!("DTR high failed: {e}");
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    /// Drop modem lines and flush before the OS handle is released.
    pub fn prepare_close(&mut self) {
        self.flush_input();
        let _ = self.port.flush();
        let _ = self.port.write_request_to_send(false);
        let _ = self.port.write_data_terminal_ready(false);
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Bytes that fit in ~20 ms at 8N1, inside the 50 ms write timeout.
pub(crate) fn tx_chunk_len(baud: u32) -> usize {
    let baud = baud.max(300) as usize;
    (baud / 500).clamp(1, 32)
}

/// How long `nbytes` take to leave a UART (8N1, 10 bit times per byte).
pub(crate) fn wire_time(baud: u32, nbytes: usize) -> Duration {
    let baud = u64::from(baud.max(300));
    let us = (nbytes as u64)
        .saturating_mul(10)
        .saturating_mul(1_000_000)
        / baud;
    Duration::from_micros(us.max(1_000))
}

impl Drop for SerialLink {
    fn drop(&mut self) {
        let _ = self.port.write_request_to_send(false);
        let _ = self.port.write_data_terminal_ready(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_fits_inside_the_write_timeout() {
        assert_eq!(tx_chunk_len(9600), 19);
        assert_eq!(tx_chunk_len(115_200), 32);
        assert_eq!(tx_chunk_len(300), 1);
    }

    #[test]
    fn wire_time_matches_8n1() {
        // 96 bytes at 9600 baud is 100 ms (10 bit times each).
        assert_eq!(wire_time(9600, 96), Duration::from_millis(100));
    }
}
