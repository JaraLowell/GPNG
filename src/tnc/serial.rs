//! Shared serial-port helpers for all modem drivers.

use std::io::{Read, Write};
use std::time::Duration;

use serialport::SerialPort;
use tracing::info;

use crate::config::SerialConfig;

use super::TncError;

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
        let port = serialport::new(&serial.port, serial.baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(50))
            .open()?;
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
}
