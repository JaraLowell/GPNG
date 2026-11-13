//! KISS connected-mode pacing (AX.25 modulo 8).
//!
//! The UI may hand over several I-frames at once. Each link (our call to
//! their call) sends one, waits for an RR or I whose N(R) acknowledges it,
//! then sends the next. A channel that is already in use does not hold
//! another station's frames — the redirected user still gets CTEXT.
//! UI and supervisory frames (SABM, DISC, UA, RR, DM) go out immediately.
//! Every I-frame, including NL0MSK to NL0MSK-2, waits for an RR. A
//! self-connect has no RF loopback, so the UI invents that RR
//! ([`crate::tnc::TncCommand::LocalAck`]). The frame is still written to the
//! radio, so the invented RR is held until the frame could have been
//! transmitted and a real peer would have answered. Applying it in the same
//! burst releases the next packet while the UART is still full.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use super::ax25::{self, Ax25FrameType};
use super::TxSnap;

/// Time to wait for an acknowledgement before repeating the I-frame.
pub const T1: Duration = Duration::from_secs(5);
/// Retries after the first send. Then the outstanding part and anything
/// still queued behind it are dropped.
pub const N2: u32 = 10;

#[derive(Debug, Clone)]
pub struct Outgoing {
    pub port: u8,
    pub frame: Vec<u8>,
}

struct Inflight {
    port: u8,
    frame: Vec<u8>,
    ns: u8,
    sent: Instant,
    tries: u32,
}

/// Peer asked us to stop sending (RNR). Cleared by RR / REJ / I.
#[derive(Default)]
struct PeerFlow {
    busy: bool,
}

#[derive(Debug)]
pub enum Tick {
    Resend(Outgoing),
    /// N2 exhausted. Later I-frames for this link were dropped with it.
    GaveUp {
        port: u8,
    },
}

/// RR the UI invented for a self-connect. Not applied until
/// [`radio_ack_wait`] for the I-frame it acknowledges.
struct HeldLocalAck {
    port: u8,
    frame: Vec<u8>,
}

/// One local RR that is now allowed onto the wire, plus the I-frame it frees.
pub struct Released {
    pub ack: Outgoing,
    pub next: Option<Outgoing>,
}

/// One AX.25 link (our call → their call), including a redirected SSID.
#[derive(Default)]
struct Link {
    queued: VecDeque<Outgoing>,
    inflight: Option<Inflight>,
    flow: PeerFlow,
}

#[derive(Default)]
pub struct KissDelivery {
    /// Key is `SRC|DEST` of the frames we send. A busy channel does not hold
    /// I-frames for a different call.
    links: BTreeMap<String, Link>,
    held_acks: VecDeque<HeldLocalAck>,
}

/// How long a self-connect must wait before the next I-frame.
///
/// Channel 1 → channel 2 still sends each frame to the radio. There is no
/// loopback, so nothing comes back to say the radio finished transmitting.
/// This is that missing confirmation: TX delay, on-air time at `hbaud`, and
/// a short RR. `hbaud` is the radio's packet speed (`HB` / `HBAUD`), not the
/// PC serial baud.
pub fn radio_ack_wait(frame_len: usize, hbaud: u32) -> Duration {
    let hbaud = u64::from(hbaud.clamp(300, 115_200));
    let bytes = (frame_len as u64).saturating_add(2);
    let air_us = bytes.saturating_mul(8).saturating_mul(1_000_000) / hbaud;
    let wait = Duration::from_micros(air_us) + Duration::from_millis(280);
    wait.min(T1.saturating_sub(Duration::from_millis(500)))
}

impl KissDelivery {
    pub fn new() -> Self {
        Self::default()
    }

