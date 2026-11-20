//! WA8DED TNC2 / NORD><LINK TheFirmware, normal JHOST1 polling mode.
//! TNC3 extensions are outside this driver's compatibility target.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::{SerialConfig, TncConfig};

use super::super::hostmode::{
    exchange, expand_command, parse_free_buffer, parse_link_status, HostCode, HostIo, InboundFrame,
};
use super::super::serial::SerialLink;
use super::super::wa8ded_ini::{self, resolve_wa8ded_ini};
use super::super::reconnect::{self, Attempt};
use super::super::{BufferReport, ChannelBuffers, LinkPhase, TncCommand, TncError, TncEvent};

const CHANNELS: u8 = 10;

pub fn run(
    serial: SerialConfig,
    config: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let label = {
        let port = serial.port.trim();
        if port.is_empty() {
            "COM".to_string()
        } else {
            port.to_string()
        }
    };
    let mut failures = 0u8;
    loop {
        match run_once(&serial, &config, &events, &mut commands, &label, &mut failures) {
            Attempt::Stop => return Ok(()),
            Attempt::Again(err) => {
                if !reconnect::fail_link(&events, &mut commands, &label, &mut failures, &err) {
                    return Ok(());
                }
            }
        }
    }
}

fn run_once(
    serial: &SerialConfig,
    config: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    commands: &mut mpsc::Receiver<TncCommand>,
    label: &str,
    failures: &mut u8,
) -> Attempt {
    let mut link = match SerialLink::open(serial) {
        Ok(link) => link,
        Err(err) => return Attempt::Again(err),
    };
    link.watch(events.clone());
    let _ = events.blocking_send(TncEvent::Status(format!(
        "Opening WA8DED TNC2 on {} ({} baud)",
        link.port_name, link.baud
    )));
    let result = run_hostmode(
        &mut link,
        config,
        events,
        commands,
        serial.poll_interval_ms(),
        label,
        failures,
    );
    link.prepare_close();
    match result {
        Ok(()) => Attempt::Stop,
        Err(err) => Attempt::Again(err),
    }
}

