//! Adapts the common session API to raw KISS/BayCom or TNC-owned hostmode links.
//!
//! A pure WA8DED link does not build the KISS session machine and does not
//! wake its 20 ms timer. APRS still arrives as monitor frames and is decoded
//! in the UI, so it does not depend on the KISS driver being started.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::{HostProtocol, TncConfig};

use super::session::{
    canonical_call, command_channel, validate_configs, validate_via, Action, ChannelConfig,
    KissSessions, SessionCommand, SessionEvent, Settings,
};
use super::{HostCode, TncCommand, TncEvent};

pub(super) async fn run(
    config: TncConfig,
    mut commands: mpsc::Receiver<TncCommand>,
    events: mpsc::Sender<TncEvent>,
    driver_commands: mpsc::Sender<TncCommand>,
    mut driver_events: mpsc::Receiver<TncEvent>,
) {
    let modem = Modem::from_config(&config);
    let mut sessions = if modem.runs_kiss() {
        let mut sessions = KissSessions::new(Settings::from_config(&config));
        if modem.is_switch() {
            sessions.listen_on_every_kiss_port();
        }
        Some(sessions)
    } else {
        None
    };
    let mut host = HostSessions::default();
    let mut timer = tokio::time::interval(Duration::from_millis(20));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_buffers = None;
    let mut shutting_down = false;
    loop {
        let actions = tokio::select! {
            command = commands.recv(), if !shutting_down => {
                match command {
                    Some(TncCommand::Session(command)) => {
                        apply_session(&modem, &mut sessions, &mut host, &driver_commands, command).await
                    }
                    command => {
                        let command = command.unwrap_or(TncCommand::Shutdown);
                        let close = matches!(command, TncCommand::Shutdown | TncCommand::Reinitialize);
                        shutting_down = matches!(command, TncCommand::Shutdown);
                        let mut actions = Vec::new();
                        if close {
                            let reason = "modem stopped or reinitialized";
                            if modem.runs_kiss() {
                                actions.extend(kiss(&mut sessions).close(reason));
                            }
                            if modem.runs_host() {
                                actions.extend(host.close(reason));
                            }
                        }
                        let restores = if modem.runs_host() { host.restore_after(&command) } else { Vec::new() };
                        let gated = !restores.is_empty();
                        if gated && driver_commands.send(host_frame(0, b"Y 0".to_vec())).await.is_err() { return; }
                        if driver_commands.send(command).await.is_err() { break; }
                        for restore in restores {
                            if driver_commands.send(restore).await.is_err() { return; }
                        }
                        if gated && driver_commands.send(host.incoming_limit()).await.is_err() { return; }
                        actions
                    }
                }
            }
            event = driver_events.recv() => {
                let Some(event) = event else { break; };
                match event {
                    TncEvent::KissTx { port, ref frame } if modem.runs_kiss() => {
                        let sessions = kiss(&mut sessions);
                        if modem.is_switch() {
                            sessions.credit_transmit(frame, Instant::now());
                        } else {
                            sessions.transmitted(port, frame, Instant::now());
                        }
                        if events.send(event).await.is_err() { break; }
                        Vec::new()
                    }
                    TncEvent::HostFrame { channel: port, code: HostCode::Data, ref payload }
                        if modem.kiss_frame(port) =>
                    {
                        let sessions = kiss(&mut sessions);
                        if modem.is_switch() {
                            sessions.adopt_port(port, payload);
                        }
                        let actions = sessions.receive(port, payload, Instant::now());
                        // Monitoring is presentation only; it cannot advance a link.
                        if events.send(event).await.is_err() { break; }
                        actions
                    }
                    TncEvent::HostFrame { channel, code, payload } if channel & 0x80 != 0 => {
                        let (raw, actions) = host.event(TncEvent::HostFrame {
                            channel: channel & 0x7f,
                            code,
                            payload,
                        });
                        if let Some(raw) = raw {
                            if events.send(raw).await.is_err() { break; }
                        }
                        actions
                    }
                    TncEvent::LinkDown(ref reason) => {
                        let mut actions = Vec::new();
                        if modem.runs_kiss() {
                            actions.extend(kiss(&mut sessions).close(reason));
                        }
                        if modem.runs_host() {
                            actions.extend(host.close(reason));
                        }
                        if events.send(event).await.is_err() { break; }
                        actions
                    }
                    event if modem.runs_host() && !modem.is_switch() => {
                        let (raw, actions) = host.event(event);
                        if let Some(raw) = raw {
                            if events.send(raw).await.is_err() { break; }
                        }
                        actions
                    }
                    event => {
                        if events.send(event).await.is_err() { break; }
                        Vec::new()
                    }
                }
            }
            _ = timer.tick(), if modem.runs_kiss() && !shutting_down => {
                kiss(&mut sessions).tick(Instant::now())
            }
        };
        for action in actions {
            let event = match action {
                Action::HostCommand { channel, data } => {
                    if driver_commands
                        .send(host_frame(channel, data))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Action::Transmit { port, frame } => {
                    if driver_commands
                        .send(TncCommand::HostFrame {
                            channel: port,
                            data: frame,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Action::Event(event) => TncEvent::Session(event),
                Action::Warning(message) => TncEvent::Warning(message),
            };
            if events.send(event).await.is_err() {
                return;
            }
        }
        if modem.runs_kiss() {
            let report = kiss(&mut sessions).buffers();
            if last_buffers.as_ref() != Some(&report) {
                last_buffers = Some(report.clone());
                if events.send(TncEvent::Buffers(report)).await.is_err() {
                    break;
                }
            }
        }
    }
}

enum Modem {
    Kiss,
    Host,
    Switch {
        kiss_ports: BTreeSet<u8>,
        wa8ded: Option<u8>,
    },
}

impl Modem {
    fn from_config(config: &TncConfig) -> Self {
        match config.protocol_kind() {
            HostProtocol::Kiss | HostProtocol::Baycom => Self::Kiss,
            HostProtocol::Tfpcx => {
                let links = config.validated_links().unwrap_or_default();
                Self::Switch {
                    kiss_ports: links
                        .iter()
                        .filter(|link| link.is_kiss())
                        .flat_map(|link| link.ports.iter().copied())
                        .collect(),
                    wa8ded: links
                        .iter()
                        .find(|link| link.is_wa8ded())
                        .and_then(|link| link.ports.first().copied()),
                }
            }
            HostProtocol::Wa8ded => Self::Host,
        }
    }

    /// KISS or BayCom, or a TFPCX switch that actually has a KISS radio.
    /// An all-WA8DED switch does not load the software AX.25 machine.
    fn runs_kiss(&self) -> bool {
        match self {
            Self::Kiss => true,
            Self::Switch { kiss_ports, .. } => !kiss_ports.is_empty(),
            Self::Host => false,
        }
    }

    fn runs_host(&self) -> bool {
        matches!(self, Self::Host | Self::Switch { .. })
    }

    fn is_switch(&self) -> bool {
        matches!(self, Self::Switch { .. })
    }

    fn kiss_frame(&self, port: u8) -> bool {
        match self {
            Self::Kiss => port & 0x80 == 0,
            Self::Switch { kiss_ports, .. } => kiss_ports.contains(&port),
            Self::Host => false,
        }
    }

    fn host_command(&self, host: &HostSessions, command: &SessionCommand) -> bool {
        match self {
            Self::Host => true,
            Self::Kiss => false,
            Self::Switch { wa8ded, .. } => match command {
                SessionCommand::Unproto { port, .. } | SessionCommand::Connect { port, .. } => {
                    *wa8ded == Some(*port)
                }
                SessionCommand::Disconnect { channel } | SessionCommand::Send { channel, .. } => {
                    host.configs.contains_key(channel)
                }
                SessionCommand::Configure(_) => false,
            },
        }
    }
}

fn kiss(sessions: &mut Option<KissSessions>) -> &mut KissSessions {
    sessions
        .as_mut()
        .expect("KISS sessions exist only while a KISS radio is loaded")
}

async fn apply_session(
    modem: &Modem,
    sessions: &mut Option<KissSessions>,
    host: &mut HostSessions,
    driver_commands: &mpsc::Sender<TncCommand>,
    command: SessionCommand,
) -> Vec<Action> {
    if let Modem::Switch { wa8ded, .. } = modem {
        if let SessionCommand::Configure(configs) = &command {
            host.allowed_port = wa8ded.unwrap_or(0);
            let (kiss, host_configs): (Vec<_>, Vec<_>) = configs
                .iter()
                .cloned()
                .partition(|config| *wa8ded != Some(config.port));
            let mut actions = if let Some(sessions) = sessions.as_mut() {
                sessions.command(SessionCommand::Configure(kiss), Instant::now())
            } else {
                Vec::new()
            };
            match host.command(SessionCommand::Configure(host_configs)) {
                Ok(raw) => {
                    for command in raw {
                        if driver_commands.send(command).await.is_err() {
                            return actions;
                        }
                    }
                }
                Err(reason) => actions.push(Action::Event(SessionEvent::Error {
                    channel: None,
                    reason,
                })),
            }
            return actions;
        }
    }
    if modem.host_command(host, &command) {
        let channel = command_channel(&command);
        match host.command(command) {
            Ok(raw) => {
                for command in raw {
                    if driver_commands.send(command).await.is_err() {
                        return Vec::new();
                    }
                }
                Vec::new()
            }
            Err(reason) => vec![Action::Event(SessionEvent::Error { channel, reason })],
        }
    } else if let Some(sessions) = sessions.as_mut() {
        sessions.command(command, Instant::now())
    } else {
        vec![Action::Event(SessionEvent::Error {
            channel: command_channel(&command),
            reason: "no KISS radio is loaded".into(),
        })]
    }
}

#[derive(Default)]
struct HostSessions {
    configs: BTreeMap<u8, ChannelConfig>,
    pending: BTreeMap<u8, (String, Vec<String>)>,
    connected: BTreeMap<u8, String>,
    outgoing: BTreeSet<u8>,
    /// Radio port this adapter will accept. Plain WA8DED stays on 0.
    allowed_port: u8,
}

impl HostSessions {
    fn incoming_limit(&self) -> TncCommand {
        host_frame(
            0,
            format!(
                "Y {}",
                self.configs.keys().next_back().copied().unwrap_or(0)
            )
            .into_bytes(),
        )
    }

    fn restore_after(&self, command: &TncCommand) -> Vec<TncCommand> {
        let payload = match command {
            TncCommand::HostFrame { channel: 0, data } => data.as_slice(),
            TncCommand::TerminalLine(line) => line.trim().trim_start_matches('\x1b').as_bytes(),
            _ => return Vec::new(),
        };
        // Global I/F/N/O reset the identities of idle link blocks in TheFirmware.
        if !matches!(
            payload.first().map(u8::to_ascii_uppercase),
            Some(b'I' | b'F' | b'N' | b'O')
        ) || payload[1..].iter().all(u8::is_ascii_whitespace)
        {
            return Vec::new();
        }
        self.configs
            .values()
            .filter(|c| {
                !self.pending.contains_key(&c.channel) && !self.connected.contains_key(&c.channel)
            })
            .map(|c| host_frame(c.channel, format!("I {}", c.local).into_bytes()))
            .collect()
    }

    fn command(&mut self, command: SessionCommand) -> Result<Vec<TncCommand>, String> {
        let mut raw = Vec::new();
        match command {
            SessionCommand::Unproto { .. } => {
                return Err("structured unproto is currently implemented for KISS only".into())
            }
            SessionCommand::Configure(configs) => {
                let configs = validate_configs(configs)?;
                if configs.values().any(|c| c.port != self.allowed_port) {
                    return Err(
                        "this hostmode adapter has no verified multi-radio port selector".into(),
                    );
                }
                if configs.keys().any(|&ch| ch > 10) {
                    return Err("TNC2 hostmode supports logical channels 1..10".into());
                }
                for ch in self.pending.keys().chain(self.connected.keys()) {
                    if self.configs.get(ch) != configs.get(ch) {
                        return Err(format!("ch{ch}: disconnect before changing its identity"));
                    }
                }
                let changed = self.configs != configs;
                if changed {
                    raw.push(host_frame(0, b"Y 0".to_vec()));
                }
                for c in configs.values() {
                    if self.configs.get(&c.channel) != Some(c) {
                        raw.push(host_frame(c.channel, format!("I {}", c.local).into_bytes()));
                    }
                }
                self.configs = configs;
                if changed {
                    raw.push(self.incoming_limit());
                }
            }
            SessionCommand::Connect {
                channel,
                port,
                local,
                remote,
                via,
            } => {
                let config = self
                    .configs
                    .get(&channel)
                    .ok_or_else(|| format!("ch{channel}: no session configuration"))?;
                if config.port != port || config.local != canonical_call(&local)? {
                    return Err(format!(
                        "ch{channel}: requested endpoint does not match session configuration"
                    ));
                }
                if self.pending.contains_key(&channel) || self.connected.contains_key(&channel) {
                    return Err(format!("ch{channel}: already in use"));
                }
                let remote = canonical_call(&remote)?;
                let via = validate_via(via)?;
                let command = format!("C {remote} {}", via.join(" "))
                    .trim_end()
                    .as_bytes()
                    .to_vec();
                self.pending.insert(channel, (remote, via));
                self.outgoing.insert(channel);
                raw.push(host_frame(channel, command));
            }
            SessionCommand::Disconnect { channel } => {
                raw.push(host_frame(channel, b"D".to_vec()));
            }
            SessionCommand::Send { channel, data, id } => {
                if id.is_some() {
                    return Err(
                        "RF delivery receipts are not yet verified for hostmode file transfers"
                            .into(),
                    );
                }
                if data.is_empty() || data.len() > 256 {
                    return Err("hostmode payload must contain 1..=256 bytes".into());
                }
                if channel != 0 && !self.connected.contains_key(&channel) {
                    return Err(format!("ch{channel}: no established hostmode connection"));
                }
                raw.push(TncCommand::HostData { channel, data });
            }
        }
        Ok(raw)
    }

    fn event(&mut self, event: TncEvent) -> (Option<TncEvent>, Vec<Action>) {
        let TncEvent::HostFrame {
            mut channel,
            code,
            ref payload,
        } = event
        else {
            return (Some(event), Vec::new());
        };
        let text = String::from_utf8_lossy(payload);
        let upper = text.to_ascii_uppercase();
        if code == HostCode::Error {
            self.pending.remove(&channel);
            self.outgoing.remove(&channel);
            return (
                None,
                vec![Action::Event(SessionEvent::Error {
                    channel: (channel != 0).then_some(channel),
                    reason: format!("TNC2 ch{channel}: {}", text.trim()),
                })],
            );
        }
        let status = code == HostCode::LinkStatus;
        if channel == 0 && status {
            channel = tagged_channel(&text).unwrap_or(0);
        }
        if code == HostCode::Data && channel != 0 && !self.configs.contains_key(&channel) {
            return (
                None,
                vec![Action::Warning(format!(
                    "ch{channel}: data received before hostmode channel configuration"
                ))],
            );
        }
        if channel == 0 || !self.configs.contains_key(&channel) {
            return (Some(event), Vec::new());
        }
        if status {
            if [
                "DISCONNECTED",
                "LINK FAIL",
                "BUSY FROM",
                "BUSY FM",
                "FAILURE WITH",
                "RETRY OUT",
                "NO ANSWER",
            ]
            .iter()
            .any(|s| upper.contains(s))
            {
                self.pending.remove(&channel);
                self.connected.remove(&channel);
                self.outgoing.remove(&channel);
                return (
                    None,
                    vec![Action::Event(SessionEvent::Disconnected {
                        channel,
                        reason: text.trim().into(),
                    })],
                );
            }
            if let Some(pos) = upper.find("CONNECTED TO ") {
                let remote = text[pos + "CONNECTED TO ".len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
                let remote = match canonical_call(remote) {
                    Ok(remote) => remote,
                    Err(err) => {
                        return (
                            None,
                            vec![Action::Warning(format!("hostmode link status: {err}"))],
                        )
                    }
                };
                if self
                    .pending
                    .get(&channel)
                    .is_some_and(|(target, _)| target != &remote)
                {
                    self.pending.remove(&channel);
                    self.outgoing.remove(&channel);
                    return (
                        None,
                        vec![
                            Action::HostCommand {
                                channel,
                                data: b"D".to_vec(),
                            },
                            Action::Event(SessionEvent::Error {
                                channel: Some(channel),
                                reason: format!(
                                    "ch{channel}: TNC connected to an unexpected endpoint {remote}"
                                ),
                            }),
                        ],
                    );
                }
                if self.connected.get(&channel) == Some(&remote) {
                    return (None, Vec::new());
                }
                let via = self
                    .pending
                    .remove(&channel)
                    .map(|(_, via)| via)
                    .unwrap_or_default();
                self.connected.insert(channel, remote.clone());
                let config = &self.configs[&channel];
                return (
                    None,
                    vec![Action::Event(SessionEvent::Connected {
                        channel,
                        port: config.port,
                        local: config.local.clone(),
                        remote,
                        via,
                        outgoing: self.outgoing.contains(&channel),
                    })],
                );
            }
        }
        if code == HostCode::Data {
            let mut actions = Vec::new();
            if let Some(remote) = self.connected.get(&channel) {
                actions.push(Action::Event(SessionEvent::Data {
                    channel,
                    local: self.configs[&channel].local.clone(),
                    remote: remote.clone(),
                    data: payload.clone(),
                }));
                return (None, actions);
            }
            return (
                None,
                vec![Action::Warning(format!(
                    "ch{channel}: data received without a known hostmode connection"
                ))],
            );
        }
        (Some(event), Vec::new())
    }

    fn close(&mut self, reason: &str) -> Vec<Action> {
        let channels: BTreeSet<_> = self
            .connected
            .keys()
            .chain(self.pending.keys())
            .copied()
            .collect();
        self.pending.clear();
        self.connected.clear();
        self.outgoing.clear();
        self.configs.clear();
        channels
            .into_iter()
            .map(|channel| {
                Action::Event(SessionEvent::Disconnected {
                    channel,
                    reason: reason.into(),
                })
            })
            .collect()
    }
}

fn host_frame(channel: u8, data: Vec<u8>) -> TncCommand {
    TncCommand::HostFrame { channel, data }
}

fn tagged_channel(text: &str) -> Option<u8> {
    let text = text.trim_start();
    let (text, end) = if let Some(rest) = text.strip_prefix('(') {
        (rest, ')')
    } else {
        (text, ':')
    };
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 || text.as_bytes().get(len).copied()? != end as u8 {
        return None;
    }
    text[..len].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tnc::ax25;

    fn channel(port: u8) -> ChannelConfig {
        ChannelConfig {
            channel: 1,
            port,
            local: "N0ME".into(),
        }
    }

    #[test]
    fn session_hostmode_command_replies_and_payload_text_cannot_change_link_state() {
        let mut host = HostSessions::default();
        let (raw, actions) = host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::Data,
            payload: b"unconfigured".to_vec(),
        });
        assert!(raw.is_none());
        assert!(matches!(&actions[0], Action::Warning(_)));
        host.command(SessionCommand::Configure(vec![channel(0)]))
            .unwrap();
        assert!(
            matches!(&host.restore_after(&host_frame(0, b"I N0NEW".to_vec()))[..],
            [TncCommand::HostFrame { channel: 1, data }] if data == b"I N0ME")
        );
        let connect = SessionCommand::Connect {
            channel: 1,
            port: 0,
            local: "N0ME".into(),
            remote: "N1YOU".into(),
            via: Vec::new(),
        };
        host.command(connect.clone()).unwrap();
        assert!(host
            .restore_after(&host_frame(0, b"I N0NEW".to_vec()))
            .is_empty());
        for code in [HostCode::CommandReply, HostCode::Data] {
            let (_, actions) = host.event(TncEvent::HostFrame {
                channel: 1,
                code,
                payload: b"*** CONNECTED to N1YOU".to_vec(),
            });
            assert!(!actions
                .iter()
                .any(|a| matches!(a, Action::Event(SessionEvent::Connected { .. }))));
            assert!(host.connected.is_empty());
        }
        let (_, actions) = host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::Error,
            payload: b"INVALID CALLSIGN".to_vec(),
        });
        assert!(matches!(
            &actions[0],
            Action::Event(SessionEvent::Error {
                channel: Some(1),
                ..
            })
        ));
        assert!(host.pending.is_empty());
        host.command(connect).unwrap();
        host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::LinkStatus,
            payload: b"*** CONNECTED to N1YOU".to_vec(),
        });
        let (_, actions) = host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::Data,
            payload: b"*** DISCONNECTED fm N1YOU".to_vec(),
        });
        assert!(matches!(
            &actions[0],
            Action::Event(SessionEvent::Data { .. })
        ));
        assert!(host.connected.contains_key(&1));
        host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::LinkStatus,
            payload: b"*** BUSY fm N1YOU".to_vec(),
        });
        assert!(host.connected.is_empty());
    }

    #[test]
    fn session_hostmode_adapter_delegates_l2_and_normalizes_channel_events() {
        let mut host = HostSessions::default();
        let raw = host
            .command(SessionCommand::Configure(vec![channel(0)]))
            .unwrap();
        assert!(matches!(&raw[0], TncCommand::HostFrame { channel: 0, data } if data == b"Y 0"));
        assert!(matches!(&raw[1], TncCommand::HostFrame { channel: 1, data } if data == b"I N0ME"));
        assert!(matches!(&raw[2], TncCommand::HostFrame { channel: 0, data } if data == b"Y 1"));
        let raw = host
            .command(SessionCommand::Connect {
                channel: 1,
                port: 0,
                local: "N0ME".into(),
                remote: "N1YOU".into(),
                via: vec!["N2DIGI".into()],
            })
            .unwrap();
        assert!(
            matches!(&raw[0], TncCommand::HostFrame { channel: 1, data } if data == b"C N1YOU N2DIGI")
        );
        let (raw, actions) = host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::LinkStatus,
            payload: b"*** CONNECTED to N1YOU".to_vec(),
        });
        assert!(raw.is_none());
        assert!(matches!(
            &actions[0],
            Action::Event(SessionEvent::Connected {
                channel: 1,
                outgoing: true,
                via,
                ..
            }) if via == &["N2DIGI"]
        ));
        let (_, actions) = host.event(TncEvent::HostFrame {
            channel: 1,
            code: HostCode::Data,
            payload: b"hello".to_vec(),
        });
        assert!(
            matches!(&actions[0], Action::Event(SessionEvent::Data { channel: 1, data, .. }) if data == b"hello")
        );
        let raw = host
            .command(SessionCommand::Send {
                channel: 1,
                data: b"reply".to_vec(),
                id: None,
            })
            .unwrap();
        assert!(matches!(&raw[0], TncCommand::HostData { channel: 1, data } if data == b"reply"));
        let raw = host
            .command(SessionCommand::Disconnect { channel: 1 })
            .unwrap();
        assert!(matches!(&raw[0], TncCommand::HostFrame { channel: 1, data } if data == b"D"));
        assert!(host
            .command(SessionCommand::Configure(vec![channel(1)]))
            .is_err());
    }

    #[tokio::test]
    async fn session_monitor_forwarding_preserves_unmatched_frames_and_repeated_tx() {
        let (commands_tx, commands_rx) = mpsc::channel(2);
        let (events_tx, mut events_rx) = mpsc::channel(2);
        let (raw_commands_tx, _raw_commands_rx) = mpsc::channel(2);
        let (raw_events_tx, raw_events_rx) = mpsc::channel(2);
        let config = TncConfig {
            protocol: "kiss".into(),
            ..TncConfig::default()
        };
        let service = tokio::spawn(run(
            config,
            commands_rx,
            events_tx,
            raw_commands_tx,
            raw_events_rx,
        ));
        let packets = [
            ax25::build_rr("N9OTHER", "N1TEST", 1, true, false),
            vec![1, 0xDB, 0xC0],
        ];
        let mut expected = Vec::new();
        for port in [0, 3, 12] {
            for packet in &packets {
                for tx in [false, true, true] {
                    expected.push((tx, port, packet.clone()));
                }
            }
        }
        let feed = expected.clone();
        let sender = tokio::spawn(async move {
            for (tx, port, packet) in feed {
                let event = if tx {
                    TncEvent::KissTx {
                        port,
                        frame: packet,
                    }
                } else {
                    TncEvent::HostFrame {
                        channel: port,
                        code: HostCode::Data,
                        payload: packet,
                    }
                };
                raw_events_tx.send(event).await.unwrap();
            }
            raw_events_tx
        });
        let mut observed = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while observed.len() < expected.len() {
                match events_rx.recv().await.unwrap() {
                    TncEvent::KissTx { port, frame } => observed.push((true, port, frame)),
                    TncEvent::HostFrame {
                        channel,
                        code: HostCode::Data,
                        payload,
                    } => {
                        observed.push((false, channel, payload));
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(observed, expected);
        let raw_events_tx = sender.await.unwrap();
        commands_tx.send(TncCommand::Shutdown).await.unwrap();
        drop(raw_events_tx);
        tokio::time::timeout(Duration::from_secs(2), service)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn session_service_routes_gui_commands_and_driver_frames_without_gui_protocol_state() {
        software_session_roundtrip("kiss", 12).await;
    }

    #[tokio::test]
    async fn baycom_uses_software_sessions_and_forwards_rx_tx_and_acknowledgements() {
        software_session_roundtrip("baycom", 0).await;
    }

    async fn software_session_roundtrip(protocol: &str, radio_port: u8) {
        let (commands_tx, commands_rx) = mpsc::channel(16);
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let (raw_commands_tx, mut raw_commands_rx) = mpsc::channel(16);
        let (raw_events_tx, raw_events_rx) = mpsc::channel(16);
        let config = TncConfig {
            protocol: protocol.into(),
            ..TncConfig::default()
        };
        let service = tokio::spawn(run(
            config,
            commands_rx,
            events_tx,
            raw_commands_tx,
            raw_events_rx,
        ));
        commands_tx
            .send(TncCommand::Session(SessionCommand::Configure(vec![
                channel(radio_port),
            ])))
            .await
            .unwrap();
        commands_tx
            .send(TncCommand::Session(SessionCommand::Connect {
                channel: 1,
                port: radio_port,
                local: "N0ME".into(),
                remote: "N1YOU".into(),
                via: Vec::new(),
            }))
            .await
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(2), raw_commands_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(command, TncCommand::HostFrame { channel, data } if channel == radio_port && ax25::parse_frame(&data).unwrap().kind == ax25::Ax25FrameType::Sabm)
        );
        let ua = ax25::build_ua("N0ME", "N1YOU", true);
        raw_events_tx
            .send(TncEvent::HostFrame {
                channel: radio_port,
                code: HostCode::Data,
                payload: ua.clone(),
            })
            .await
            .unwrap();
        let mut monitored_ua = false;
        loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
                .await
                .unwrap()
                .unwrap();
            match event {
                TncEvent::HostFrame {
                    channel, payload, ..
                } => {
                    assert_eq!((channel, payload), (radio_port, ua.clone()));
                    monitored_ua = true;
                }
                TncEvent::Session(SessionEvent::Connected { channel, port, .. }) => {
                    assert_eq!((channel, port), (1, radio_port));
                    assert!(monitored_ua);
                    break;
                }
                _ => {}
            }
        }
        commands_tx
            .send(TncCommand::Session(SessionCommand::Send {
                channel: 1,
                data: b"hello".to_vec(),
                id: Some(81),
            }))
            .await
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(2), raw_commands_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let TncCommand::HostFrame {
            channel: port,
            data,
        } = command
        else {
            panic!("expected KISS frame");
        };
        assert_eq!(port, radio_port);
        assert_eq!(ax25::parse_frame(&data).unwrap().info, b"hello");
        let tx = data.clone();
        raw_events_tx
            .send(TncEvent::KissTx { port, frame: data })
            .await
            .unwrap();
        raw_events_tx
            .send(TncEvent::HostFrame {
                channel: radio_port,
                code: HostCode::Data,
                payload: ax25::build_rr("N0ME", "N1YOU", 1, false, false),
            })
            .await
            .unwrap();
        let (mut monitored_tx, mut monitored_rr) = (false, false);
        loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
                .await
                .unwrap()
                .unwrap();
            match event {
                TncEvent::KissTx { port, frame } => {
                    assert_eq!((port, frame), (radio_port, tx.clone()));
                    monitored_tx = true;
                }
                TncEvent::HostFrame {
                    channel, payload, ..
                } => {
                    assert_eq!(channel, radio_port);
                    assert_eq!(
                        ax25::parse_frame(&payload).unwrap().kind,
                        ax25::Ax25FrameType::Rr
                    );
                    monitored_rr = true;
                }
                TncEvent::Session(SessionEvent::Acknowledged { channel, id }) => {
                    assert_eq!((channel, id), (1, 81));
                    assert!(monitored_tx && monitored_rr);
                    break;
                }
                _ => {}
            }
        }
        commands_tx.send(TncCommand::Shutdown).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), raw_commands_rx.recv())
                .await
                .unwrap(),
            Some(TncCommand::Shutdown)
        ));
        drop(raw_events_tx);
        tokio::time::timeout(Duration::from_secs(2), service)
            .await
            .unwrap()
            .unwrap();
    }
}