    /// One row per link. `tries` is repeats so far (0 while the first copy is
    /// still waiting). `unack` is 1 for the frame on the air. Bytes still
    /// sitting in the queue, not the frame already sent, are `waiting_bytes`.
    pub fn tx_snapshots(&self) -> Vec<TxSnap> {
        let mut map: BTreeMap<String, TxSnap> = BTreeMap::new();
        for (key, link) in &self.links {
            if let Some(cur) = &link.inflight {
                let peer = peer_of(&cur.frame);
                map.insert(
                    key.clone(),
                    TxSnap {
                        channel: 0,
                        peer,
                        not_sent: 0,
                        unack: 1,
                        tries: cur.tries.saturating_sub(1),
                        waiting_bytes: 0,
                    },
                );
            }
            for job in &link.queued {
                let peer = peer_of(&job.frame);
                let snap = map.entry(key.clone()).or_insert(TxSnap {
                    channel: 0,
                    peer,
                    not_sent: 0,
                    unack: 0,
                    tries: 0,
                    waiting_bytes: 0,
                });
                snap.not_sent += 1;
                snap.waiting_bytes += packet_len(&job.frame);
            }
        }
        map.into_values().collect()
    }

    /// Decide what to put on the wire for this AX.25 frame.
    pub fn push(&mut self, port: u8, frame: Vec<u8>, now: Instant) -> Option<Outgoing> {
        let parsed = ax25::parse_frame(&frame);
        if let Some(parsed) = parsed
            .as_ref()
            .filter(|f| matches!(f.kind, Ax25FrameType::Sabm | Ax25FrameType::Disc))
        {
            let key = pair_key(&parsed.src, &parsed.dest);
            let src = parsed.src.clone();
            let dest = parsed.dest.clone();
            self.links.remove(&key);
            // Drop invented RRs for this link only. Another QSO keeps its queue.
            self.held_acks.retain(|ack| {
                ax25::parse_frame(&ack.frame)
                    .map(|a| {
                        !(a.src.eq_ignore_ascii_case(&dest) && a.dest.eq_ignore_ascii_case(&src))
                    })
                    .unwrap_or(true)
            });
            return Some(Outgoing { port, frame });
        }
        let Some(parsed) = parsed else {
            return Some(Outgoing { port, frame });
        };
        if parsed.kind != Ax25FrameType::Info {
            return Some(Outgoing { port, frame });
        }
        let key = pair_key(&parsed.src, &parsed.dest);
        let link = self.links.entry(key).or_default();
        if link.inflight.is_some() || link.flow.busy {
            link.queued.push_back(Outgoing { port, frame });
            return None;
        }
        link.inflight = Some(Inflight {
            port,
            frame: frame.clone(),
            ns: parsed.ns & 7,
            sent: now,
            tries: 1,
        });
        Some(Outgoing { port, frame })
    }

    /// Queue a self-connect RR. [`Self::poll_local_ack`] writes it later.
    pub fn hold_local_ack(&mut self, port: u8, frame: Vec<u8>) {
        self.held_acks.push_back(HeldLocalAck { port, frame });
    }

    /// One due local RR, if the in-flight frame has had time to go out on the air.
    pub fn poll_local_ack(&mut self, now: Instant, hbaud: u32) -> Option<Released> {
        let ack = self.held_acks.front()?;
        let (sent, len) = self.inflight_age_for_ack(&ack.frame)?;
        if now.duration_since(sent) < radio_ack_wait(len, hbaud) {
            return None;
        }
        let ack = self.held_acks.pop_front()?;
        let next = self.on_rx(&ack.frame, now);
        Some(Released {
            ack: Outgoing {
                port: ack.port,
                frame: ack.frame,
            },
            next,
        })
    }

