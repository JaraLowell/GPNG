//! Experimental FT232R asynchronous bit-bang backend for BayCom ser12.
//! USB buffers clock samples, never per-bit host sleeps. CTS is RXD; DTR
//! carries NRZI TXD and active-low RTS keys the modem through RS232 drivers.

use std::fmt::Display;
use std::time::{Duration, Instant};

use libftd2xx::{BitMode, DeviceInfo, DeviceType, Ft232r, FtdiCommon};
use tokio::sync::mpsc;

use crate::config::{SerialConfig, TncConfig};
use crate::tnc::baycom::{Action, Baycom, Nrzi, Settings, BIT_RATE};
use crate::tnc::kiss_ini::parse_params_section;
use crate::tnc::{
    resolve_kiss_ini, shutdown_requested, DetectedPort, HostCode, TncCommand, TncError, TncEvent,
};

const SAMPLES_PER_BIT: usize = 8;
const SAMPLE_RATE: usize = BIT_RATE as usize * SAMPLES_PER_BIT;
// FT232R bit-bang sample clock = configured UART baud rate * 16.
const CLOCK_BAUD: u32 = SAMPLE_RATE as u32 / 16;
const TXD: u8 = 1;
const RTS: u8 = 1 << 2;
const CTS: u8 = 1 << 3;
const DTR: u8 = 1 << 4;
const OUTPUTS: u8 = TXD | RTS | DTR;
const IDLE: u8 = RTS;
const USB_PREFIX: &str = "FTDI:";
const SAMPLE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_BACKLOG: usize = SAMPLE_RATE / 4;

fn error(operation: &str, reason: impl Display) -> TncError {
    TncError::Protocol(format!("BayCom FT232R {operation}: {reason}"))
}

fn devices() -> Result<Vec<DeviceInfo>, TncError> {
    libftd2xx::list_devices().map_err(|e| error("USB enumeration", e))
}

fn supported_device(device: &DeviceInfo) -> bool {
    device.device_type == DeviceType::FT232R
        && device.vendor_id == 0x0403
        && device.product_id == 0x6001
}

/// Direct-USB identities remain usable when the VCP driver is detached.
pub fn detect_ports() -> Result<Vec<DetectedPort>, TncError> {
    Ok(devices()?
        .into_iter()
        .filter(|d| supported_device(d) && !d.serial_number.trim().is_empty())
        .map(|d| DetectedPort {
            name: format!("{USB_PREFIX}{}", d.serial_number),
            detail: format!("FT232R direct USB ({})", d.description),
        })
        .collect())
}

fn serial_for_port(
    selected: &str,
    ports: &[serialport::SerialPortInfo],
) -> Result<String, TncError> {
    let port = ports
        .iter()
        .find(|p| {
            if cfg!(windows) {
                p.port_name.eq_ignore_ascii_case(selected)
            } else {
                p.port_name == selected
            }
        })
        .ok_or_else(|| error("selection", format!("port {selected} is not present")))?;
    let serialport::SerialPortType::UsbPort(usb) = &port.port_type else {
        return Err(error(
            "selection",
            "requires an FT232R USB-to-RS232 adapter",
        ));
    };
    if usb.vid != 0x0403 || usb.pid != 0x6001 {
        return Err(error(
            "selection",
            "requires FTDI VID 0403 / PID 6001; PL2303 and other adapters are unsupported",
        ));
    }
    usb.serial_number
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .ok_or_else(|| error("selection", "adapter needs a unique USB serial number"))
}

fn validate_identity(serial: &str, devices: &[DeviceInfo]) -> Result<(), TncError> {
    let matches: Vec<_> = devices
        .iter()
        .filter(|d| d.serial_number == serial)
        .collect();
    if serial.trim().is_empty() || matches.len() != 1 {
        return Err(error(
            "selection",
            "USB serial identity is missing or ambiguous; never opening a different adapter",
        ));
    }
    if matches[0].port_open {
        return Err(error(
            "selection",
            "adapter is already open; close competing programs",
        ));
    }
    if !supported_device(matches[0]) {
        return Err(error(
            "selection",
            "selected USB device is not a supported 0403:6001 FT232R",
        ));
    }
    Ok(())
}

