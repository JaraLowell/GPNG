//! Shared serial-port helpers for all modem drivers.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use serialport::SerialPort;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::SerialConfig;

use super::{TncError, TncEvent};

/// KISS return-to-command-mode sequence (KISSINIT `\k`).
pub const KISS_EXIT: [u8; 3] = [0xC0, 0xFF, 0xC0];

/// Thin wrapper around a configured OS serial handle.
pub struct SerialLink {
    port: Box<dyn SerialPort>,
    pub port_name: String,
    pub baud: u32,
    /// UI lamp. `try_send` so a full queue never stalls the UART thread.
    activity: Option<mpsc::Sender<TncEvent>>,
    last_tx_note: Option<Instant>,
    last_rx_note: Option<Instant>,
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
            activity: None,
            last_tx_note: None,
            last_rx_note: None,
        })
    }

    /// Report raw send/receive to the session-bar lamp.
    pub fn watch(&mut self, events: mpsc::Sender<TncEvent>) {
        self.activity = Some(events);
    }

    /// One note per direction per 40 ms. A file send stays lit without
    /// filling the UI queue with one event per chunk.
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
        if !buf.is_empty() {
            self.note_link(true);
        }
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
            Ok(n) => {
                if n > 0 {
                    self.note_link(false);
                }
                Ok(n)
            }
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

/// A serial device the OS currently has. `name` is what we open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPort {
    pub name: String,
    /// USB product, or "Bluetooth". Empty when the OS gave no description.
    pub detail: String,
}

impl DetectedPort {
    pub fn menu_label(&self) -> String {
        if self.detail.is_empty() {
            self.name.clone()
        } else {
            format!("{} — {}", self.name, self.detail)
        }
    }
}

/// Ports that are present now.
///
/// Windows: devices SetupAPI reports as present, plus COM names mapped in
/// `SERIALCOMM`. macOS: the callout device (`/dev/cu.…`), not the dial-in
/// twin (`/dev/tty.…`). Linux: tty devices udev or sysfs attach to a driver
/// (USB serial, ACM, a real UART). Phantom 8250 nodes are left out by the
/// serialport crate.
pub fn detect_serial_ports() -> Vec<DetectedPort> {
    let listed = match serialport::available_ports() {
        Ok(ports) => ports,
        Err(err) => {
            warn!("serial port list: {err}");
            return Vec::new();
        }
    };
    let mut ports: Vec<DetectedPort> = listed
        .into_iter()
        .map(|info| DetectedPort {
            detail: port_detail(&info),
            name: info.port_name,
        })
        .collect();
    let names: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
    ports.retain(|p| keep_port(&p.name, &names));
    ports.sort_by(|a, b| port_ord(&a.name, &b.name));
    ports.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    ports
}

fn port_detail(info: &serialport::SerialPortInfo) -> String {
    match &info.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            let product = usb.product.as_deref().unwrap_or("").trim();
            let maker = usb.manufacturer.as_deref().unwrap_or("").trim();
            let raw = if !product.is_empty() {
                product.to_string()
            } else if !maker.is_empty() {
                maker.to_string()
            } else {
                format!("USB {:04X}:{:04X}", usb.vid, usb.pid)
            };
            clean_detail(&raw, &info.port_name)
        }
        serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
        serialport::SerialPortType::PciPort | serialport::SerialPortType::Unknown => String::new(),
    }
}

/// Windows often appends the COM name, e.g. `USB-KSG Radio Cable (COM6)`.
fn clean_detail(detail: &str, port_name: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() || port_name.is_empty() {
        return detail.to_string();
    }
    let lower = detail.to_ascii_lowercase();
    let suffix = format!("({})", port_name.to_ascii_lowercase());
    if !lower.ends_with(&suffix) {
        return detail.to_string();
    }
    detail[..detail.len() - suffix.len()]
        .trim()
        .trim_end_matches(['-', '—', '–', ':'])
        .trim()
        .to_string()
}

/// macOS lists each adapter twice. Open the callout node (`cu`), not `tty`.
fn keep_port(name: &str, all: &[String]) -> bool {
    let Some(rest) = name.strip_prefix("/dev/tty.") else {
        return true;
    };
    let callout = format!("/dev/cu.{rest}");
    !all.iter().any(|n| n == &callout)
}

/// `COM2` before `COM10`. Other paths sort after COM names, case-insensitive.
fn port_ord(a: &str, b: &str) -> std::cmp::Ordering {
    fn key(name: &str) -> (u8, u32, String) {
        let upper = name.to_ascii_uppercase();
        if let Some(n) = upper.strip_prefix("COM").and_then(|s| s.parse().ok()) {
            (0, n, String::new())
        } else {
            (1, 0, upper)
        }
    }
    key(a).cmp(&key(b))
}

/// Bytes that fit in ~20 ms at 8N1, inside the 50 ms write timeout.
pub(crate) fn tx_chunk_len(baud: u32) -> usize {
    let baud = baud.max(300) as usize;
    (baud / 500).clamp(1, 32)
}

/// How long `nbytes` take to leave a UART (8N1, 10 bit times per byte).
pub(crate) fn wire_time(baud: u32, nbytes: usize) -> Duration {
    let baud = u64::from(baud.max(300));
    let us = (nbytes as u64).saturating_mul(10).saturating_mul(1_000_000) / baud;
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

    #[test]
    fn macos_keeps_the_callout_device() {
        let all = vec![
            "/dev/tty.usbserial-1".to_string(),
            "/dev/cu.usbserial-1".to_string(),
            "/dev/tty.Bluetooth-Incoming-Port".to_string(),
        ];
        let kept: Vec<_> = all.iter().filter(|n| keep_port(n, &all)).cloned().collect();
        assert_eq!(
            kept,
            vec![
                "/dev/cu.usbserial-1".to_string(),
                "/dev/tty.Bluetooth-Incoming-Port".to_string(),
            ]
        );
    }

    #[test]
    fn detail_drops_the_repeated_com_name() {
        assert_eq!(
            clean_detail("USB-KSG Radio Cable (COM6)", "COM6"),
            "USB-KSG Radio Cable"
        );
        assert_eq!(
            clean_detail("USB-KSG Radio Cable (com6)", "COM6"),
            "USB-KSG Radio Cable"
        );
        assert_eq!(clean_detail("CP2102", "COM6"), "CP2102");
    }

    #[test]
    fn com_numbers_sort_before_device_paths() {
        let mut names = vec![
            "/dev/ttyUSB1".to_string(),
            "COM10".to_string(),
            "COM2".to_string(),
            "/dev/ttyUSB0".to_string(),
            "com1".to_string(),
        ];
        names.sort_by(|a, b| port_ord(a, b));
        assert_eq!(
            names,
            vec!["com1", "COM2", "COM10", "/dev/ttyUSB0", "/dev/ttyUSB1"]
        );
    }
}