    /// Peer RR / REJ / RNR / I. N(R) is the next N(S) the peer expects.
    ///
    /// RR and I clear peer-busy and free the acknowledged frame. REJ naming
    /// our outstanding N(S) sends that frame again (it was missed). REJ with
    /// N(R) just past it is an ack. Any other N(R) is a frame this window
    /// does not have: the outstanding I-frame and the queue are dropped so
    /// T1 does not keep repeating the wrong one. The UI then resets the link.
    pub fn on_rx(&mut self, ax25_payload: &[u8], now: Instant) -> Option<Outgoing> {
        let Some(frame) = ax25::parse_frame(ax25_payload) else {
            return None;
        };
        if !matches!(
            frame.kind,
            Ax25FrameType::Rr | Ax25FrameType::Rnr | Ax25FrameType::Rej | Ax25FrameType::Info
        ) {
            return None;
        }
        let Some(key) = self.links.iter().find_map(|(key, link)| {
            // Match the link even with nothing in flight (RNR while idle).
            let peer_ok = if let Some(current) = link.inflight.as_ref() {
                let sent = ax25::parse_frame(&current.frame)?;
                frame.src.eq_ignore_ascii_case(&sent.dest)
                    && frame.dest.eq_ignore_ascii_case(&sent.src)
            } else {
                let (src, dest) = split_key(key)?;
                frame.src.eq_ignore_ascii_case(dest) && frame.dest.eq_ignore_ascii_case(src)
            };
            if peer_ok {
                Some(key.clone())
            } else {
                None
            }
        }) else {
            return None;
        };
        let link = self.links.get_mut(&key)?;
        match frame.kind {
            Ax25FrameType::Rnr => {
                link.flow.busy = true;
                if link
                    .inflight
                    .as_ref()
                    .is_some_and(|cur| frame.nr == (cur.ns + 1) & 7)
                {
                    let _ = release_inflight_only(link);
                }
                None
            }
            Ax25FrameType::Rej => {
                link.flow.busy = false;
                if let Some(cur) = link.inflight.as_ref() {
                    if frame.nr == (cur.ns & 7) {
                        // They missed this one — send the same I-frame again.
                        let out = Outgoing {
                            port: cur.port,
                            frame: cur.frame.clone(),
                        };
                        if let Some(cur) = link.inflight.as_mut() {
                            cur.sent = now;
                            cur.tries = cur.tries.saturating_add(1);
                        }
                        return Some(out);
                    }
                    if frame.nr == (cur.ns + 1) & 7 {
                        return release_next(link, now);
                    }
                    // N(R) is not the frame we can send. Stop repeating it.
                    link.inflight = None;
                    link.queued.clear();
                }
                None
            }
            Ax25FrameType::Rr | Ax25FrameType::Info => {
                link.flow.busy = false;
                if link
                    .inflight
                    .as_ref()
                    .is_some_and(|cur| frame.nr == (cur.ns + 1) & 7)
                {
                    release_next(link, now)
                } else if link.inflight.is_none() {
                    // Peer reopened the pipe; start the next queued I.
                    release_next(link, now)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn tick(&mut self, now: Instant) -> Option<Tick> {
        self.links
            .retain(|_, link| link.inflight.is_some() || !link.queued.is_empty());
        let due_key = self.links.iter().find_map(|(key, link)| {
            if link.flow.busy {
                return None;
            }
            let cur = link.inflight.as_ref()?;
            if now.duration_since(cur.sent) >= T1 {
                Some(key.clone())
            } else {
                None
            }
        })?;
        let link = self.links.get_mut(&due_key)?;
        let tries = link.inflight.as_ref().map(|f| f.tries).unwrap_or(0);
        if tries >= N2 {
            let port = link.inflight.as_ref().map(|f| f.port).unwrap_or(0);
            link.inflight = None;
            link.queued.clear();
            return Some(Tick::GaveUp { port });
        }
        let inflight = link.inflight.as_mut().expect("inflight");
        inflight.tries += 1;
        inflight.sent = now;
        Some(Tick::Resend(Outgoing {
            port: inflight.port,
            frame: inflight.frame.clone(),
        }))
    }

    /// When the invented RR was queued, and how long that I-frame is.
    fn inflight_age_for_ack(&self, ack: &[u8]) -> Option<(Instant, usize)> {
        let frame = ax25::parse_frame(ack)?;
        for link in self.links.values() {
            let Some(current) = link.inflight.as_ref() else {
                continue;
            };
            if frame.nr != (current.ns + 1) & 7 {
                continue;
            }
            let sent = ax25::parse_frame(&current.frame)?;
            if frame.src.eq_ignore_ascii_case(&sent.dest)
                && frame.dest.eq_ignore_ascii_case(&sent.src)
            {
                return Some((current.sent, current.frame.len()));
            }
        }
        None
    }
}

fn release_inflight_only(link: &mut Link) {
    link.inflight = None;
}

fn release_next(link: &mut Link, now: Instant) -> Option<Outgoing> {
    link.inflight = None;
    if link.flow.busy {
        return None;
    }
    let next = link.queued.pop_front()?;
    let ns = ax25::parse_frame(&next.frame)
        .map(|f| f.ns & 7)
        .unwrap_or(0);
    link.inflight = Some(Inflight {
        port: next.port,
        frame: next.frame.clone(),
        ns,
        sent: now,
        tries: 1,
    });
    Some(next)
}

fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('|')
}

fn pair_key(src: &str, dest: &str) -> String {
    format!("{}|{}", src.to_ascii_uppercase(), dest.to_ascii_uppercase())
}

fn peer_of(frame: &[u8]) -> String {
    ax25::parse_frame(frame).map(|f| f.dest).unwrap_or_default()
}

fn packet_len(frame: &[u8]) -> u32 {
    ax25::parse_frame(frame)
        .map(|f| f.info.len() as u32)
        .unwrap_or(frame.len() as u32)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::tnc::ax25::{build_i, build_rej, build_rr, build_ui};

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn holds_next_i_until_rr() {
        let mut q = KissDelivery::new();
        let t = now();
        let first = q
            .push(0, build_i("PEER", "ME", 0, 0, false, b"one"), t)
            .expect("first I goes out");
        assert_eq!(
            super::ax25::parse_frame(&first.frame).unwrap().kind,
            Ax25FrameType::Info
        );
        assert!(q
            .push(0, build_i("PEER", "ME", 1, 0, false, b"two"), t)
            .is_none());
        let rr = build_rr("ME", "PEER", 1, true, false);
        let second = q.on_rx(&rr, t).expect("second I after ack");
        let parsed = ax25::parse_frame(&second.frame).unwrap();
        assert_eq!(parsed.ns, 1);
        assert_eq!(parsed.info, b"two");
    }

    #[test]
    fn ui_is_not_held() {
        let mut q = KissDelivery::new();
        let t = now();
        assert!(q.push(0, build_ui("CQ", "ME", &[], b"beacon"), t).is_some());
        assert!(q.push(0, build_ui("CQ", "ME", &[], b"again"), t).is_some());
        assert!(q.tx_snapshots().is_empty());
    }

    #[test]
    fn local_ack_waits_until_the_frame_could_leave_the_radio() {
        let mut q = KissDelivery::new();
        let t = now();
        let first = build_i("ME-1", "ME", 0, 0, false, &[b'x'; 80]);
        let wait = radio_ack_wait(first.len(), 1200);
        q.push(0, first, t).unwrap();
        assert!(q
            .push(0, build_i("ME-1", "ME", 1, 0, false, b"next"), t)
            .is_none());
        q.hold_local_ack(0, build_rr("ME", "ME-1", 1, false, false));
        q.hold_local_ack(0, build_rr("ME", "ME-1", 2, false, false));
        assert!(q
            .poll_local_ack(t + wait - Duration::from_millis(1), 1200)
            .is_none());
        let released = q.poll_local_ack(t + wait, 1200).unwrap();
        assert_eq!(
            ax25::parse_frame(&released.next.unwrap().frame).unwrap().ns,
            1
        );
        // The next invented RR stays queued until this new frame's air time.
        assert!(q.poll_local_ack(t + wait, 1200).is_none());
    }

    #[test]
    fn slower_radio_waits_longer() {
        let frame = 160;
        assert!(radio_ack_wait(frame, 1200) > radio_ack_wait(frame, 9600));
    }

    #[test]
    fn self_connect_i_waits_for_rr() {
        let mut q = KissDelivery::new();
        let t = now();
        assert!(q
            .push(0, build_i("ME-1", "ME", 0, 0, false, b"a"), t)
            .is_some());
        assert!(q
            .push(0, build_i("ME-1", "ME", 1, 0, false, b"b"), t)
            .is_none());
        let rr = build_rr("ME", "ME-1", 1, false, false);
        let second = q.on_rx(&rr, t).expect("RR releases the next I");
        assert_eq!(super::ax25::parse_frame(&second.frame).unwrap().ns, 1);
    }

    #[test]
    fn busy_link_does_not_hold_another_stations_ctext() {
        let mut q = KissDelivery::new();
        let t = now();
        // The channel that is already in use still has an I-frame out.
        q.push(0, build_i("OTHER", "NL1AS", 0, 0, false, b"qso"), t)
            .unwrap();
        // The station we moved to NL1AS-1 gets the greeting now.
        let ctext = q
            .push(0, build_i("NL0MSK", "NL1AS-1", 0, 0, false, b"welcome"), t)
            .expect("CTEXT is not queued behind the busy channel");
        assert_eq!(ax25::parse_frame(&ctext.frame).unwrap().info, b"welcome");
        // The next greeting chunk still waits on that same redirected link.
        assert!(q
            .push(0, build_i("NL0MSK", "NL1AS-1", 1, 0, false, b"more"), t)
            .is_none());
        let rr = build_rr("NL1AS-1", "NL0MSK", 1, false, false);
        let more = q.on_rx(&rr, t).expect("RR from the redirected user");
        assert_eq!(ax25::parse_frame(&more.frame).unwrap().info, b"more");
        // The original QSO is still waiting on its own ack.
        assert!(q
            .tx_snapshots()
            .iter()
            .any(|s| s.peer == "OTHER" && s.unack == 1));
    }

    #[test]
    fn queued_i_waits_and_inflight_is_unack() {
        let mut q = KissDelivery::new();
        let t = now();
        q.push(0, build_i("PEER", "ME", 0, 0, false, b"one"), t)
            .unwrap();
        assert!(q
            .push(0, build_i("PEER", "ME", 1, 0, false, b"two!"), t)
            .is_none());
        let snaps = q.tx_snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].peer, "PEER");
        assert_eq!(snaps[0].unack, 1);
        assert_eq!(snaps[0].tries, 0);
        assert_eq!(snaps[0].not_sent, 1);
        assert_eq!(snaps[0].waiting_bytes, 4);
        let later = t + T1 + Duration::from_millis(1);
        assert!(matches!(q.tick(later), Some(Tick::Resend(_))));
        let snaps = q.tx_snapshots();
        assert_eq!(snaps[0].tries, 1);
        assert_eq!(snaps[0].unack, 1);
    }

    #[test]
    fn rej_of_the_frame_in_flight_sends_it_again() {
        let mut q = KissDelivery::new();
        let t = now();
        q.push(0, build_i("PEER", "ME", 1, 0, false, b"one"), t)
            .unwrap();
        let again = q
            .on_rx(&build_rej("ME", "PEER", 1, false), t)
            .expect("resend the missed I-frame");
        assert_eq!(ax25::parse_frame(&again.frame).unwrap().ns, 1);
        assert!(q.tick(t + T1 - Duration::from_millis(1)).is_none());
    }

    #[test]
    fn rej_for_a_frame_we_do_not_have_stops_the_retry() {
        let mut q = KissDelivery::new();
        let t = now();
        q.push(0, build_i("PEER", "ME", 3, 3, false, b"lol"), t)
            .unwrap();
        assert!(q
            .push(0, build_i("PEER", "ME", 4, 3, false, b"next"), t)
            .is_none());
        assert!(q.on_rx(&build_rej("ME", "PEER", 1, false), t).is_none());
        assert!(q.tick(t + T1 + Duration::from_millis(1)).is_none());
        assert!(q.tx_snapshots().is_empty());
    }
}
