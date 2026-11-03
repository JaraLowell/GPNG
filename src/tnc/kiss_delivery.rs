//! KISS connected-mode pacing (AX.25 modulo 8).
//!
//! The UI may hand over several I-frames at once. This queue sends one,
//! waits for an RR or I whose N(R) acknowledges it, then sends the next.
//! UI and supervisory frames (SABM, DISC, UA, RR, DM) go out immediately.
//! Every I-frame, including NL0MSK to NL0MSK-2, waits for an RR. A
//! self-connect has no RF loopback, so the UI sends that RR itself and the
//! driver applies it with [`crate::tnc::TncCommand::LocalAck`].

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

#[derive(Debug)]
pub enum Tick {
    Resend(Outgoing),
    /// N2 exhausted. Later I-frames for this link were dropped with it.
    GaveUp { port: u8 },
}

#[derive(Default)]
pub struct KissDelivery {
    queued: VecDeque<Outgoing>,
    inflight: Option<Inflight>,
}

impl KissDelivery {
    pub fn new() -> Self {
        Self::default()
    }

    /// One row per peer. `tries` is repeats so far (0 while the first copy is
    /// still waiting). `unack` is 1 for the frame on the air. Bytes still
    /// sitting in the queue, not the frame already sent, are `waiting_bytes`.
    pub fn tx_snapshots(&self) -> Vec<TxSnap> {
        let mut map: BTreeMap<String, TxSnap> = BTreeMap::new();
        if let Some(cur) = &self.inflight {
            let peer = peer_of(&cur.frame);
            map.insert(
                peer.clone(),
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
        for job in &self.queued {
            let peer = peer_of(&job.frame);
            let snap = map.entry(peer.clone()).or_insert(TxSnap {
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
        map.into_values().collect()
    }

    /// Decide what to put on the wire for this AX.25 frame.
    pub fn push(&mut self, port: u8, frame: Vec<u8>, now: Instant) -> Option<Outgoing> {
        let parsed = ax25::parse_frame(&frame);
        if parsed.as_ref().is_some_and(|f| {
            matches!(f.kind, Ax25FrameType::Sabm | Ax25FrameType::Disc)
        }) {
            self.queued.clear();
            self.inflight = None;
            return Some(Outgoing { port, frame });
        }
        let Some(parsed) = parsed else {
            return Some(Outgoing { port, frame });
        };
        if parsed.kind != Ax25FrameType::Info {
            return Some(Outgoing { port, frame });
        }
        if self.inflight.is_some() {
            self.queued.push_back(Outgoing { port, frame });
            return None;
        }
        self.inflight = Some(Inflight {
            port,
            frame: frame.clone(),
            ns: parsed.ns & 7,
            sent: now,
            tries: 1,
        });
        Some(Outgoing { port, frame })
    }

    /// Peer RR or I. N(R) is the next N(S) the peer expects.
    pub fn on_rx(&mut self, ax25_payload: &[u8], now: Instant) -> Option<Outgoing> {
        let Some(frame) = ax25::parse_frame(ax25_payload) else {
            return None;
        };
        if !matches!(frame.kind, Ax25FrameType::Rr | Ax25FrameType::Info) {
            return None;
        }
        let ack_ok = {
            let Some(current) = self.inflight.as_ref() else {
                return None;
            };
            if frame.nr != (current.ns + 1) & 7 {
                return None;
            }
            // The ack has to be from the station we sent to, back to us.
            match ax25::parse_frame(&current.frame) {
                Some(sent) => {
                    frame.src.eq_ignore_ascii_case(&sent.dest)
                        && frame.dest.eq_ignore_ascii_case(&sent.src)
                }
                None => true,
            }
        };
        if !ack_ok {
            return None;
        }
        self.inflight = None;
        self.send_next(now)
    }

    pub fn tick(&mut self, now: Instant) -> Option<Tick> {
        let due = self
            .inflight
            .as_ref()
            .is_some_and(|f| now.duration_since(f.sent) >= T1);
        if !due {
            return None;
        }
        let tries = self.inflight.as_ref().map(|f| f.tries).unwrap_or(0);
        if tries >= N2 {
            let port = self.inflight.as_ref().map(|f| f.port).unwrap_or(0);
            self.inflight = None;
            self.queued.clear();
            return Some(Tick::GaveUp { port });
        }
        let inflight = self.inflight.as_mut().expect("inflight");
        inflight.tries += 1;
        inflight.sent = now;
        Some(Tick::Resend(Outgoing {
            port: inflight.port,
            frame: inflight.frame.clone(),
        }))
    }

    fn send_next(&mut self, now: Instant) -> Option<Outgoing> {
        let next = self.queued.pop_front()?;
        let ns = ax25::parse_frame(&next.frame).map(|f| f.ns & 7).unwrap_or(0);
        self.inflight = Some(Inflight {
            port: next.port,
            frame: next.frame.clone(),
            ns,
            sent: now,
            tries: 1,
        });
        Some(next)
    }
}

fn peer_of(frame: &[u8]) -> String {
    ax25::parse_frame(frame)
        .map(|f| f.dest)
        .unwrap_or_default()
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
    use crate::tnc::ax25::{build_i, build_rr, build_ui};

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
        assert!(q
            .push(0, build_ui("CQ", "ME", &[], b"beacon"), t)
            .is_some());
        assert!(q
            .push(0, build_ui("CQ", "ME", &[], b"again"), t)
            .is_some());
        assert!(q.tx_snapshots().is_empty());
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
}
