//! TFPCX switch. Up to three KISS or WA8DED modems, each started with the
//! existing driver. Port numbers are rewritten here. The drivers are not modified.
//!
//! A KISS link's hardware ports are 0, 1, … in the order the operator chose.
//! WA8DED stays a single logical port and still receives ordinary hostmode commands.

use std::thread;
use std::time::Duration;

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
                    let _ = child_events.blocking_send(TncEvent::Warning(format!(
                        "link {slot}: {err}"
                    )));
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

    let result = supervise(&plugins, &kiss_ports, &mut hosts, &events, &mut commands, &mut incoming);
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
    loop {
        loop {
            match commands.try_recv() {
                Ok(cmd) => {
                    if dispatch(plugins, kiss_ports, hosts, events, cmd)? {
                        return Ok(());
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    let _ = shutdown(plugins);
                    return Ok(());
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }

        for (index, rx) in incoming.iter_mut().enumerate() {
            while let Ok(event) = rx.try_recv() {
                lift(index, &plugins[index], hosts, events, event);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn dispatch(
    plugins: &[Plugin],
    kiss_ports: &[u8],
    hosts: &mut KnownHosts,
    events: &mpsc::Sender<TncEvent>,
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
            send_wa8ded(
                plugins,
                events,
                TncCommand::HostFrame { channel, data },
            )?;
        }
        TncCommand::HostFrame { channel, data } => {
            send_kiss(plugins, kiss_ports, hosts, events, channel, &data)?;
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
    for port in ports {
        send_on_logical(plugins, events, port, data.to_vec())?;
    }
    Ok(())
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