trait Samples {
    fn write_samples(&mut self, samples: &[u8]) -> Result<(), TncError>;
    fn read_samples(&mut self, samples: &mut [u8]) -> Result<usize, TncError>;
    fn pins(&mut self) -> Result<u8, TncError>;
    fn discard_rx(&mut self) -> Result<(), TncError>;
}

impl Samples for Ft232r {
    fn write_samples(&mut self, samples: &[u8]) -> Result<(), TncError> {
        self.write_all(samples).map_err(|e| error("write", e))
    }

    fn read_samples(&mut self, samples: &mut [u8]) -> Result<usize, TncError> {
        let pending = self
            .status()
            .map_err(|e| error("status", e))?
            .ammount_in_rx_queue as usize;
        // D2XX supplies no per-sample timestamps or overflow markers.
        // Stop rather than decode a substantially delayed, possibly gapped stream.
        if pending > MAX_BACKLOG {
            return Err(error(
                "receive",
                "sample backlog exceeded 250 ms; possible sample loss",
            ));
        }
        let count = pending.min(samples.len());
        if count == 0 {
            return Ok(0);
        }
        self.read(&mut samples[..count])
            .map_err(|e| error("read", e))
    }

    fn pins(&mut self) -> Result<u8, TncError> {
        self.bit_mode().map_err(|e| error("GPIO readback", e))
    }

    fn discard_rx(&mut self) -> Result<(), TncError> {
        self.purge_rx().map_err(|e| error("purge RX", e))
    }
}

struct ClockRecovery {
    last: Option<bool>,
    until_center: usize,
    since_edge: usize,
    good_edges: u8,
    carrier_left: usize,
    nrzi: Nrzi,
}

impl Default for ClockRecovery {
    fn default() -> Self {
        Self {
            last: None,
            until_center: SAMPLES_PER_BIT / 2,
            since_edge: SAMPLE_RATE,
            good_edges: 0,
            carrier_left: 0,
            nrzi: Nrzi::new(true),
        }
    }
}

impl ClockRecovery {
    fn push(&mut self, pins: u8) -> Option<bool> {
        let level = pins & CTS != 0;
        self.since_edge = self.since_edge.saturating_add(1);
        self.carrier_left = self.carrier_left.saturating_sub(1);
        if self.last.is_some_and(|last| last != level) {
            if (SAMPLES_PER_BIT / 2..=SAMPLES_PER_BIT * 8).contains(&self.since_edge) {
                self.good_edges = self.good_edges.saturating_add(1);
                if self.good_edges >= 3 {
                    self.carrier_left = SAMPLE_RATE / 10;
                }
            } else {
                self.good_edges = 0;
            }
            self.since_edge = 0;
            self.until_center = SAMPLES_PER_BIT / 2;
        }
        self.last = Some(level);
        self.until_center -= 1;
        if self.until_center == 0 {
            self.until_center = SAMPLES_PER_BIT;
            Some(self.nrzi.decode(level))
        } else {
            None
        }
    }

    fn carrier(&self) -> bool {
        self.carrier_left != 0
    }
}

fn waveform(bits: &[bool]) -> Vec<u8> {
    let mut nrzi = Nrzi::new(false);
    let mut samples = Vec::with_capacity(bits.len() * SAMPLES_PER_BIT + 1);
    for &bit in bits {
        let value = if nrzi.encode(bit) { DTR } else { 0 };
        samples.extend(std::iter::repeat(value).take(SAMPLES_PER_BIT));
    }
    // The last hardware-clocked sample releases PTT. Readback of this
    // sentinel proves more than an empty host-side USB transmit queue.
    samples.push(IDLE);
    samples
}

