//! Channel-level interface shared by software AX.25 and hostmode TNCs.
//!
//! A terminal channel is not a radio port. KISS circuits are identified by
//! (port, local callsign/SSID, remote callsign/SSID), never by a peer alone.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use super::ax25::{self, Ax25Frame, Ax25FrameType};
use super::{buffers_from_tx, BufferReport, TxSnap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelConfig {
    pub channel: u8,
    pub port: u8,
    pub local: String,
}

#[derive(Debug, Clone)]
pub enum SessionCommand {
    Configure(Vec<ChannelConfig>),
    Unproto {
        port: u8,
        local: String,
        remote: String,
        via: Vec<String>,
        data: Vec<u8>,
    },
    /// Beacon UI on every listed KISS port. Not the monitor's one-port unproto:
    /// TFPCX must not fold this onto the known-host radio.
    UnprotoBroadcast {
        ports: Vec<u8>,
        local: String,
        remote: String,
        via: Vec<String>,
        data: Vec<u8>,
    },
    Connect {
        channel: u8,
        port: u8,
        local: String,
        remote: String,
        via: Vec<String>,
    },
    Disconnect {
        channel: u8,
    },
    Send {
        channel: u8,
        data: Vec<u8>,
        /// Application receipt, independent of wrapping AX.25 sequence numbers.
        id: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Error {
        channel: Option<u8>,
        reason: String,
    },
    Connected {
        channel: u8,
        port: u8,
        local: String,
        remote: String,
        via: Vec<String>,
        outgoing: bool,
    },
    Disconnected {
        channel: u8,
        reason: String,
    },
    Data {
        channel: u8,
        local: String,
        remote: String,
        data: Vec<u8>,
    },
    Acknowledged {
        channel: u8,
        id: u64,
    },
    Reset {
        channel: u8,
        reason: String,
    },
}

pub(super) enum Action {
    Transmit {
        port: u8,
        frame: Vec<u8>,
    },
    /// One UI frame, delivered on each port. The switch does not reroute it.
    Broadcast {
        ports: Vec<u8>,
        frame: Vec<u8>,
    },
    HostCommand {
        channel: u8,
        data: Vec<u8>,
    },
    Event(SessionEvent),
    Warning(String),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Settings {
    pub t1: Duration,
    pub t3: Duration,
    pub retries: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            t1: Duration::from_secs(5),
            t3: Duration::from_secs(300),
            retries: 10,
        }
    }
}

impl Settings {
    pub fn from_config(config: &crate::config::TncConfig) -> Self {
        let timing = super::timing::timing_from_tnc_config(config);
        Self {
            t1: Duration::from_secs(u64::from(timing.frack.unwrap_or(5).max(1))),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Connecting,
    Connected,
    Disconnecting,
}

struct Packet {
    data: Vec<u8>,
    id: Option<u64>,
}

struct Inflight {
    packet: Packet,
    ns: u8,
    sent: Instant,
    written: bool,
}

struct Link {
    config: ChannelConfig,
    remote: String,
    via: Vec<String>,
    state: State,
    vs: u8,
    va: u8,
    vr: u8,
    busy: bool,
    reject_sent: bool,
    received: bool,
    queued: VecDeque<Packet>,
    inflight: Option<Inflight>,
    /// Connection/disconnection retry clock, or an outstanding RR(P).
    timer: Option<Instant>,
    retries: u32,
    activity: Instant,
    local_peer: Option<u8>,
    awaiting_write: bool,
}

impl Link {
    fn new(config: ChannelConfig, remote: String, via: Vec<String>, now: Instant) -> Self {
        Self {
            config,
            remote,
            via,
            state: State::Connecting,
            vs: 0,
            va: 0,
            vr: 0,
            busy: false,
            reject_sent: false,
            received: false,
            queued: VecDeque::new(),
            inflight: None,
            timer: Some(now),
            retries: 0,
            activity: now,
            local_peer: None,
            awaiting_write: true,
        }
    }

    fn reset(&mut self, now: Instant) {
        self.vs = 0;
        self.va = 0;
        self.vr = 0;
        self.busy = false;
        self.reject_sent = false;
        self.received = false;
        self.queued.clear();
        self.inflight = None;
        self.timer = None;
        self.retries = 0;
        self.activity = now;
        self.awaiting_write = false;
    }

    /// I frames the peer has not acknowledged, oldest first.
    ///
    /// A SABM starts the sequence numbers again at 0. Those frames were never
    /// accepted, so they are sent again instead of being dropped.
    fn take_unacked(&mut self) -> VecDeque<Packet> {
        let mut pending = VecDeque::new();
        if let Some(cur) = self.inflight.take() {
            pending.push_back(cur.packet);
        }
        pending.append(&mut self.queued);
        pending
    }

    fn tx(&self, frame: Vec<u8>, out: &mut Vec<Action>) {
        out.push(Action::Transmit {
            port: self.config.port,
            frame,
        });
    }

    fn sabm(&self, out: &mut Vec<Action>) {
        let via = refs(&self.via);
        self.tx(
            ax25::build_sabm_via(&self.remote, &self.config.local, &via),
            out,
        );
    }

    fn disc(&self, out: &mut Vec<Action>) {
        let via = refs(&self.via);
        self.tx(
            ax25::build_disc_via(&self.remote, &self.config.local, &via),
            out,
        );
    }

    fn rr(&self, pf: bool, command: bool, out: &mut Vec<Action>) {
        self.tx(
            ax25::build_rr_via(
                &self.remote,
                &self.config.local,
                self.vr,
                pf,
                command,
                &refs(&self.via),
            ),
            out,
        );
    }

    fn connected(&self, outgoing: bool, out: &mut Vec<Action>) {
        out.push(Action::Event(SessionEvent::Connected {
            channel: self.config.channel,
            port: self.config.port,
            local: self.config.local.clone(),
            remote: self.remote.clone(),
            via: self.via.clone(),
            outgoing,
        }));
    }

    fn retransmit(&mut self, now: Instant, out: &mut Vec<Action>) {
        if let Some(cur) = &mut self.inflight {
            cur.sent = now;
            self.awaiting_write = true;
            let frame = ax25::build_i_via(
                &self.remote,
                &self.config.local,
                cur.ns,
                self.vr,
                false,
                &cur.packet.data,
                &refs(&self.via),
            );
            self.tx(frame, out);
        }
    }

    fn send_next(&mut self, now: Instant, out: &mut Vec<Action>) {
        if self.state != State::Connected
            || self.busy
            || self.timer.is_some()
            || self.inflight.is_some()
        {
            return;
        }
        let Some(packet) = self.queued.pop_front() else {
            return;
        };
        let ns = self.vs;
        self.vs = (self.vs + 1) & 7;
        let frame = ax25::build_i_via(
            &self.remote,
            &self.config.local,
            ns,
            self.vr,
            false,
            &packet.data,
            &refs(&self.via),
        );
        self.inflight = Some(Inflight {
            packet,
            ns,
            sent: now,
            written: false,
        });
        self.awaiting_write = true;
        self.activity = now;
        self.tx(frame, out);
    }

    fn acknowledge(&mut self, nr: u8, out: &mut Vec<Action>) -> bool {
        if nr != self.va && nr != self.vs {
            return false;
        }
        if nr == self.vs {
            if self.inflight.as_ref().is_some_and(|cur| !cur.written) {
                return false;
            }
            if let Some(cur) = self.inflight.take() {
                self.awaiting_write = false;
                if let Some(id) = cur.packet.id {
                    out.push(Action::Event(SessionEvent::Acknowledged {
                        channel: self.config.channel,
                        id,
                    }));
                }
            }
            self.va = nr;
            if self.timer.is_none() {
                self.retries = 0;
            }
        }
        true
    }

    fn recover(&mut self, now: Instant, reason: &str, out: &mut Vec<Action>) {
        self.reset(now);
        self.state = State::Connecting;
        self.timer = Some(now);
        self.awaiting_write = true;
        out.push(Action::Event(SessionEvent::Reset {
            channel: self.config.channel,
            reason: reason.into(),
        }));
        self.sabm(out);
    }
}

pub(super) struct KissSessions {
    configs: BTreeMap<u8, ChannelConfig>,
    links: BTreeMap<u8, Link>,
    settings: Settings,
    /// TFPCX: a free channel may answer a SABM on any KISS radio.
    any_kiss_port: bool,
}

impl KissSessions {
    pub fn new(settings: Settings) -> Self {
        Self {
            configs: BTreeMap::new(),
            links: BTreeMap::new(),
            settings,
            any_kiss_port: false,
        }
    }

    /// A connect request on one TFPCX radio can use a channel set up for another.
    pub fn listen_on_every_kiss_port(&mut self) {
        self.any_kiss_port = true;
    }

    pub fn command(&mut self, command: SessionCommand, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        let channel = command_channel(&command);
        if let Err(err) = self.handle_command(command, now, &mut out) {
            out.push(Action::Event(SessionEvent::Error {
                channel,
                reason: err,
            }));
        }
        out
    }

    fn handle_command(
        &mut self,
        command: SessionCommand,
        now: Instant,
        out: &mut Vec<Action>,
    ) -> Result<(), String> {
        match command {
            SessionCommand::Unproto {
                port,
                local,
                remote,
                via,
                data,
            } => {
                if port > 15 || data.len() > 256 {
                    return Err(
                        "unproto requires radio port 0..15 and at most 256 information bytes"
                            .into(),
                    );
                }
                let local = canonical_call(&local)?;
                let remote = canonical_call(&remote)?;
                let via = validate_via(via)?;
                out.push(Action::Transmit {
                    port,
                    frame: ax25::build_ui(&remote, &local, &refs(&via), &data),
                });
            }
            SessionCommand::UnprotoBroadcast {
                mut ports,
                local,
                remote,
                via,
                data,
            } => {
                if ports.is_empty() || ports.iter().any(|port| *port > 15) || data.len() > 256 {
                    return Err(
                        "beacon requires radio ports 0..15 and at most 256 information bytes"
                            .into(),
                    );
                }
                ports.sort_unstable();
                ports.dedup();
                let local = canonical_call(&local)?;
                let remote = canonical_call(&remote)?;
                let via = validate_via(via)?;
                out.push(Action::Broadcast {
                    ports,
                    frame: ax25::build_ui(&remote, &local, &refs(&via), &data),
                });
            }
            SessionCommand::Configure(configs) => {
                let configs = validate_configs(configs)?;
                for (&ch, link) in &self.links {
                    if configs.get(&ch) != Some(&link.config) {
                        return Err(format!(
                            "ch{ch}: disconnect before changing its callsign or radio port"
                        ));
                    }
                }
                self.configs = configs;
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
                    .ok_or_else(|| format!("ch{channel}: no session configuration"))?
                    .clone();
                if port != config.port || canonical_call(&local)? != config.local {
                    return Err(format!(
                        "ch{channel}: requested endpoint does not match session configuration"
                    ));
                }
                if self.links.contains_key(&channel) {
                    return Err(format!("ch{channel}: already in use"));
                }
                let remote = canonical_call(&remote)?;
                let via = validate_via(via)?;
                if self.find(config.port, &config.local, &remote).is_some() {
                    return Err("this exact radio port and callsign pair is already in use".into());
                }
                if config.local == remote {
                    return Err("self-connect needs different local and remote SSIDs".into());
                }
                let mut link = Link::new(config.clone(), remote.clone(), via.clone(), now);
                let local = via
                    .is_empty()
                    .then(|| {
                        self.configs
                            .values()
                            .find(|c| {
                                c.channel != channel
                                    && c.port == config.port
                                    && c.local == remote
                                    && !self.links.contains_key(&c.channel)
                            })
                            .cloned()
                    })
                    .flatten();
                if let Some(answer) = local {
                    let mut other =
                        Link::new(answer.clone(), config.local.clone(), Vec::new(), now);
                    link.reset(now);
                    other.reset(now);
                    link.state = State::Connected;
                    other.state = State::Connected;
                    link.local_peer = Some(answer.channel);
                    other.local_peer = Some(channel);
                    link.connected(true, out);
                    other.connected(false, out);
                    self.links.insert(answer.channel, other);
                } else {
                    if via.is_empty() && self.owns(config.port, &remote) {
                        return Err("all local channels for this endpoint are busy".into());
                    }
                    link.sabm(out);
                }
                self.links.insert(channel, link);
            }
            SessionCommand::Disconnect { channel } => {
                let Some(mut link) = self.links.remove(&channel) else {
                    return Err(format!("ch{channel}: no connection to disconnect"));
                };
                link.queued.clear();
                link.inflight = None;
                if let Some(other) = link.local_peer {
                    self.links.remove(&other);
                    disconnected(other, "local disconnect", out);
                    disconnected(channel, "local disconnect", out);
                } else if link.state == State::Connecting {
                    link.disc(out);
                    disconnected(channel, "connection attempt cancelled", out);
                } else {
                    link.state = State::Disconnecting;
                    link.timer = Some(now);
                    link.awaiting_write = true;
                    link.retries = 0;
                    link.disc(out);
                    self.links.insert(channel, link);
                }
            }
            SessionCommand::Send { channel, data, id } => {
                let link = self
                    .links
                    .get_mut(&channel)
                    .ok_or_else(|| format!("ch{channel}: not connected"))?;
                if link.state != State::Connected {
                    return Err(format!("ch{channel}: not connected"));
                }
                if data.len() > 256 {
                    return Err(format!("ch{channel}: AX.25 information exceeds 256 bytes"));
                }
                if let Some(other) = link.local_peer {
                    let local = link.config.local.clone();
                    let remote = link.remote.clone();
                    out.push(Action::Event(SessionEvent::Data {
                        channel: other,
                        local: remote,
                        remote: local,
                        data,
                    }));
                    if let Some(id) = id {
                        out.push(Action::Event(SessionEvent::Acknowledged { channel, id }));
                    }
                } else {
                    link.queued.push_back(Packet { data, id });
                    link.send_next(now, out);
                }
            }
        }
        Ok(())
    }

    fn find(&self, port: u8, local: &str, remote: &str) -> Option<u8> {
        self.links.iter().find_map(|(&ch, l)| {
            (l.config.port == port && l.config.local == local && l.remote == remote).then_some(ch)
        })
    }

    pub fn receive(&mut self, port: u8, payload: &[u8], now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        let Some(frame) = ax25::parse_frame(payload) else {
            return out;
        };
        let ours: Vec<_> = self
            .configs
            .values()
            .filter(|c| self.any_kiss_port || c.port == port)
            .map(|c| c.local.clone())
            .collect();
        if let Some(repeated) = ax25::repeat_if_ours(payload, &ours) {
            out.push(Action::Transmit {
                port,
                frame: repeated.clone(),
            });
            out.extend(self.receive(port, &repeated, now));
            return out;
        }
        if !frame.path_complete() {
            return out;
        }
        let channel = self.find(port, &frame.dest, &frame.src);
        if frame.kind == Ax25FrameType::Sabm && frame.command {
            if let Some(ch) = channel {
                let link = self.links.get_mut(&ch).expect("matched link");
                if link.state == State::Disconnecting || link.local_peer.is_some() {
                    reply(&frame, port, false, &mut out);
                    return out;
                }
                let was_connected = link.state == State::Connected;
                // The peer's SABM never acknowledged the I frames already
                // sent. Keep them and transmit again from N(S) = 0.
                let unacked = if was_connected {
                    link.take_unacked()
                } else {
                    VecDeque::new()
                };
                link.reset(now);
                link.queued = unacked;
                link.state = State::Connected;
                link.via = ax25::reply_via(&frame.via);
                reply(&frame, port, true, &mut out);
                if was_connected {
                    out.push(Action::Event(SessionEvent::Reset {
                        channel: ch,
                        reason: "peer reset the link".into(),
                    }));
                    link.connected(true, &mut out);
                } else {
                    link.connected(true, &mut out);
                }
                link.send_next(now, &mut out);
            } else if let Some(config) = self.incoming_config(port, &frame.dest) {
                let mut link = Link::new(
                    config.clone(),
                    frame.src.clone(),
                    ax25::reply_via(&frame.via),
                    now,
                );
                link.reset(now);
                link.state = State::Connected;
                reply(&frame, port, true, &mut out);
                link.connected(false, &mut out);
                self.links.insert(config.channel, link);
            } else if self.owns(port, &frame.dest) {
                reply(&frame, port, false, &mut out);
            }
            return out;
        }
        let Some(ch) = channel else {
            if self.owns(port, &frame.dest)
                && frame.command
                && (frame.kind == Ax25FrameType::Disc
                    || (frame.poll && frame.kind != Ax25FrameType::Sabm))
            {
                reply(&frame, port, false, &mut out);
            }
            return out;
        };
        let link = self.links.get_mut(&ch).expect("matched link");
        if link.local_peer.is_some() {
            return out;
        }
        if frame.kind == Ax25FrameType::Disc && frame.command {
            let ua = link.state != State::Connecting;
            reply(&frame, port, ua, &mut out);
            self.links.remove(&ch);
            disconnected(ch, "peer disconnected", &mut out);
            return out;
        }
        if frame.kind == Ax25FrameType::Dm
            && !frame.command
            && (frame.poll || link.state == State::Connected)
        {
            self.links.remove(&ch);
            disconnected(ch, "peer refused or closed the link", &mut out);
            return out;
        }
        if frame.kind == Ax25FrameType::Ua && !frame.command && frame.poll {
            match link.state {
                State::Connecting => {
                    link.reset(now);
                    link.state = State::Connected;
                    link.connected(true, &mut out);
                }
                State::Disconnecting => {
                    self.links.remove(&ch);
                    disconnected(ch, "disconnect acknowledged", &mut out);
                }
                State::Connected => {}
            }
            return out;
        }
        if link.state != State::Connected {
            return out;
        }
        if frame.kind == Ax25FrameType::Frmr && !frame.command {
            link.recover(now, "peer rejected a frame", &mut out);
            return out;
        }
        if !matches!(
            frame.kind,
            Ax25FrameType::Info | Ax25FrameType::Rr | Ax25FrameType::Rnr | Ax25FrameType::Rej
        ) || (frame.kind == Ax25FrameType::Info && !frame.command)
        {
            return out;
        }
        if frame.info.len() > 256 {
            link.recover(now, "AX.25 information exceeds the receive limit", &mut out);
            return out;
        }
        if !link.acknowledge(frame.nr, &mut out) {
            link.recover(now, "invalid acknowledgement sequence", &mut out);
            return out;
        }
        link.activity = now;
        if frame.kind == Ax25FrameType::Info {
            if frame.ns == link.vr {
                link.vr = (link.vr + 1) & 7;
                link.reject_sent = false;
                link.received = true;
                out.push(Action::Event(SessionEvent::Data {
                    channel: ch,
                    local: link.config.local.clone(),
                    remote: link.remote.clone(),
                    data: frame.info,
                }));
                link.rr(frame.poll, false, &mut out);
            } else if link.received && frame.ns == (link.vr + 7) & 7 {
                link.rr(frame.poll, false, &mut out);
            } else if !link.reject_sent {
                link.reject_sent = true;
                link.tx(
                    ax25::build_rej_via(
                        &link.remote,
                        &link.config.local,
                        link.vr,
                        frame.poll,
                        &refs(&link.via),
                    ),
                    &mut out,
                );
            } else if frame.poll {
                link.rr(true, false, &mut out);
            }
        } else {
            link.busy = frame.kind == Ax25FrameType::Rnr;
            let final_reply = !frame.command && frame.poll && link.timer.is_some();
            if final_reply {
                link.timer = None;
                link.awaiting_write = false;
                link.retries = 0;
            }
            if frame.command && frame.poll {
                link.rr(true, false, &mut out);
            }
            if !link.busy && (frame.kind == Ax25FrameType::Rej || final_reply) {
                link.retransmit(now, &mut out);
            }
        }
        link.send_next(now, &mut out);
        out
    }

    fn owns(&self, port: u8, call: &str) -> bool {
        self.configs
            .values()
            .any(|c| (self.any_kiss_port || c.port == port) && c.local == call)
    }

    /// Free channel for an incoming SABM. TFPCX may move it onto the radio that called.
    fn incoming_config(&self, port: u8, local: &str) -> Option<ChannelConfig> {
        let free = |c: &ChannelConfig| c.local == local && !self.links.contains_key(&c.channel);
        if let Some(config) = self
            .configs
            .values()
            .find(|c| c.port == port && free(c))
            .cloned()
        {
            return Some(config);
        }
        if !self.any_kiss_port {
            return None;
        }
        self.configs.values().find(|c| free(c)).map(|c| {
            let mut config = c.clone();
            config.port = port;
            config
        })
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        let mut failed = Vec::new();
        for (&ch, link) in &mut self.links {
            if link.local_peer.is_some() || link.awaiting_write {
                continue;
            }
            let due = if let Some(sent) = link.timer {
                now.duration_since(sent) >= self.settings.t1
            } else if link.state == State::Connected {
                link.inflight
                    .as_ref()
                    .is_some_and(|p| now.duration_since(p.sent) >= self.settings.t1)
                    || now.duration_since(link.activity)
                        >= if link.busy {
                            self.settings.t1
                        } else {
                            self.settings.t3
                        }
            } else {
                false
            };
            if !due {
                continue;
            }
            if link.retries >= self.settings.retries {
                failed.push(ch);
                continue;
            }
            link.retries += 1;
            link.timer = Some(now);
            link.awaiting_write = true;
            match link.state {
                State::Connecting => link.sabm(&mut out),
                State::Disconnecting => link.disc(&mut out),
                State::Connected => link.rr(true, true, &mut out),
            }
        }
        for ch in failed {
            self.links.remove(&ch);
            disconnected(ch, "AX.25 retry limit exceeded", &mut out);
        }
        out
    }

    /// Move a circuit onto the port that answered. Used by TFPCX only.
    /// Standalone KISS keeps the port it was opened on.
    pub fn adopt_port(&mut self, port: u8, payload: &[u8]) -> bool {
        let Some(frame) = ax25::parse_frame(payload) else {
            return false;
        };
        if self.find(port, &frame.dest, &frame.src).is_some() {
            return false;
        }
        let Some(channel) = self.links.iter().find_map(|(&channel, link)| {
            (link.config.local == frame.dest
                && link.remote == frame.src
                && link.config.port != port)
                .then_some(channel)
        }) else {
            return false;
        };
        if let Some(link) = self.links.get_mut(&channel) {
            link.config.port = port;
        }
        if let Some(config) = self.configs.get_mut(&channel) {
            config.port = port;
        }
        true
    }

    /// Credit a write to the circuit with these calls, whichever port carried it.
    /// TFPCX may probe more than one port before the answer locks the circuit.
    pub fn credit_transmit(&mut self, payload: &[u8], now: Instant) {
        let Some(frame) = ax25::parse_frame(payload) else {
            return;
        };
        let Some(port) = self.links.iter().find_map(|(_, link)| {
            (link.config.local == frame.src && link.remote == frame.dest)
                .then_some(link.config.port)
        }) else {
            return;
        };
        self.transmitted(port, payload, now);
    }

    pub fn transmitted(&mut self, port: u8, payload: &[u8], now: Instant) {
        let Some(frame) = ax25::parse_frame(payload) else {
            return;
        };
        let Some(ch) = self.find(port, &frame.src, &frame.dest) else {
            return;
        };
        let link = self.links.get_mut(&ch).expect("matched transmitted link");
        link.activity = now;
        let timed_control = match frame.kind {
            Ax25FrameType::Sabm => link.state == State::Connecting,
            Ax25FrameType::Disc => link.state == State::Disconnecting,
            Ax25FrameType::Rr => frame.command && frame.poll && link.timer.is_some(),
            _ => false,
        };
        if timed_control {
            link.timer = Some(now);
            link.awaiting_write = false;
        } else if frame.kind == Ax25FrameType::Info {
            if let Some(cur) = link.inflight.as_mut().filter(|cur| cur.ns == frame.ns) {
                cur.sent = now;
                cur.written = true;
                link.awaiting_write = false;
            }
        }
    }

    pub fn close(&mut self, reason: &str) -> Vec<Action> {
        let mut out = Vec::new();
        for ch in self.links.keys().copied() {
            disconnected(ch, reason, &mut out);
        }
        self.links.clear();
        out
    }

    /// A circuit that is connecting, connected, or disconnecting.
    /// An empty `ports` set means every circuit on this modem.
    pub fn busy_on(&self, ports: &BTreeSet<u8>) -> bool {
        self.links
            .values()
            .any(|link| ports.is_empty() || ports.contains(&link.config.port))
    }

    /// Drop circuits on `ports` without sending DISC. The COM port is about to go away.
    pub fn close_on(&mut self, ports: &BTreeSet<u8>, reason: &str) -> Vec<Action> {
        let channels: Vec<u8> = self
            .links
            .iter()
            .filter(|(_, link)| ports.is_empty() || ports.contains(&link.config.port))
            .map(|(channel, _)| *channel)
            .collect();
        let mut out = Vec::new();
        for channel in channels {
            disconnected(channel, reason, &mut out);
            self.links.remove(&channel);
        }
        out
    }

    pub fn buffers(&self) -> BufferReport {
        let snaps: Vec<_> = self
            .links
            .values()
            .map(|l| {
                let pending = l.inflight.as_ref().filter(|p| !p.written);
                TxSnap {
                    channel: l.config.channel,
                    peer: l.remote.clone(),
                    not_sent: l.queued.len() as u32 + u32::from(pending.is_some()),
                    unack: u32::from(l.inflight.as_ref().is_some_and(|p| p.written)),
                    tries: l.retries,
                    waiting_bytes: l.queued.iter().map(|p| p.data.len() as u32).sum::<u32>()
                        + pending.map_or(0, |p| p.packet.data.len() as u32),
                }
            })
            .collect();
        buffers_from_tx(&snaps)
    }
}

fn disconnected(channel: u8, reason: &str, out: &mut Vec<Action>) {
    out.push(Action::Event(SessionEvent::Disconnected {
        channel,
        reason: reason.into(),
    }));
}

fn reply(frame: &Ax25Frame, port: u8, ua: bool, out: &mut Vec<Action>) {
    let via = ax25::reply_via(&frame.via);
    let frame = if ua {
        ax25::build_ua_via(&frame.src, &frame.dest, frame.poll, &refs(&via))
    } else {
        ax25::build_dm_via(&frame.src, &frame.dest, frame.poll, &refs(&via))
    };
    out.push(Action::Transmit { port, frame });
}

fn refs(calls: &[String]) -> Vec<&str> {
    calls.iter().map(String::as_str).collect()
}

pub(super) fn canonical_call(call: &str) -> Result<String, String> {
    let call = crate::config::normalize_mycall(call)?;
    Ok(call.strip_suffix("-0").unwrap_or(&call).to_string())
}

pub(super) fn command_channel(command: &SessionCommand) -> Option<u8> {
    match command {
        SessionCommand::Connect { channel, .. }
        | SessionCommand::Send { channel, .. }
        | SessionCommand::Disconnect { channel } => Some(*channel),
        _ => None,
    }
}

pub(super) fn validate_configs(
    configs: Vec<ChannelConfig>,
) -> Result<BTreeMap<u8, ChannelConfig>, String> {
    let mut result = BTreeMap::new();
    for mut config in configs {
        if config.channel == 0 || config.port > 15 {
            return Err("session channel must be nonzero and KISS radio port must be 0..15".into());
        }
        config.local = canonical_call(&config.local)?;
        let ch = config.channel;
        if result.insert(ch, config).is_some() {
            return Err(format!("duplicate session channel {ch}"));
        }
    }
    Ok(result)
}

pub(super) fn validate_via(via: Vec<String>) -> Result<Vec<String>, String> {
    if via.len() > 8 {
        return Err("AX.25 supports at most eight digipeaters".into());
    }
    via.iter().map(|v| canonical_call(v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(configs: &[(u8, u8, &str)]) -> KissSessions {
        let mut engine = KissSessions::new(Settings {
            t1: Duration::from_secs(2),
            t3: Duration::from_secs(30),
            retries: 2,
        });
        let out = engine.command(
            SessionCommand::Configure(
                configs
                    .iter()
                    .map(|&(channel, port, local)| ChannelConfig {
                        channel,
                        port,
                        local: local.into(),
                    })
                    .collect(),
            ),
            Instant::now(),
        );
        assert!(out.is_empty());
        engine
    }

    fn frames(actions: &[Action]) -> Vec<(u8, Ax25Frame)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Transmit { port, frame } => {
                    Some((*port, ax25::parse_frame(frame).unwrap()))
                }
                _ => None,
            })
            .collect()
    }

    fn events(actions: &[Action]) -> Vec<&SessionEvent> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Event(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    fn connect(engine: &mut KissSessions, channel: u8, port: u8, remote: &str, now: Instant) {
        let local = engine.configs[&channel].local.clone();
        let out = engine.command(
            SessionCommand::Connect {
                channel,
                port,
                local: local.clone(),
                remote: remote.into(),
                via: Vec::new(),
            },
            now,
        );
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Sabm);
        written(engine, &out, now);
        let out = engine.receive(port, &ax25::build_ua(&local, remote, true), now);
        assert!(
            matches!(events(&out).as_slice(), [SessionEvent::Connected { channel: ch, .. }] if *ch == channel)
        );
    }

    fn send(engine: &mut KissSessions, channel: u8, id: u64, now: Instant) -> Vec<Action> {
        let out = engine.command(
            SessionCommand::Send {
                channel,
                data: vec![id as u8],
                id: Some(id),
            },
            now,
        );
        written(engine, &out, now);
        out
    }

    fn written(engine: &mut KissSessions, actions: &[Action], now: Instant) {
        for action in actions {
            if let Action::Transmit { port, frame } = action {
                engine.transmitted(*port, frame, now);
            }
        }
    }

    #[test]
    fn session_ports_and_exact_ssids_isolate_acknowledgements_and_counters() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME"), (2, 1, "N0ME"), (3, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        connect(&mut e, 2, 1, "N1YOU", now);
        connect(&mut e, 3, 0, "N1YOU-1", now);
        for ch in 1..=3 {
            send(&mut e, ch, u64::from(ch), now);
            send(&mut e, ch, 10 + u64::from(ch), now);
        }
        assert!(e
            .receive(2, &ax25::build_rr("N0ME", "N1YOU", 1, false, false), now)
            .is_empty());
        assert!(e
            .receive(0, &ax25::build_rr("N0ME-1", "N1YOU", 1, false, false), now)
            .is_empty());
        let out = e.receive(1, &ax25::build_rr("N0ME", "N1YOU", 1, false, false), now);
        assert_eq!(
            events(&out),
            vec![&SessionEvent::Acknowledged { channel: 2, id: 2 }]
        );
        assert_eq!(frames(&out)[0].0, 1);
        written(&mut e, &out, now);
        let rows = e.buffers().rows;
        assert_eq!(
            rows.iter()
                .map(|r| (r.channel, r.not_sent, r.unack))
                .collect::<Vec<_>>(),
            vec![(1, 1, 1), (2, 0, 1), (3, 1, 1)]
        );
    }

    #[test]
    fn session_transit_frames_cannot_acknowledge_before_the_last_digipeater() {
        let now = Instant::now();
        let mut e = setup(&[(1, 4, "N0ME")]);
        connect(&mut e, 1, 4, "N1YOU", now);
        send(&mut e, 1, 1, now);
        send(&mut e, 1, 2, now);
        let mut rr = ax25::build_rr_via("N0ME", "N1YOU", 1, false, false, &["N2DIGI"]);
        assert!(e.receive(4, &rr, now).is_empty());
        assert_eq!(e.buffers().rows[0].not_sent, 1);
        rr[20] |= 0x80;
        let out = e.receive(4, &rr, now);
        assert_eq!(
            events(&out),
            vec![&SessionEvent::Acknowledged { channel: 1, id: 1 }]
        );
        assert_eq!(frames(&out)[0].0, 4);
    }

    #[test]
    fn session_queue_assigns_sequences_at_transmission_and_refreshes_nr_on_rej() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        assert_eq!(frames(&send(&mut e, 1, 40, now))[0].1.ns, 0);
        assert!(send(&mut e, 1, 41, now).is_empty());
        assert_eq!(e.links[&1].vs, 1);
        e.receive(
            0,
            &ax25::build_i("N0ME", "N1YOU", 0, 0, false, b"reply"),
            now,
        );
        let out = e.receive(0, &ax25::build_rej("N0ME", "N1YOU", 0, false), now);
        let f = &frames(&out)[0].1;
        assert_eq!((f.kind, f.ns, f.nr), (Ax25FrameType::Info, 0, 1));
        assert_eq!(f.info, vec![40]);
        assert!(events(&out).is_empty());
        let out = e.receive(0, &ax25::build_rr("N0ME", "N1YOU", 1, false, false), now);
        let f = &frames(&out)[0].1;
        assert_eq!((f.ns, f.nr), (1, 1));
        assert_eq!(
            events(&out),
            vec![&SessionEvent::Acknowledged { channel: 1, id: 40 }]
        );
    }

    fn rnr(local: &str, remote: &str, nr: u8, final_: bool) -> Vec<u8> {
        let mut frame = ax25::build_rr(local, remote, nr, final_, false);
        frame[14] |= 0x04;
        frame
    }

    #[test]
    fn session_rnr_persists_across_incoming_data_and_idle_ticks_until_rr() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        e.receive(0, &rnr("N0ME", "N1YOU", 0, false), now);
        assert!(send(&mut e, 1, 1, now).is_empty());
        let out = e.receive(
            0,
            &ax25::build_i("N0ME", "N1YOU", 0, 0, false, b"busy but sending"),
            now,
        );
        assert!(frames(&out)
            .iter()
            .all(|(_, f)| f.kind != Ax25FrameType::Info));
        let later = now + Duration::from_secs(2);
        let out = e.tick(later);
        assert!(frames(&out)
            .iter()
            .any(|(_, f)| f.kind == Ax25FrameType::Rr && f.command && f.poll));
        e.receive(0, &rnr("N0ME", "N1YOU", 0, true), later);
        assert!(e.links[&1].busy);
        let out = e.receive(0, &ax25::build_rr("N0ME", "N1YOU", 0, false, false), later);
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Info);
    }

    #[test]
    fn session_only_final_response_completes_poll_and_dead_link_is_removed() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME"), (2, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        connect(&mut e, 2, 0, "N2YOU", now);
        send(&mut e, 1, 1, now);
        let later = now + Duration::from_secs(2);
        let out = e.tick(later);
        written(&mut e, &out, later);
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Rr);
        e.receive(0, &ax25::build_rr("N0ME", "N1YOU", 0, false, false), later);
        assert!(e.links[&1].timer.is_some());
        e.receive(0, &ax25::build_rr("N0ME", "N1YOU", 0, true, true), later);
        assert!(e.links[&1].timer.is_some());
        let out = e.tick(later + Duration::from_secs(2));
        written(&mut e, &out, later + Duration::from_secs(2));
        let out = e.tick(later + Duration::from_secs(4));
        assert!(matches!(
            events(&out).as_slice(),
            [SessionEvent::Disconnected { channel: 1, .. }]
        ));
        assert!(!e.links.contains_key(&1));
        assert!(e.links.contains_key(&2));
        assert!(e.tick(later + Duration::from_secs(6)).is_empty());
    }

    #[test]
    fn session_valid_final_retransmits_missing_information_with_same_ns() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        send(&mut e, 1, 9, now);
        let later = now + Duration::from_secs(2);
        e.tick(later);
        let out = e.receive(0, &ax25::build_rr("N0ME", "N1YOU", 0, true, false), later);
        assert_eq!(
            (frames(&out)[0].1.ns, frames(&out)[0].1.info.clone()),
            (0, vec![9])
        );
        assert!(e.links[&1].timer.is_none());
    }

    #[test]
    fn session_remote_disc_drops_one_circuit_and_sabm_resends_unacked() {
        for reset in [false, true] {
            let now = Instant::now();
            let mut e = setup(&[(1, 0, "N0ME"), (2, 0, "N0ME")]);
            connect(&mut e, 1, 0, "N1YOU", now);
            connect(&mut e, 2, 0, "N2YOU", now);
            send(&mut e, 1, 1, now);
            send(&mut e, 1, 2, now);
            send(&mut e, 2, 3, now);
            let command = if reset {
                ax25::build_sabm("N0ME", "N1YOU")
            } else {
                ax25::build_disc("N0ME", "N1YOU")
            };
            let out = e.receive(0, &command, now);
            assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Ua);
            assert!(e.links[&2].inflight.is_some());
            if reset {
                let info = frames(&out)
                    .into_iter()
                    .find(|(_, frame)| frame.kind == Ax25FrameType::Info)
                    .expect("unacked I frame");
                assert_eq!((info.1.ns, info.1.nr, info.1.info), (0, 0, vec![1]));
                assert_eq!(e.links[&1].inflight.as_ref().map(|cur| cur.ns), Some(0));
                assert_eq!(
                    e.links[&1]
                        .queued
                        .iter()
                        .map(|packet| packet.data.clone())
                        .collect::<Vec<_>>(),
                    vec![vec![2]]
                );
                assert!(frames(&send(&mut e, 1, 4, now)).is_empty());
                assert_eq!(e.links[&1].queued.len(), 2);
            } else {
                assert!(!e.links.contains_key(&1));
            }
        }
    }

    #[test]
    fn session_duplicate_data_and_repeated_gaps_are_not_delivered_or_reset() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        let i = ax25::build_i("N0ME", "N1YOU", 0, 0, true, b"once");
        assert!(matches!(
            events(&e.receive(0, &i, now)).as_slice(),
            [SessionEvent::Data { .. }]
        ));
        let out = e.receive(0, &i, now);
        assert!(events(&out).is_empty());
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Rr);
        for _ in 0..3 {
            let out = e.receive(0, &ax25::build_i("N0ME", "N1YOU", 3, 0, false, b"gap"), now);
            assert!(events(&out).is_empty());
            assert!(frames(&out)
                .iter()
                .all(|(_, f)| f.kind != Ax25FrameType::Sabm));
        }
        assert_eq!(e.links[&1].vr, 1);
    }

    #[test]
    fn session_wrong_ssid_and_information_cannot_complete_connect_and_f_mirrors_p() {
        let now = Instant::now();
        let mut e = setup(&[(1, 5, "N0ME"), (2, 5, "N0ME")]);
        e.command(
            SessionCommand::Connect {
                channel: 1,
                port: 5,
                local: "N0ME".into(),
                remote: "N1YOU".into(),
                via: Vec::new(),
            },
            now,
        );
        for (port, frame) in [
            (5, ax25::build_ua("N0ME", "N1YOU-1", true)),
            (0, ax25::build_ua("N0ME", "N1YOU", true)),
            (5, ax25::build_ua("N0ME", "N1YOU", false)),
            (5, ax25::build_i("N0ME", "N1YOU", 0, 0, false, b"not UA")),
        ] {
            assert!(events(&e.receive(port, &frame, now)).is_empty());
        }
        assert!(e.links[&1].state == State::Connecting);
        let mut sabm = ax25::build_sabm("N0ME", "N2YOU");
        sabm[14] &= !0x10;
        let out = e.receive(5, &sabm, now);
        assert_eq!(frames(&out)[0].0, 5);
        assert!(!frames(&out)[0].1.poll);
        assert_eq!(frames(&out)[0].1.src, "N0ME");
    }

    #[test]
    fn session_modulo_wrap_receipts_are_application_ids_and_invalid_nr_resets() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        for id in 0..18 {
            let out = send(&mut e, 1, id, now);
            assert_eq!(frames(&out)[0].1.ns, (id & 7) as u8);
            let out = e.receive(
                0,
                &ax25::build_rr("N0ME", "N1YOU", ((id + 1) & 7) as u8, false, false),
                now,
            );
            assert_eq!(
                events(&out),
                vec![&SessionEvent::Acknowledged { channel: 1, id }]
            );
        }
        send(&mut e, 1, 100, now);
        send(&mut e, 1, 101, now);
        let out = e.receive(0, &ax25::build_rr("N0ME", "N1YOU", 7, false, false), now);
        assert!(matches!(
            events(&out).as_slice(),
            [SessionEvent::Reset { channel: 1, .. }]
        ));
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Sabm);
        assert!(e.links[&1].queued.is_empty());
        assert!(e.links[&1].inflight.is_none());
    }

    #[test]
    fn session_virtual_self_connect_uses_distinct_exact_endpoints_without_rf_acks() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME-1"), (2, 0, "N0ME")]);
        let out = e.command(
            SessionCommand::Connect {
                channel: 1,
                port: 0,
                local: "N0ME-1".into(),
                remote: "N0ME".into(),
                via: Vec::new(),
            },
            now,
        );
        assert_eq!(events(&out).len(), 2);
        assert!(frames(&out).is_empty());
        let out = send(&mut e, 1, 123, now);
        assert_eq!(
            events(&out),
            vec![
                &SessionEvent::Data {
                    channel: 2,
                    local: "N0ME".into(),
                    remote: "N0ME-1".into(),
                    data: vec![123]
                },
                &SessionEvent::Acknowledged {
                    channel: 1,
                    id: 123
                },
            ]
        );
        assert!(frames(&out).is_empty());
        e.command(SessionCommand::Disconnect { channel: 2 }, now);
        assert!(e.links.is_empty());
    }

    #[test]
    fn session_retry_clock_and_unack_start_after_the_modem_write() {
        let now = Instant::now();
        let mut e = setup(&[(1, 0, "N0ME")]);
        connect(&mut e, 1, 0, "N1YOU", now);
        let out = e.command(
            SessionCommand::Send {
                channel: 1,
                data: b"hello".to_vec(),
                id: None,
            },
            now,
        );
        let row = &e.buffers().rows[0];
        assert_eq!((row.not_sent, row.unack), (1, 0));
        let later = now + Duration::from_secs(100);
        assert!(e.tick(later).is_empty());
        written(&mut e, &out, later);
        let row = &e.buffers().rows[0];
        assert_eq!((row.not_sent, row.unack), (0, 1));
        assert!(e.tick(later + Duration::from_millis(1999)).is_empty());
        let out = e.tick(later + Duration::from_secs(2));
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Rr);
        assert!(frames(&out)[0].1.poll);
    }

    #[test]
    fn answering_port_becomes_the_circuit_port() {
        let now = Instant::now();
        let mut engine = setup(&[(1, 0, "N0ME")]);
        let out = engine.command(
            SessionCommand::Connect {
                channel: 1,
                port: 0,
                local: "N0ME".into(),
                remote: "N1YOU".into(),
                via: Vec::new(),
            },
            now,
        );
        let frame = match &out[0] {
            Action::Transmit { frame, .. } => frame.clone(),
            _ => panic!("sabm"),
        };
        engine.credit_transmit(&frame, now);
        assert!(!engine.links[&1].awaiting_write);
        let ua = ax25::build_ua("N0ME", "N1YOU", true);
        assert!(engine.adopt_port(2, &ua));
        let out = engine.receive(2, &ua, now);
        assert!(matches!(
            events(&out).as_slice(),
            [SessionEvent::Connected { port: 2, .. }]
        ));
        assert_eq!(engine.links[&1].config.port, 2);
    }

    #[test]
    fn incoming_sabm_on_another_kiss_port_opens_a_free_channel() {
        let now = Instant::now();
        let mut engine = setup(&[(1, 0, "NL0MSK"), (2, 0, "NL0MSK")]);
        engine.listen_on_every_kiss_port();
        let sabm = ax25::build_sabm("NL0MSK", "NL1MSK");
        let out = engine.receive(1, &sabm, now);
        assert_eq!(frames(&out)[0].0, 1);
        assert_eq!(frames(&out)[0].1.kind, Ax25FrameType::Ua);
        assert!(matches!(
            events(&out).as_slice(),
            [SessionEvent::Connected {
                channel: 1,
                port: 1,
                outgoing: false,
                ..
            }]
        ));
        assert_eq!(engine.links[&1].config.port, 1);
        assert_eq!(engine.configs[&1].port, 0);
        let again = engine.receive(1, &sabm, now);
        assert!(events(&again)
            .iter()
            .all(|event| !matches!(event, SessionEvent::Connected { channel: 2, .. })));
        assert!(!engine.links.contains_key(&2));
    }

    #[test]
    fn close_on_drops_only_the_ports_being_refreshed() {
        let now = Instant::now();
        let mut engine = setup(&[(1, 0, "N0ME"), (2, 1, "N0ME")]);
        connect(&mut engine, 1, 0, "N1YOU", now);
        connect(&mut engine, 2, 1, "N2YOU", now);
        let port0 = std::collections::BTreeSet::from([0]);
        assert!(engine.busy_on(&port0));
        assert!(engine.busy_on(&std::collections::BTreeSet::new()));
        let out = engine.close_on(&port0, "disconnected for serial refresh");
        assert!(matches!(
            events(&out).as_slice(),
            [SessionEvent::Disconnected { channel: 1, .. }]
        ));
        assert!(!engine.busy_on(&port0));
        assert!(engine.busy_on(&std::collections::BTreeSet::from([1])));
    }
}
