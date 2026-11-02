//! Shared serial-port helpers for all modem drivers.

use std::io::{Read, Write};
use std::time::Duration;

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

    pub fn write_all(&mut self, buf: &[u8]) -> Result<(), TncError> {
        self.port.write_all(buf)?;
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

impl Drop for SerialLink {
    fn drop(&mut self) {
        let _ = self.port.write_request_to_send(false);
        let _ = self.port.write_data_terminal_ready(false);
    }
}