fn wait_ptt(io: &mut impl Samples, keyed: bool, timeout: Duration) -> Result<(), TncError> {
    let deadline = Instant::now() + timeout;
    loop {
        if (io.pins()? & RTS == 0) == keyed {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(error("PTT", "GPIO completion readback timed out"));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn transmit(io: &mut impl Samples, modem: &mut Baycom) -> Result<Vec<u8>, TncError> {
    io.write_samples(&[0])?;
    wait_ptt(io, true, Duration::from_millis(250))?;
    let actions = modem
        .ptt_changed(true, Instant::now())
        .map_err(|e| error("PTT confirmation", e))?;
    let Some(Action::SendBits(bits)) = actions.into_iter().next() else {
        return Err(error("transmit", "missing bit stream"));
    };
    let samples = waveform(&bits);
    let timeout =
        Duration::from_secs_f64(samples.len() as f64 / SAMPLE_RATE as f64) + Duration::from_secs(2);
    io.write_samples(&samples)?;
    wait_ptt(io, false, timeout)?;
    let actions = modem
        .transmit_complete()
        .map_err(|e| error("completion", e))?;
    modem
        .ptt_changed(false, Instant::now())
        .map_err(|e| error("PTT release", e))?;
    io.discard_rx()?;
    actions
        .into_iter()
        .find_map(|action| match action {
            Action::Transmitted(frame) => Some(frame),
            _ => None,
        })
        .ok_or_else(|| error("completion", "missing transmitted frame"))
}

fn emit(events: &mpsc::Sender<TncEvent>, event: TncEvent) -> Result<(), TncError> {
    events
        .try_send(event)
        .map_err(|e| error("event delivery", e))
}

fn banner(modem: &Baycom) -> String {
    let s = modem.settings();
    format!(
        "BayCom FT232R 1200 bit/s (experimental)\nTX delay {} ms; tail {} ms; slot {} ms; persistence {}; turnaround {} ms",
        s.tx_delay.as_millis(), s.tx_tail.as_millis(), s.slot_time.as_millis(),
        s.persist, s.turnaround.as_millis()
    )
}

fn run_loop(
    io: &mut impl Samples,
    mut modem: Baycom,
    events: &mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let mut clock = ClockRecovery::default();
    let mut samples = [0; 1024];
    let mut last_input = Instant::now();
    let mut last_service = Instant::now();
    let mut ready = false;
    let mut bad_frames = 0u32;
    let mut last_bad_report = Instant::now() - Duration::from_secs(1);
    let mut last_receive_error = String::new();
    loop {
        if shutdown_requested() {
            return Ok(());
        }
        if last_service.elapsed() > Duration::from_millis(200) {
            return Err(error(
                "receive",
                "USB sample servicing stalled; reopen the radio",
            ));
        }
        last_service = Instant::now();
        let count = io.read_samples(&mut samples)?;
        if count > 0 {
            last_input = Instant::now();
            for &sample in &samples[..count] {
                if let Some(bit) = clock.push(sample) {
                    match modem.receive_bit(bit) {
                        Some(Ok(payload)) => {
                            emit(
                                events,
                                TncEvent::LinkIo {
                                    tx: false,
                                    index: 0,
                                },
                            )?;
                            emit(
                                events,
                                TncEvent::HostFrame {
                                    channel: 0,
                                    code: HostCode::Data,
                                    payload,
                                },
                            )?;
                        }
                        Some(Err(e)) => {
                            bad_frames = bad_frames.saturating_add(1);
                            last_receive_error = e.to_string();
                        }
                        None => {}
                    }
                }
            }
            if !ready {
                ready = true;
                emit(events, TncEvent::TimingBanner(banner(&modem)))?;
                emit(
                    events,
                    TncEvent::Status("BayCom FT232R ready (experimental)".into()),
                )?;
            }
        } else if last_input.elapsed() > SAMPLE_TIMEOUT {
            return Err(error("receive", "no GPIO samples for 500 ms"));
        }
        if bad_frames != 0 && last_bad_report.elapsed() >= Duration::from_secs(1) {
            emit(
                events,
                TncEvent::Warning(format!(
                    "BayCom rejected {bad_frames} HDLC frame(s); last error: {last_receive_error}"
                )),
            )?;
            bad_frames = 0;
            last_bad_report = Instant::now();
        }
        for _ in 0..16 {
            match commands.try_recv() {
                Ok(TncCommand::Shutdown) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Ok(())
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Ok(TncCommand::HostFrame { channel: 0, data })
                | Ok(TncCommand::KissBroadcast { data, .. }) => {
                    modem.enqueue(&data).map_err(|e| error("enqueue", e))?;
                }
                Ok(TncCommand::HostFrame { channel, .. }) => {
                    return Err(error(
                        "radio port",
                        format!("only port 0 exists, not {channel}"),
                    ));
                }
                Ok(TncCommand::KissParam { cmd, arg }) => match modem.set_parameter(cmd, arg) {
                    Ok(()) => emit(events, TncEvent::TimingBanner(banner(&modem)))?,
                    Err(e) => emit(events, TncEvent::Warning(e.to_string()))?,
                },
                Ok(TncCommand::QueryTiming) => {
                    emit(events, TncEvent::TimingBanner(banner(&modem)))?
                }
                Ok(TncCommand::Reinitialize) => {
                    return Err(error("reinitialize", "stop and reopen the BayCom radio"));
                }
                Ok(_) => emit(
                    events,
                    TncEvent::Warning("BayCom has no hostmode command channel".into()),
                )?,
            }
        }
        if ready
            && !modem
                .poll(Instant::now(), clock.carrier(), rand::random())
                .is_empty()
        {
            let frame = transmit(io, &mut modem)?;
            // Samples collected while keyed include self-echo, not peer traffic.
            clock = ClockRecovery::default();
            last_input = Instant::now();
            last_service = Instant::now();
            emit(events, TncEvent::LinkIo { tx: true, index: 0 })?;
            emit(events, TncEvent::KissTx { port: 0, frame })?;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn configure(io: &mut Ft232r) -> Result<(), TncError> {
    io.set_timeouts(Duration::from_millis(100), Duration::from_secs(10))
        .map_err(|e| error("timeouts", e))?;
    io.clear_rts()
        .map_err(|e| error("initial RTS release", e))?;
    io.set_bit_mode(0, BitMode::Reset)
        .map_err(|e| error("reset mode", e))?;
    io.purge_all().map_err(|e| error("purge", e))?;
    // Complete IN requests after one USB packet, not 4 KiB of samples.
    io.set_usb_parameters(64)
        .map_err(|e| error("USB parameters", e))?;
    io.set_latency_timer(Duration::from_millis(16))
        .map_err(|e| error("latency", e))?;
    // Stage the idle latch before enabling the PTT output.
    io.set_bit_mode(TXD | DTR, BitMode::AsyncBitbang)
        .map_err(|e| error("bit-bang mode", e))?;
    io.set_baud_rate(CLOCK_BAUD)
        .map_err(|e| error("sample clock", e))?;
    io.write_samples(&[IDLE])?;
    io.set_bit_mode(OUTPUTS, BitMode::AsyncBitbang)
        .map_err(|e| error("GPIO directions", e))?;
    io.set_baud_rate(CLOCK_BAUD)
        .map_err(|e| error("sample clock", e))?;
    io.write_samples(&[IDLE])?;
    wait_ptt(io, false, Duration::from_millis(250))?;
    io.discard_rx()
}

fn release(io: &mut Ft232r) -> Result<(), TncError> {
    // Attempt every safety operation even when an earlier operation fails.
    let timeouts = io
        .set_timeouts(Duration::from_millis(100), Duration::from_millis(250))
        .map_err(|e| error("shutdown timeouts", e));
    let purge = io.purge_tx().map_err(|e| error("shutdown purge", e));
    let idle = io
        .write_samples(&[IDLE])
        .and_then(|_| wait_ptt(io, false, Duration::from_millis(250)));
    let rts = io.clear_rts().map_err(|e| error("shutdown RTS release", e));
    let reset = io
        .set_bit_mode(0, BitMode::Reset)
        .map_err(|e| error("shutdown mode", e));
    let close = io.close().map_err(|e| error("close", e));
    let failures: Vec<_> = [timeouts, purge, idle, rts, reset, close]
        .into_iter()
        .filter_map(Result::err)
        .map(|e| e.to_string())
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(error(
            "shutdown",
            format!(
                "{}; PTT safety could not be fully confirmed: disable the transmitter",
                failures.join("; ")
            ),
        ))
    }
}

pub fn run(
    serial: SerialConfig,
    config: TncConfig,
    events: mpsc::Sender<TncEvent>,
    commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    if serial.baud != BIT_RATE {
        return Err(error(
            "configuration",
            "only a 1200 bit/s radio rate is supported",
        ));
    }
    if config.kiss_port != 0 || config.channel_ports.iter().any(|&port| port != 0) {
        return Err(error(
            "configuration",
            "one radio only: kiss_port and channel_ports must be 0",
        ));
    }
    let mut settings = Settings::default();
    let path = resolve_kiss_ini(&config.kiss_ini);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let params = parse_params_section(&text).ok_or_else(|| {
                error(
                    "parameters",
                    format!("{} has no [params] section", path.display()),
                )
            })?;
            for param in params {
                settings
                    .set_parameter(param.cmd, param.value)
                    .map_err(|e| error("parameters", e))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error("parameters", format!("{}: {e}", path.display()))),
    }
    let modem = Baycom::new(settings).map_err(|e| error("configuration", e))?;
    let identity = if let Some(identity) = serial.port.strip_prefix(USB_PREFIX) {
        identity.to_string()
    } else {
        let ports = serialport::available_ports().map_err(|e| error("serial enumeration", e))?;
        serial_for_port(&serial.port, &ports)?
    };
    validate_identity(&identity, &devices()?)?;
    let mut io = Ft232r::with_serial_number(&identity).map_err(|e| {
        error(
            "open",
            format!(
                "{e}; close other programs and check D2XX/VCP driver ownership (docs/BAYCOM.md)"
            ),
        )
    })?;
    let result = configure(&mut io).and_then(|_| run_loop(&mut io, modem, &events, commands));
    let cleanup = release(&mut io);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
        (Err(e), Err(cleanup)) => Err(error("failure", format!("{e}; {cleanup}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tnc::baycom::{encode_frame, Receiver};
    use std::collections::VecDeque;

    #[test]
    fn gpio_clock_recovers_binary_frames_with_phase_polarity_and_clock_error() {
        let frame: Vec<_> = (0..=255).collect();
        let bits: Vec<_> = encode_frame(b"preamble")
            .unwrap()
            .into_iter()
            .chain(encode_frame(&frame).unwrap())
            .collect();
        for invert in [false, true] {
            for phase in 0..SAMPLES_PER_BIT {
                for scale in [0.98, 1.0, 1.02] {
                    let mut nrzi = Nrzi::new(false);
                    let levels: Vec<_> =
                        bits.iter().map(|&bit| nrzi.encode(bit) ^ invert).collect();
                    let mut clock = ClockRecovery::default();
                    let mut rx = Receiver::default();
                    let mut received = Vec::new();
                    let count = (levels.len() as f64 * SAMPLES_PER_BIT as f64 * scale) as usize;
                    for n in phase..count {
                        let index = (n as f64 / (SAMPLES_PER_BIT as f64 * scale)) as usize;
                        if let Some(bit) = clock.push(if levels[index] { CTS } else { 0 }) {
                            if let Some(Ok(frame)) = rx.push(bit) {
                                received.push(frame);
                            }
                        }
                    }
                    assert!(
                        received.contains(&frame),
                        "invert={invert}, phase={phase}, scale={scale}"
                    );
                    assert!(clock.carrier());
                    for _ in 0..SAMPLE_RATE / 5 {
                        clock.push(if *levels.last().unwrap() { CTS } else { 0 });
                    }
                    assert!(!clock.carrier());
                }
            }
        }
    }

    struct Fake {
        writes: Vec<Vec<u8>>,
        readback: VecDeque<u8>,
        fail_write: bool,
        purged: bool,
    }

    impl Samples for Fake {
        fn write_samples(&mut self, samples: &[u8]) -> Result<(), TncError> {
            if self.fail_write && !self.writes.is_empty() {
                return Err(error("write", "simulated USB disconnect"));
            }
            self.writes.push(samples.to_vec());
            Ok(())
        }
        fn read_samples(&mut self, _: &mut [u8]) -> Result<usize, TncError> {
            Ok(0)
        }
        fn pins(&mut self) -> Result<u8, TncError> {
            self.readback
                .pop_front()
                .ok_or_else(|| error("readback", "no completion"))
        }
        fn discard_rx(&mut self) -> Result<(), TncError> {
            self.purged = true;
            Ok(())
        }
    }

    #[test]
    fn transmit_requires_key_and_final_unkey_readback_not_usb_acceptance() {
        for failure in 0..3 {
            let mut modem = Baycom::new(Settings {
                persist: 255,
                ..Settings::default()
            })
            .unwrap();
            modem.enqueue(b"binary\0\xff").unwrap();
            let now = Instant::now();
            assert!(modem.poll(now, false, 0).is_empty());
            assert_eq!(
                modem.poll(now + Duration::from_secs(1), false, 0),
                [Action::Ptt(true)]
            );
            let mut io = Fake {
                writes: Vec::new(),
                readback: if failure == 2 {
                    [IDLE, 0, 0].into()
                } else {
                    [IDLE, 0, 0, 0, IDLE].into()
                },
                fail_write: failure == 1,
                purged: false,
            };
            let result = transmit(&mut io, &mut modem);
            if failure != 0 {
                let expected = if failure == 1 {
                    "disconnect"
                } else {
                    "readback"
                };
                assert!(result.unwrap_err().to_string().contains(expected));
                assert_eq!(modem.buffered_frames(), 1);
                assert!(!io.purged);
            } else {
                assert_eq!(result.unwrap(), b"binary\0\xff");
                assert_eq!(modem.buffered_frames(), 0);
                assert!(io.readback.is_empty());
                assert!(io.purged);
                let samples = &io.writes[1];
                assert_eq!(samples.last(), Some(&IDLE));
                assert!(samples[..samples.len() - 1]
                    .iter()
                    .all(|s| s & (RTS | TXD) == 0));
                let mut nrzi = Nrzi::new(false);
                let mut rx = Receiver::default();
                let decoded: Vec<_> = samples[..samples.len() - 1]
                    .chunks_exact(SAMPLES_PER_BIT)
                    .filter_map(|s| {
                        assert!(s.iter().all(|&b| b == s[0]));
                        rx.push(nrzi.decode(s[0] & DTR != 0))
                    })
                    .collect();
                assert_eq!(decoded, [Ok(b"binary\0\xff".to_vec())]);
            }
        }
    }

    #[test]
    fn usb_selection_never_falls_back_to_first_adapter() {
        use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};
        let mut port = SerialPortInfo {
            port_name: "COM999".into(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x0403,
                pid: 0x6001,
                serial_number: Some("selected".into()),
                manufacturer: None,
                product: None,
            }),
        };
        assert_eq!(
            serial_for_port("COM999", &[port.clone()]).unwrap(),
            "selected"
        );
        assert!(serial_for_port("COM998", &[port.clone()]).is_err());
        if let SerialPortType::UsbPort(usb) = &mut port.port_type {
            usb.vid = 0x067b;
        }
        assert!(serial_for_port("COM999", &[port.clone()]).is_err());
        if let SerialPortType::UsbPort(usb) = &mut port.port_type {
            usb.vid = 0x0403;
            usb.serial_number = None;
        }
        assert!(serial_for_port("COM999", &[port]).is_err());
        let mut device = DeviceInfo {
            port_open: false,
            speed: None,
            device_type: DeviceType::FT232R,
            vendor_id: 0x0403,
            product_id: 0x6001,
            serial_number: "selected".into(),
            description: "test".into(),
        };
        assert!(validate_identity("selected", &[device.clone()]).is_ok());
        assert!(validate_identity("other", &[device.clone()]).is_err());
        assert!(validate_identity("selected", &[device.clone(), device.clone()]).is_err());
        device.port_open = true;
        assert!(validate_identity("selected", &[device.clone()]).is_err());
        device.port_open = false;
        device.device_type = DeviceType::FT2232H;
        assert!(validate_identity("selected", &[device]).is_err());
    }
}