fn run_hostmode(
    link: &mut impl HostIo,
    config: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
    commands: &mut mpsc::Receiver<TncCommand>,
    poll_ms: u64,
    label: &str,
    failures: &mut u8,
) -> Result<(), TncError> {
    let mut stream = Vec::new();
    enter_hostmode(link, &mut stream, events)?;
    run_open(link, &mut stream, config, events)?;
    *failures = 0;
    let _ = events.blocking_send(TncEvent::Modem {
        index: 0,
        label: label.to_string(),
        phase: LinkPhase::Open,
    });
    let mut poller = Poller::default();
    let mut status = 0;
    let mut next_status = Instant::now();
    let interval = Duration::from_millis(poll_ms.max(10));
    let mut next_poll = Instant::now();
    loop {
        match commands.try_recv() {
            Ok(TncCommand::Shutdown) | Err(mpsc::error::TryRecvError::Disconnected) => {
                run_close(link, &mut stream, config, events)?;
                let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
                return Ok(());
            }
            Ok(TncCommand::HostFrame { channel, data }) => {
                let reply = exchange(link, &mut stream, channel, true, &data, Some(events))?;
                if data.starts_with(b"I ") || data.starts_with(b"Y ") {
                    require_success(&reply, &String::from_utf8_lossy(&data))?;
                }
                emit(events, reply);
            }
            Ok(TncCommand::HostData { channel, data }) => {
                let reply = exchange(link, &mut stream, channel, false, &data, Some(events))?;
                emit(events, reply);
            }
            Ok(TncCommand::TerminalLine(line)) => {
                let command = line.trim().trim_start_matches('\x1b');
                let reply = exchange(link, &mut stream, 0, true, command.as_bytes(), Some(events))?;
                emit(events, reply);
            }
            Ok(TncCommand::QueryTiming) => timing(link, &mut stream, config, events)?,
            Ok(TncCommand::Reinitialize) => {
                for channel in 1..=CHANNELS {
                    let reply = exchange(link, &mut stream, channel, true, b"D", Some(events))?;
                    require_success(&reply, "disconnect for reinitialization")?;
                }
                run_open(link, &mut stream, config, events)?;
                poller = Poller::default();
            }
            Ok(TncCommand::KissParam { .. }) => {
                let _ = events.blocking_send(TncEvent::Warning(
                    "KISS parameters are not supported by WA8DED TNC2".into(),
                ));
            }
            Ok(TncCommand::Session(_)) => {
                return Err(TncError::Protocol("unadapted session command".into()));
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        let now = Instant::now();
        if now >= next_poll {
            poller.poll(link, &mut stream, events)?;
            next_poll = Instant::now() + interval;
        }
        if now >= next_status {
            let (channel, command) = if status == 0 {
                (0, &b"@B"[..])
            } else {
                (status - 1, &b"L"[..])
            };
            let reply = exchange(link, &mut stream, channel, true, command, Some(events))?;
            if reply.code == HostCode::CommandReply {
                let report = if status == 0 {
                    parse_free_buffer(&reply.payload).map(|free| BufferReport {
                        free: Some(free),
                        not_displayed: None,
                        replace_rows: false,
                        rows: Vec::new(),
                    })
                } else {
                    parse_link_status(&reply.payload).map(|p| BufferReport {
                        free: None,
                        not_displayed: (channel == 0).then_some(p.not_disp),
                        replace_rows: false,
                        rows: if channel != 0 && p.full {
                            vec![ChannelBuffers {
                                channel,
                                peer: String::new(),
                                not_disp: p.not_disp,
                                not_sent: p.not_sent,
                                tries: p.tries,
                                unack: p.unack,
                            }]
                        } else {
                            Vec::new()
                        },
                    })
                };
                if let Some(report) = report {
                    let _ = events.blocking_send(TncEvent::Buffers(report));
                } else {
                    let _ = events.blocking_send(TncEvent::Warning(format!(
                        "invalid TNC2 counter reply on ch{channel}: {:?}",
                        String::from_utf8_lossy(&reply.payload)
                    )));
                }
            } else {
                emit(events, reply);
            }
            status = (status + 1) % (CHANNELS + 2);
            next_status = Instant::now() + Duration::from_millis(170);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn drain_terminal(link: &mut impl HostIo, events: &mpsc::Sender<TncEvent>) -> Result<(), TncError> {
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut buf = [0; 256];
    while Instant::now() < deadline {
        let n = link.try_read(&mut buf)?;
        if n > 0 {
            let _ = events.blocking_send(TncEvent::RawRx(buf[..n].to_vec()));
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}

fn enter_hostmode(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    link.write_all(b"\x11\x18\x1b")?;
    drain_terminal(link, events)?;
    link.write_all(b"JHOST1\r")?;
    drain_terminal(link, events)?;
    let reply = exchange(link, stream, 0, true, b"V", Some(events))?;
    if reply.code != HostCode::CommandReply || reply.payload.is_empty() {
        return Err(TncError::Protocol(
            "TNC2 did not confirm binary hostmode; check baud/firmware and restart the modem"
                .into(),
        ));
    }
    let _ = events.blocking_send(TncEvent::Status(format!(
        "WA8DED TNC2 firmware: {}",
        String::from_utf8_lossy(&reply.payload)
    )));
    Ok(())
}

fn require_success(reply: &InboundFrame, command: &str) -> Result<(), TncError> {
    if !matches!(reply.code, HostCode::Ok | HostCode::CommandReply) {
        return Err(TncError::Protocol(format!(
            "{command}: TNC2 response {:?}: {}",
            reply.code,
            String::from_utf8_lossy(&reply.payload)
        )));
    }
    Ok(())
}

fn emit(events: &mpsc::Sender<TncEvent>, reply: InboundFrame) {
    let _ = events.blocking_send(TncEvent::HostFrame {
        channel: reply.channel,
        code: reply.code,
        payload: reply.payload,
    });
}

fn command_list(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    commands: &[String],
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    for command in commands {
        for wire in expand_command(command) {
            let reply = exchange(link, stream, 0, true, wire.as_bytes(), Some(events))?;
            require_success(&reply, &wire)?;
        }
    }
    Ok(())
}

fn run_open(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    config: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let reply = exchange(link, stream, 0, true, b"Y0", Some(events))?;
    require_success(&reply, "disable incoming connects during initialization")?;
    let path = resolve_wa8ded_ini(&config.wa8ded_ini);
    if !config.wa8ded_ini.is_empty() && path.exists() {
        wa8ded_ini::apply_section(
            link,
            stream,
            &path,
            "open",
            config.wa8ded_line_delay_ms,
            &config.monitor_mode,
            Some(events),
        )?;
    } else {
        let lines: Vec<_> = config
            .init
            .iter()
            .map(|line| wa8ded_ini::expand_placeholders(line, &config.monitor_mode))
            .collect();
        command_list(link, stream, &lines, events)?;
    }
    if !config.runtime_mycall.is_empty() {
        let command = format!("I {}", config.runtime_mycall);
        let reply = exchange(link, stream, 0, true, command.as_bytes(), Some(events))?;
        require_success(&reply, &command)?;
    }
    timing(link, stream, config, events)?;
    let reply = exchange(link, stream, 0, true, b"M", Some(events))?;
    require_success(&reply, "M")?;
    emit(events, reply);
    let _ = events.blocking_send(TncEvent::Status("WA8DED TNC2 hostmode ready".into()));
    Ok(())
}

fn timing(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    config: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let path = resolve_wa8ded_ini(&config.wa8ded_ini);
    let open = if !config.wa8ded_ini.is_empty() && path.exists() {
        wa8ded_ini::read_section(&path, "open")?
            .ok_or_else(|| TncError::Protocol(format!("missing [open] in {}", path.display())))?
    } else {
        config.init.clone()
    };
    let queries = if !config.wa8ded_ini.is_empty() && path.exists() {
        wa8ded_ini::read_section(&path, "status")?
            .unwrap_or_else(|| vec!["F".into(), "@T2".into(), "@T3".into()])
    } else {
        vec!["F".into(), "@T2".into(), "@T3".into()]
    };
    let mut replies = Vec::new();
    for query in queries {
        let reply = exchange(link, stream, 0, true, query.as_bytes(), Some(events))?;
        if reply.code == HostCode::CommandReply {
            replies.push((query, String::from_utf8_lossy(&reply.payload).into_owned()));
        } else {
            let _ = events.blocking_send(TncEvent::Warning(format!(
                "TNC2 timing query {query}: {:?} {}",
                reply.code,
                String::from_utf8_lossy(&reply.payload)
            )));
        }
    }
    let banner = wa8ded_ini::format_timing_banner(&config.runtime_mycall, &replies, &open);
    let _ = events.blocking_send(TncEvent::TimingBanner(banner));
    Ok(())
}

fn run_close(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    config: &TncConfig,
    events: &mpsc::Sender<TncEvent>,
) -> Result<(), TncError> {
    let path = resolve_wa8ded_ini(&config.wa8ded_ini);
    let mut lines = if !config.wa8ded_ini.is_empty() && path.exists() {
        wa8ded_ini::read_section(&path, "close")?
            .ok_or_else(|| TncError::Protocol(format!("missing [close] in {}", path.display())))?
    } else {
        config.deinit.clone()
    };
    if !lines.iter().any(|line| line.eq_ignore_ascii_case("JHOST0")) {
        lines.push("JHOST0".into());
    }
    if lines
        .iter()
        .position(|line| line.eq_ignore_ascii_case("JHOST0"))
        != Some(lines.len() - 1)
    {
        return Err(TncError::Protocol(
            "JHOST0 must be the last close command".into(),
        ));
    }
    let lines: Vec<_> = lines
        .iter()
        .map(|line| wa8ded_ini::expand_placeholders(line, &config.monitor_mode))
        .collect();
    command_list(link, stream, &lines, events)
}

#[derive(Default)]
struct Poller {
    extended: Option<bool>,
    pending: VecDeque<u8>,
    channel: u8,
}

impl Poller {
    fn poll(
        &mut self,
        link: &mut impl HostIo,
        stream: &mut Vec<u8>,
        events: &mpsc::Sender<TncEvent>,
    ) -> Result<(), TncError> {
        if self.extended != Some(false) && self.pending.is_empty() {
            let reply = exchange(link, stream, 255, true, b"G", Some(events))?;
            if reply.code == HostCode::Error {
                self.extended = Some(false);
                let _ = events.blocking_send(TncEvent::Status(format!(
                    "TNC2 extended poll unavailable; polling channels 0..10: {}",
                    String::from_utf8_lossy(&reply.payload)
                )));
            } else {
                require_success(&reply, "extended G")?;
                for channel in reply.payload {
                    if !(1..=CHANNELS + 1).contains(&channel) {
                        return Err(TncError::Protocol(format!(
                            "invalid TNC2 extended poll channel byte {channel:#04x}"
                        )));
                    }
                    self.pending.push_back(channel - 1);
                }
                self.extended = Some(true);
            }
        }
        let channel = if self.extended == Some(true) {
            let Some(channel) = self.pending.pop_front() else {
                return Ok(());
            };
            channel
        } else {
            let channel = self.channel;
            self.channel = (self.channel + 1) % (CHANNELS + 1);
            channel
        };
        let reply = exchange(link, stream, channel, true, b"G", Some(events))?;
        emit(events, reply);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tnc::{ChannelConfig, SessionCommand, SessionEvent};
    use std::collections::BTreeMap;

    struct Modem {
        stage: u8,
        extended: bool,
        reject_init: bool,
        rx: VecDeque<u8>,
        queued: BTreeMap<u8, VecDeque<(u8, Vec<u8>)>>,
        writes: Vec<Vec<u8>>,
    }

    impl Modem {
        fn new() -> Self {
            Self {
                stage: 0,
                extended: true,
                reject_init: false,
                rx: VecDeque::new(),
                writes: Vec::new(),
                queued: BTreeMap::new(),
            }
        }
        fn response(&mut self, channel: u8, code: u8, payload: &[u8]) {
            self.rx.extend([channel, code]);
            match code {
                0 => {}
                6 | 7 => {
                    self.rx.push_back((payload.len() - 1) as u8);
                    self.rx.extend(payload);
                }
                _ => {
                    self.rx.extend(payload);
                    self.rx.push_back(0);
                }
            }
        }
    }

    impl HostIo for Modem {
        fn write_all(&mut self, wire: &[u8]) -> Result<(), TncError> {
            assert!(
                self.rx.is_empty(),
                "host wrote before consuming the previous reply"
            );
            self.writes.push(wire.to_vec());
            if self.stage == 0 {
                assert_eq!(wire, b"\x11\x18\x1b");
                self.rx.extend(b"* ");
                self.stage = 1;
                return Ok(());
            }
            if self.stage == 1 {
                assert_eq!(wire, b"JHOST1\r");
                self.rx.extend(b"JHOST1\r\n");
                self.stage = 2;
                return Ok(());
            }
            assert_eq!(wire.len(), usize::from(wire[2]) + 4);
            let (channel, command, payload) = (wire[0], wire[1], &wire[3..]);
            if command == 0 {
                self.response(channel, 0, &[]);
                return Ok(());
            }
            assert_eq!(command, 1);
            if payload.starts_with(b"C ") {
                let remote = String::from_utf8_lossy(&payload[2..]);
                self.queued.entry(channel).or_default().extend([
                    (3, format!("*** CONNECTED to {remote}").into_bytes()),
                    (7, vec![channel, 0, 0xdb, 0xc0, b'\r', b'\n']),
                ]);
                self.response(channel, 0, &[]);
            } else if payload == b"D" {
                self.queued
                    .entry(channel)
                    .or_default()
                    .push_back((3, b"*** DISCONNECTED fm N1YOU".to_vec()));
                self.response(channel, 0, &[]);
            } else if channel == 255 && payload == b"G" {
                if self.extended {
                    let pending: Vec<_> = self
                        .queued
                        .iter()
                        .filter(|(_, queue)| !queue.is_empty())
                        .map(|(&ch, _)| ch + 1)
                        .collect();
                    self.response(255, 1, &pending);
                } else {
                    self.response(255, 2, b"INVALID CHANNEL NUMBER");
                }
            } else if payload == b"G" {
                let (code, payload) = self
                    .queued
                    .entry(channel)
                    .or_default()
                    .pop_front()
                    .unwrap_or((0, Vec::new()));
                self.response(channel, code, &payload);
            } else if self.reject_init && matches!(payload, b"U0" | b"I N0BAD") {
                self.response(channel, 2, b"INVALID COMMAND");
            } else {
                let text: Option<&[u8]> = match payload {
                    b"V" => Some(b"TheFirmware Version 2.7 TNC2"),
                    b"M" => Some(b"IUSC"),
                    b"F" => Some(b"250"),
                    b"@T2" => Some(b"150"),
                    b"@T3" => Some(b"18000"),
                    b"@B" => Some(b"868"),
                    b"L" if channel == 0 => Some(b"0 0"),
                    b"L" => Some(b"0 0 2 1 4 5"),
                    _ => None,
                };
                if let Some(text) = text {
                    self.response(channel, 1, text);
                } else {
                    self.response(channel, 0, &[]);
                }
            }
            Ok(())
        }
        fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, TncError> {
            let len = buf.len().min(3).min(self.rx.len());
            for byte in &mut buf[..len] {
                *byte = self.rx.pop_front().unwrap();
            }
            Ok(len)
        }
    }

    #[test]
    fn tnc2_falls_back_to_polling_every_channel_and_rejects_bad_initialization() {
        let (events, mut rx) = mpsc::channel(256);
        let mut modem = Modem::new();
        modem.stage = 2;
        modem.extended = false;
        let mut poller = Poller::default();
        for _ in 0..11 {
            poller.poll(&mut modem, &mut Vec::new(), &events).unwrap();
        }
        assert_eq!(modem.writes[0], b"\xff\x01\x00G");
        for channel in 0..=10 {
            assert_eq!(modem.writes[channel + 1], [channel as u8, 1, 0, b'G']);
        }
        modem.reject_init = true;
        let config = TncConfig {
            wa8ded_ini: String::new(),
            init: vec!["U0".into()],
            ..TncConfig::default()
        };
        assert!(run_open(&mut modem, &mut Vec::new(), &config, &events).is_err());
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, TncEvent::Status(ref s) if s == "WA8DED TNC2 hostmode ready"));
        }
    }

    #[tokio::test]
    async fn tnc2_modem_runs_two_channels_through_the_shared_stack() {
        let config = TncConfig {
            protocol: "wa8ded".into(),
            wa8ded_ini: String::new(),
            init: vec!["DateTime".into(), "U0".into(), "M IUSC".into()],
            deinit: vec!["MN".into(), "JHOST0".into()],
            runtime_mycall: "N0ME".into(),
            ..TncConfig::default()
        };
        let (commands_tx, commands_rx) = mpsc::channel(32);
        let (events_tx, mut events_rx) = mpsc::channel(256);
        let (raw_commands_tx, raw_commands_rx) = mpsc::channel(32);
        let (raw_events_tx, raw_events_rx) = mpsc::channel(256);
        let driver_config = config.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let mut modem = Modem::new();
            modem.queued.entry(0).or_default().extend([
                (5, b"fm N1HEAR to CQ ctl UI^ pid F0".to_vec()),
                (6, b"monitor\x00\xDB".to_vec()),
            ]);
            let mut raw_commands_rx = raw_commands_rx;
            let mut failures = 0u8;
            let result = run_hostmode(
                &mut modem,
                &driver_config,
                &raw_events_tx,
                &mut raw_commands_rx,
                10,
                "COM1",
                &mut failures,
            );
            (result, modem.writes)
        });
        let service = tokio::spawn(crate::tnc::session_service::run(
            config,
            commands_rx,
            events_tx,
            raw_commands_tx,
            raw_events_rx,
        ));
        commands_tx
            .send(TncCommand::Session(SessionCommand::Configure(vec![
                ChannelConfig {
                    channel: 1,
                    port: 0,
                    local: "N0ME".into(),
                },
                ChannelConfig {
                    channel: 10,
                    port: 0,
                    local: "N0ME-1".into(),
                },
            ])))
            .await
            .unwrap();
        for (channel, local, remote) in [(1, "N0ME", "N1YOU"), (10, "N0ME-1", "N2YOU")] {
            commands_tx
                .send(TncCommand::Session(SessionCommand::Connect {
                    channel,
                    port: 0,
                    local: local.into(),
                    remote: remote.into(),
                    via: Vec::new(),
                }))
                .await
                .unwrap();
        }
        let mut connected = Vec::new();
        let mut received = Vec::new();
        let mut monitor = Vec::new();
        let mut free = false;
        let mut counters = false;
        let observed = tokio::time::timeout(Duration::from_secs(8), async {
            while connected.len() < 2
                || received.len() < 2
                || monitor.len() < 2
                || !free
                || !counters
            {
                match events_rx.recv().await.unwrap() {
                    TncEvent::Session(SessionEvent::Connected { channel, .. }) => {
                        connected.push(channel)
                    }
                    TncEvent::Session(SessionEvent::Data { channel, data, .. }) => {
                        received.push((channel, data))
                    }
                    TncEvent::Session(SessionEvent::Acknowledged { .. }) => {
                        panic!("host ACK is not an RF receipt")
                    }
                    TncEvent::HostFrame {
                        channel: 0,
                        code: HostCode::MonitorHeader | HostCode::MonitorData,
                        payload,
                    } => monitor.push(payload),
                    TncEvent::Buffers(report) => {
                        free |= report.free == Some(868);
                        counters |= report.rows.iter().any(|r| {
                            r.channel == 1 && r.not_sent == 2 && r.unack == 1 && r.tries == 4
                        });
                    }
                    _ => {}
                }
            }
            commands_tx
                .send(TncCommand::Session(SessionCommand::Send {
                    channel: 10,
                    data: (0..=255).collect(),
                    id: None,
                }))
                .await
                .unwrap();
            loop {
                if matches!(
                    events_rx.recv().await.unwrap(),
                    TncEvent::HostFrame {
                        channel: 10,
                        code: HostCode::Ok,
                        ..
                    }
                ) {
                    break;
                }
            }
            for channel in [1, 10] {
                commands_tx
                    .send(TncCommand::Session(SessionCommand::Disconnect { channel }))
                    .await
                    .unwrap();
            }
            let mut disconnected = Vec::new();
            while disconnected.len() < 2 {
                if let TncEvent::Session(SessionEvent::Disconnected { channel, .. }) =
                    events_rx.recv().await.unwrap()
                {
                    disconnected.push(channel);
                }
            }
            disconnected
        })
        .await;
        commands_tx.send(TncCommand::Shutdown).await.unwrap();
        while events_rx.recv().await.is_some() {}
        let (result, writes) = worker.await.unwrap();
        service.await.unwrap();
        result.unwrap();
        assert_eq!(observed.unwrap(), [1, 10]);
        assert_eq!(connected, [1, 10]);
        assert_eq!(
            received,
            [
                (1, vec![1, 0, 0xdb, 0xc0, b'\r', b'\n']),
                (10, vec![10, 0, 0xdb, 0xc0, b'\r', b'\n']),
            ]
        );
        assert_eq!(monitor[1], b"monitor\x00\xDB");
        assert!(writes.iter().any(|wire| wire == b"\x0a\x01\x07I N0ME-1"));
        assert!(writes
            .iter()
            .any(|wire| wire.starts_with(&[10, 0, 255])
                && wire[3..] == (0..=255).collect::<Vec<u8>>()));
        assert_eq!(writes.last().unwrap(), b"\x00\x01\x05JHOST0");
    }

    #[test]
    fn tnc2_rejected_identity_stops_before_a_queued_connect() {
        let mut modem = Modem::new();
        modem.reject_init = true;
        let config = TncConfig {
            wa8ded_ini: String::new(),
            init: Vec::new(),
            ..TncConfig::default()
        };
        let (events, _rx) = mpsc::channel(256);
        let (commands, mut rx) = mpsc::channel(4);
        commands
            .blocking_send(TncCommand::HostFrame {
                channel: 10,
                data: b"I N0BAD".to_vec(),
            })
            .unwrap();
        commands
            .blocking_send(TncCommand::HostFrame {
                channel: 10,
                data: b"C N1YOU".to_vec(),
            })
            .unwrap();
        let mut failures = 0u8;
        let error = run_hostmode(&mut modem, &config, &events, &mut rx, 10, "COM1", &mut failures)
            .unwrap_err();
        assert!(error.to_string().contains("I N0BAD"));
        assert!(!modem.writes.iter().any(|wire| wire.ends_with(b"C N1YOU")));
    }
}
