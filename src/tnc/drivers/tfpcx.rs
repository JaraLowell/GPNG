//! TFPCX switch. Up to three KISS or WA8DED modems, each started with the
//! existing driver. Port numbers are rewritten here. The drivers are not modified.
//!
//! A KISS link's hardware ports are 0, 1, … in the order the operator chose.
//! WA8DED stays a single logical port and still receives ordinary hostmode commands.
//!
//! A frame that has to leave on more than one modem is handed to the first
//! modem immediately. Each further modem waits out that transmission, so two
//! transmitters are not keyed together on one supply. The wait is the
//! 1200 bit/s on-air time plus key-up, and at least half a second. Receiving
//! keeps running while a later modem is waiting.

use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::{SerialConfig, TncConfig, TncLink};

use super::super::ax25;
use super::super::hostmode::HostCode;
use super::super::known_hosts::{self, KnownHosts, Route};
use super::super::{TncCommand, TncError, TncEvent};

struct Plugin {
    wa8ded: bool,
    /// Logical ports in hardware order. Wire port 0 is `ports[0]`.
    ports: Vec<u8>,
    commands: mpsc::Sender<TncCommand>,
}

pub fn run(
    serial: SerialConfig,
    tnc_cfg: TncConfig,
    events: mpsc::Sender<TncEvent>,
    mut commands: mpsc::Receiver<TncCommand>,
) -> Result<(), TncError> {
    let links = tnc_cfg.validated_links().map_err(TncError::Protocol)?;
    let mut hosts = KnownHosts::load(known_hosts::default_path(), &tnc_cfg.runtime_own_calls);
    for call in &tnc_cfg.runtime_own_calls {
        hosts.own(call);
    }

    let mut plugins = Vec::new();
    let mut incoming = Vec::new();
    for (index, link) in links.iter().enumerate() {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let slot = index + 1;
        let child_events = event_tx.clone();
        let child_serial = child_serial(&serial, link);
        let child_tnc = child_tnc(&tnc_cfg, link);
        let wa8ded = link.is_wa8ded();
        let join = thread::Builder::new()
            .name(format!("tfpcx-link-{slot}"))
            .spawn(move || {
                let result = if wa8ded {
                    super::wa8ded::run(child_serial, child_tnc, child_events.clone(), cmd_rx)
                } else {
                    super::kiss::run(child_serial, child_tnc, child_events.clone(), cmd_rx)
                };
                if let Err(err) = result {
                    let _ = child_events
                        .blocking_send(TncEvent::Warning(format!("link {slot}: {err}")));
                }
            })
            .map_err(|err| {
                let _ = hosts.save();
                TncError::Protocol(format!("link {slot}: {err}"))
            })?;
        // Detach. Shutdown is a command on the child's channel.
        drop(join);
        let _ = events.blocking_send(TncEvent::Status(link_status(slot, link)));
        plugins.push(Plugin {
            wa8ded,
            ports: link.ports.clone(),
            commands: cmd_tx,
        });
        incoming.push(event_rx);
    }

    let kiss_ports: Vec<u8> = plugins
        .iter()
        .filter(|plugin| !plugin.wa8ded)
        .flat_map(|plugin| plugin.ports.iter().copied())
        .collect();

    let result = supervise(
        &plugins,
        &kiss_ports,
        &mut hosts,
        &events,
        &mut commands,
        &mut incoming,
    );
    let _ = hosts.save();
    result
}

fn supervise(
    plugins: &[Plugin],
    kiss_ports: &[u8],
    hosts: &mut KnownHosts,
    events: &mpsc::Sender<TncEvent>,
    commands: &mut mpsc::Receiver<TncCommand>,
    incoming: &mut [mpsc::Receiver<TncEvent>],
) -> Result<(), TncError> {
    let mut stagger = Stagger::new();
    loop {
        loop {
            match commands.try_recv() {
                Ok(cmd) => {
                    if dispatch(plugins, kiss_ports, hosts, events, &mut stagger, cmd)? {
                        stagger.held.clear();
                        return Ok(());
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    stagger.held.clear();
                    let _ = shutdown(plugins);
                    return Ok(());
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }

        release_held(plugins, events, &mut stagger.held)?;

        for (index, rx) in incoming.iter_mut().enumerate() {
            while let Ok(event) = rx.try_recv() {
                lift(index, &plugins[index], hosts, events, event);
            }
        }
        thread::sleep(stagger_nap(&stagger.held));
    }
}

fn dispatch(
    plugins: &[Plugin],
    kiss_ports: &[u8],
    hosts: &mut KnownHosts,
    events: &mpsc::Sender<TncEvent>,
    stagger: &mut Stagger,
    cmd: TncCommand,
) -> Result<bool, TncError> {
    match cmd {
        TncCommand::Shutdown => {
            shutdown(plugins)?;
            let _ = events.blocking_send(TncEvent::LinkDown("shutdown".into()));
            return Ok(true);
        }
        TncCommand::Reinitialize | TncCommand::QueryTiming => {
            for plugin in plugins {
                let _ = plugin.commands.blocking_send(cmd.clone());
            }
        }
        TncCommand::RefreshSerial { port } => {
            if let Some(plugin) = plugins
                .iter()
                .find(|plugin| !plugin.wa8ded && plugin.ports.contains(&port))
            {
                let _ = plugin
                    .commands
                    .blocking_send(TncCommand::RefreshSerial { port });
            }
        }
        TncCommand::TerminalLine(line) => {
            let targets: Vec<_> = plugins.iter().filter(|plugin| plugin.wa8ded).collect();
            let targets: Vec<_> = if targets.is_empty() {
                plugins.iter().collect()
            } else {
                targets
            };
            for plugin in targets {
                let _ = plugin
                    .commands
                    .blocking_send(TncCommand::TerminalLine(line.clone()));
            }
        }
        TncCommand::KissParam { cmd, arg } => {
            for plugin in plugins.iter().filter(|plugin| !plugin.wa8ded) {
                let _ = plugin
                    .commands
                    .blocking_send(TncCommand::KissParam { cmd, arg });
            }
        }
        TncCommand::HostData { channel, data } => {
            send_wa8ded(plugins, events, TncCommand::HostData { channel, data })?;
        }
        TncCommand::HostFrame { channel, data } if host_command(&data) => {
            send_wa8ded(plugins, events, TncCommand::HostFrame { channel, data })?;
        }
        TncCommand::HostFrame { channel, data } => {
            send_kiss(plugins, kiss_ports, hosts, events, stagger, channel, &data)?;
        }
        TncCommand::KissBroadcast { ports, data } => {
            send_broadcast(plugins, events, stagger, &ports, &data)?;
        }
        TncCommand::Session(_) => {
            return Err(TncError::Protocol("unadapted session command".into()));
        }
    }
    Ok(false)
}

fn send_wa8ded(
    plugins: &[Plugin],
    events: &mpsc::Sender<TncEvent>,
    cmd: TncCommand,
) -> Result<(), TncError> {
    let Some(plugin) = plugins.iter().find(|plugin| plugin.wa8ded) else {
        let _ = events.blocking_send(TncEvent::Warning(
            "TFPCX has no WA8DED link for that command".into(),
        ));
        return Ok(());
    };
    plugin
        .commands
        .blocking_send(cmd)
        .map_err(|_| TncError::Protocol("WA8DED link stopped".into()))
}

fn send_kiss(
    plugins: &[Plugin],
    kiss_ports: &[u8],
    hosts: &mut KnownHosts,
    events: &mpsc::Sender<TncEvent>,
    stagger: &mut Stagger,
    channel: u8,
    data: &[u8],
) -> Result<(), TncError> {
    let Some(frame) = ax25::parse_frame(data) else {
        return send_on_logical(plugins, events, channel, data.to_vec());
    };
    if hosts.is_own(&frame.dest) && frame.via.is_empty() {
        return send_on_logical(plugins, events, channel, data.to_vec());
    }
    let route = hosts.route(&frame.dest, &frame.via, kiss_ports);
    let ports = match route {
        Route::Other(port) => {
            let _ = events.blocking_send(TncEvent::Warning(format!(
                "{} is known on port {port}",
                frame.dest
            )));
            return Ok(());
        }
        Route::Kiss(ports) if ports.is_empty() => {
            let _ = events.blocking_send(TncEvent::Warning(
                "TFPCX has no KISS port for that frame".into(),
            ));
            return Ok(());
        }
        Route::Kiss(ports) => ports,
    };
    if ports.len() > 1 {
        hosts.note_probe(&frame.dest, &frame.via);
    }
    let groups = ports_by_modem(plugins, &ports);
    fan_out_groups(plugins, events, stagger, groups, data)
}

/// Beacon (and the mail line after it). Every listed KISS port gets a copy.
/// A call remembered on one radio does not hide the others, and a serial
/// link and a TCP link are both keyed, one modem after the other.
fn send_broadcast(
    plugins: &[Plugin],
    events: &mpsc::Sender<TncEvent>,
    stagger: &mut Stagger,
    ports: &[u8],
    data: &[u8],
) -> Result<(), TncError> {
    if ports.is_empty() {
        let _ = events.blocking_send(TncEvent::Warning("beacon has no KISS port".into()));
        return Ok(());
    }
    let groups = ports_by_modem(plugins, ports);
    fan_out_groups(plugins, events, stagger, groups, data)
}

fn fan_out_groups(
    plugins: &[Plugin],
    events: &mpsc::Sender<TncEvent>,
    stagger: &mut Stagger,
    groups: Vec<Vec<u8>>,
    data: &[u8],
) -> Result<(), TncError> {
    if groups.len() < 2 {
        for group in groups {
            for port in group {
                send_on_logical(plugins, events, port, data.to_vec())?;
            }
        }
        return Ok(());
    }
    let gap = fanout_gap(data.len());
    let now = Instant::now();
    let mut when = if stagger.free_at > now {
        stagger.free_at
    } else {
        now
    };
    for group in groups {
        if when <= now {
            for port in group {
                send_on_logical(plugins, events, port, data.to_vec())?;
            }
        } else {
            for port in group {
                stagger.held.push(Held {
                    at: when,
                    logical: port,
                    data: data.to_vec(),
                });
            }
        }
        when += gap;
    }
    stagger.free_at = when;
    Ok(())
}

/// One copy of a frame waiting so a later modem does not key yet.
struct Held {
    at: Instant,
    logical: u8,
    data: Vec<u8>,
}

/// Shared-supply clock for frames that leave on more than one modem.
struct Stagger {
    /// When the last staggered copy should have finished keying.
    free_at: Instant,
    held: Vec<Held>,
}

impl Stagger {
    fn new() -> Self {
        Self {
            free_at: Instant::now(),
            held: Vec::new(),
        }
    }
}

/// Logical ports grouped by physical modem, lowest slot first.
///
/// Ports on one modem stay together: that box has one supply already, and
/// its TNC receives the copies on one link. A port with no modem is its own
/// group so the usual "not on a KISS link" warning still happens.
fn ports_by_modem(plugins: &[Plugin], ports: &[u8]) -> Vec<Vec<u8>> {
    let mut groups: Vec<Vec<u8>> = plugins.iter().map(|_| Vec::new()).collect();
    let mut loose = Vec::new();
    for &port in ports {
        match plugins
            .iter()
            .position(|plugin| !plugin.wa8ded && plugin.ports.contains(&port))
        {
            Some(index) => groups[index].push(port),
            None => loose.push(port),
        }
    }
    let mut out: Vec<Vec<u8>> = groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .collect();
    if !loose.is_empty() {
        out.push(loose);
    }
    out
}

/// How long the next modem waits before it is allowed to key.
///
/// 1200 bit/s is the slow packet-radio case a shared supply has to survive.
/// One byte is about 6.7 ms on the air there. A tenth of a millisecond per
/// byte would be a few tens of milliseconds, and the second transmitter
/// would still be keyed for almost the whole first transmission. The radio
/// is already drawing current through TXDELAY, before the first flag, so
/// that key-up is part of the wait. The result is never under half a second.
fn fanout_gap(frame_len: usize) -> Duration {
    const BAUD: u64 = 1200;
    const TXDELAY: Duration = Duration::from_millis(500);
    const FLOOR: Duration = Duration::from_millis(500);
    // FCS and the two HDLC flags are not in the KISS payload. Worst-case
    // bit stuffing is about one extra bit in five.
    let bytes = (frame_len as u64).saturating_add(4).saturating_mul(12) / 10;
    let air_ms = bytes.saturating_mul(8).saturating_mul(1000) / BAUD;
    TXDELAY
        .saturating_add(Duration::from_millis(air_ms))
        .max(FLOOR)
}

fn release_held(
    plugins: &[Plugin],
    events: &mpsc::Sender<TncEvent>,
    held: &mut Vec<Held>,
) -> Result<(), TncError> {
    let now = Instant::now();
    let mut index = 0;
    while index < held.len() {
        if held[index].at <= now {
            let item = held.remove(index);
            send_on_logical(plugins, events, item.logical, item.data)?;
        } else {
            index += 1;
        }
    }
    Ok(())
}

/// Keep polling commands and received frames. A later modem is due on the
/// next tick, or sooner when its wait is almost over.
fn stagger_nap(held: &[Held]) -> Duration {
    let tick = Duration::from_millis(10);
    held.iter()
        .map(|item| item.at.saturating_duration_since(Instant::now()))
        .min()
        .map(|due| due.min(tick))
        .unwrap_or(tick)
}

fn send_on_logical(
    plugins: &[Plugin],
    events: &mpsc::Sender<TncEvent>,
    logical: u8,
    data: Vec<u8>,
) -> Result<(), TncError> {
    let Some(plugin) = plugins
        .iter()
        .find(|plugin| !plugin.wa8ded && plugin.ports.contains(&logical))
    else {
        let _ = events.blocking_send(TncEvent::Warning(format!(
            "port {logical} is not on a KISS link"
        )));
        return Ok(());
    };
    let Some(wire) = known_hosts::wire_port(&plugin.ports, logical) else {
        return Ok(());
    };
    plugin
        .commands
        .blocking_send(TncCommand::HostFrame {
            channel: wire,
            data,
        })
        .map_err(|_| TncError::Protocol(format!("KISS port {logical} stopped")))
}

fn lift(
    index: usize,
    plugin: &Plugin,
    hosts: &mut KnownHosts,
    events: &mpsc::Sender<TncEvent>,
    event: TncEvent,
) {
    match event {
        TncEvent::LinkDown(reason) if reason != "shutdown" => {
            // One modem retrying must not take the other radios down.
            // The driver already reported the reason.
        }
        TncEvent::LinkDown(_) => {}
        TncEvent::LinkIo { tx, .. } => {
            let _ = events.blocking_send(TncEvent::LinkIo {
                tx,
                index: index as u8,
            });
        }
        TncEvent::Modem { label, phase, .. } => {
            let _ = events.blocking_send(TncEvent::Modem {
                index: index as u8,
                label,
                phase,
            });
        }
        TncEvent::KissTx { port, frame } => {
            let Some(logical) = known_hosts::logical_port(&plugin.ports, port) else {
                return;
            };
            let _ = events.blocking_send(TncEvent::KissTx {
                port: logical,
                frame,
            });
        }
        TncEvent::HostFrame {
            channel,
            code: HostCode::Data,
            payload,
        } if !plugin.wa8ded => {
            let Some(logical) = known_hosts::logical_port(&plugin.ports, channel) else {
                return;
            };
            if let Some(frame) = ax25::parse_frame(&payload) {
                // Beacon or any other incoming frame: keep the sending station.
                if !hosts.is_own(&frame.src) {
                    hosts.answer(&frame.src, logical);
                }
            }
            let _ = events.blocking_send(TncEvent::HostFrame {
                channel: logical,
                code: HostCode::Data,
                payload,
            });
        }
        TncEvent::HostFrame {
            channel,
            code,
            payload,
        } if plugin.wa8ded => {
            if matches!(
                code,
                HostCode::Monitor | HostCode::MonitorHeader | HostCode::MonitorData
            ) {
                if let Some(call) = heard_call(&payload) {
                    if let Some(port) = plugin.ports.first().copied() {
                        hosts.remember(&call, port);
                    }
                }
            }
            // Bit 7 keeps a hostmode channel from looking like a KISS port.
            // The session strips it before the hostmode adapter and the UI.
            let _ = events.blocking_send(TncEvent::HostFrame {
                channel: channel | 0x80,
                code,
                payload,
            });
        }
        other => {
            let _ = events.blocking_send(other);
        }
    }
}

fn heard_call(payload: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(payload);
    let (_, rest) = ax25::split_monitor_fm(text.trim())?;
    let call = rest
        .split_whitespace()
        .find(|token| !is_hhmm(token))?
        .trim();
    if call.is_empty() {
        None
    } else {
        Some(call.to_string())
    }
}

fn is_hhmm(token: &str) -> bool {
    let Some((h, m)) = token.split_once(':') else {
        return false;
    };
    h.len() == 2
        && m.len() == 2
        && h.bytes().all(|b| b.is_ascii_digit())
        && m.bytes().all(|b| b.is_ascii_digit())
}

fn host_command(data: &[u8]) -> bool {
    !data.is_empty()
        && data
            .iter()
            .all(|byte| byte.is_ascii_graphic() || byte.is_ascii_whitespace())
}

fn shutdown(plugins: &[Plugin]) -> Result<(), TncError> {
    for plugin in plugins {
        let _ = plugin.commands.blocking_send(TncCommand::Shutdown);
    }
    Ok(())
}

fn child_serial(base: &SerialConfig, link: &TncLink) -> SerialConfig {
    let mut serial = base.clone();
    if !link.serial_port.trim().is_empty() {
        serial.port = link.serial_port.clone();
    }
    if link.baud != 0 {
        serial.baud = link.baud;
    }
    serial.port_detail.clear();
    serial
}

fn child_tnc(base: &TncConfig, link: &TncLink) -> TncConfig {
    let mut cfg = base.clone();
    if link.is_wa8ded() {
        cfg.protocol = "wa8ded".into();
        cfg.kiss_port = 0;
        cfg.channel_ports.clear();
        return cfg;
    }
    cfg.protocol = "kiss".into();
    cfg.kiss_transport = if link.uses_tcp() {
        "tcp".into()
    } else {
        "serial".into()
    };
    cfg.kiss_tcp_host = link.tcp_host.clone();
    cfg.kiss_tcp_port = link.tcp_port;
    cfg.kiss_port = 0;
    cfg.channel_ports = (1..link.ports.len() as u8).collect();
    cfg
}

fn link_status(slot: usize, link: &TncLink) -> String {
    let ports = link
        .ports
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if link.is_wa8ded() {
        format!(
            "link {slot}: WA8DED {} at {} baud, port {ports}",
            link.serial_port, link.baud
        )
    } else if link.uses_tcp() {
        format!(
            "link {slot}: KISS TCP {}:{}, ports {ports}",
            link.tcp_host.trim(),
            link.tcp_port
        )
    } else {
        format!(
            "link {slot}: KISS {} at {} baud, ports {ports}",
            link.serial_port, link.baud
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tokio::sync::mpsc;

    use super::super::super::ax25;
    use super::super::super::known_hosts::KnownHosts;
    use super::super::super::TncCommand;
    use super::{
        fanout_gap, ports_by_modem, release_held, send_broadcast, send_kiss, Plugin, Stagger,
    };

    fn kiss_plugin(ports: &[u8]) -> (Plugin, mpsc::Receiver<TncCommand>) {
        let (commands, rx) = mpsc::channel(8);
        (
            Plugin {
                wa8ded: false,
                ports: ports.to_vec(),
                commands,
            },
            rx,
        )
    }

    fn hosts() -> KnownHosts {
        let path = std::env::temp_dir().join(format!(
            "gpng-tfpcx-stagger-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        KnownHosts::load(path, &[])
    }

    fn take_frame(rx: &mut mpsc::Receiver<TncCommand>) -> (u8, Vec<u8>) {
        let TncCommand::HostFrame { channel, data } = rx.try_recv().expect("frame") else {
            panic!("expected a KISS frame");
        };
        (channel, data)
    }

    #[test]
    fn fanout_gap_is_half_a_second_plus_the_1200_baud_airtime() {
        // 20-byte KISS payload → 24 bytes with FCS and flags, plus 20% stuffing.
        // 28 bytes at 1200 bit/s is 186 ms, on top of a 500 ms key-up.
        assert_eq!(fanout_gap(20).as_millis(), 686);
        // 200 bytes → 244 stuffed bytes → 1626 ms on the air, plus the key-up.
        assert_eq!(fanout_gap(200).as_millis(), 2126);
        assert!(fanout_gap(200) > fanout_gap(20));
        assert!(fanout_gap(0) >= Duration::from_millis(500));
        // A tenth of a millisecond per byte would still be inside the key-up.
        assert!(fanout_gap(200) > Duration::from_millis(500 + 20));
    }

    #[test]
    fn ports_on_one_modem_stay_together_and_slot_order_is_kept() {
        let (first, _a) = kiss_plugin(&[0, 1]);
        let (commands, _wa) = mpsc::channel(1);
        let wa8ded = Plugin {
            wa8ded: true,
            ports: vec![9],
            commands,
        };
        let (third, _c) = kiss_plugin(&[2]);
        let plugins = vec![first, wa8ded, third];
        assert_eq!(
            ports_by_modem(&plugins, &[2, 0, 1]),
            vec![vec![0, 1], vec![2]]
        );
    }

    #[test]
    fn a_frame_for_every_modem_keys_the_first_now_and_holds_the_rest() {
        let (first, mut first_rx) = kiss_plugin(&[0]);
        let (second, mut second_rx) = kiss_plugin(&[1]);
        let plugins = vec![first, second];
        let (events, _events_rx) = mpsc::channel(4);
        let mut stagger = Stagger::new();
        let mut book = hosts();
        let frame = ax25::build_ui("CQ", "N0CALL", &[], b"beacon");
        let gap = fanout_gap(frame.len());

        send_kiss(
            &plugins,
            &[0, 1],
            &mut book,
            &events,
            &mut stagger,
            0,
            &frame,
        )
        .unwrap();

        let (wire, data) = take_frame(&mut first_rx);
        assert_eq!(wire, 0);
        assert_eq!(data, frame);
        assert!(second_rx.try_recv().is_err());
        assert_eq!(stagger.held.len(), 1);
        assert_eq!(stagger.held[0].logical, 1);
        assert_eq!(stagger.held[0].data, frame);
        assert_eq!(
            stagger.free_at,
            stagger.held[0].at.checked_add(gap).unwrap()
        );

        // A second copy must not key while the first copy is still on a radio.
        send_kiss(
            &plugins,
            &[0, 1],
            &mut book,
            &events,
            &mut stagger,
            0,
            &frame,
        )
        .unwrap();
        assert!(first_rx.try_recv().is_err());
        assert_eq!(stagger.held.len(), 3);
        assert!(stagger.held[1].at >= stagger.held[0].at);
        assert!(stagger.held[2].at > stagger.held[1].at);

        for item in &mut stagger.held {
            item.at = Instant::now();
        }
        release_held(&plugins, &events, &mut stagger.held).unwrap();
        assert!(stagger.held.is_empty());
        let (wire, data) = take_frame(&mut second_rx);
        assert_eq!((wire, data), (0, frame.clone()));
        let (wire, data) = take_frame(&mut first_rx);
        assert_eq!((wire, data), (0, frame.clone()));
        let (wire, data) = take_frame(&mut second_rx);
        assert_eq!((wire, data), (0, frame));
    }

    #[test]
    fn one_modem_and_a_known_port_are_not_delayed() {
        let (dual, mut dual_rx) = kiss_plugin(&[0, 1]);
        let plugins = vec![dual];
        let (events, _events_rx) = mpsc::channel(4);
        let mut stagger = Stagger::new();
        let mut book = hosts();
        let frame = ax25::build_ui("CQ", "N0CALL", &[], b"both ports");

        send_kiss(
            &plugins,
            &[0, 1],
            &mut book,
            &events,
            &mut stagger,
            0,
            &frame,
        )
        .unwrap();
        assert!(stagger.held.is_empty());
        assert_eq!(take_frame(&mut dual_rx).0, 0);
        assert_eq!(take_frame(&mut dual_rx).0, 1);

        book.remember("N1YOU", 1);
        let (only, mut only_rx) = kiss_plugin(&[1]);
        let plugins = vec![kiss_plugin(&[0]).0, only];
        let directed = ax25::build_ui("N1YOU", "N0CALL", &[], b"one port");
        send_kiss(
            &plugins,
            &[0, 1],
            &mut book,
            &events,
            &mut stagger,
            0,
            &directed,
        )
        .unwrap();
        assert!(stagger.held.is_empty());
        let (wire, data) = take_frame(&mut only_rx);
        assert_eq!(wire, 0);
        assert_eq!(data, directed);
    }

    #[test]
    fn a_beacon_still_reaches_every_modem_when_the_destination_is_known() {
        let (serial, mut serial_rx) = kiss_plugin(&[0]);
        let (tcp, mut tcp_rx) = kiss_plugin(&[2]);
        let plugins = vec![serial, tcp];
        let (events, _events_rx) = mpsc::channel(4);
        let mut stagger = Stagger::new();
        let mut book = hosts();
        book.remember("CQ", 0);
        let frame = ax25::build_ui("CQ", "N0CALL", &[], b"beacon");

        send_broadcast(&plugins, &events, &mut stagger, &[0, 2], &frame).unwrap();

        let (wire, data) = take_frame(&mut serial_rx);
        assert_eq!((wire, data), (0, frame.clone()));
        assert!(tcp_rx.try_recv().is_err());
        assert_eq!(stagger.held.len(), 1);
        assert_eq!(stagger.held[0].logical, 2);
        for item in &mut stagger.held {
            item.at = Instant::now();
        }
        release_held(&plugins, &events, &mut stagger.held).unwrap();
        let (wire, data) = take_frame(&mut tcp_rx);
        assert_eq!((wire, data), (0, frame));
        assert_eq!(book.port_of("CQ"), Some(0));
    }
}
