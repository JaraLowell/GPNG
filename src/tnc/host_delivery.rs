//! Hostmode (WA8DED / TFPCX) command pacing.
//!
//! The TNC owns AX.25 acknowledgements. The host is allowed one outstanding
//! hostmode frame: the next text part (or command) is written only after the
//! TNC sends a reply, which is how those firmwares accept a block into the
//! transmit buffer.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use super::TxSnap;

/// How long to wait for the TNC's hostmode reply.
pub const ACK_WAIT: Duration = Duration::from_secs(3);
/// Resends of the same frame before it and the rest of the queue are dropped.
pub const N2: u32 = 5;

/// What an outstanding host→TNC frame is. Polls and status queries must not
/// be counted as operator packets on the magenta bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    Data,
    Poll,
    LinkStatus,
    FreeBuffer,
}

#[derive(Debug, Clone)]
pub struct Outgoing {
    pub channel: u8,
    pub data: Vec<u8>,
    pub kind: HostKind,
}

struct Inflight {
    frame: Outgoing,
    sent: Instant,
    tries: u32,
}

#[derive(Debug)]
pub enum Tick {
    Resend(Outgoing),
    GaveUp { channel: u8 },
}

#[derive(Default)]
pub struct HostDelivery {
    queued: VecDeque<Outgoing>,
    inflight: Option<Inflight>,
}

impl HostDelivery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn idle(&self) -> bool {
        self.inflight.is_none() && self.queued.is_empty()
    }

    pub fn inflight_kind(&self) -> Option<HostKind> {
        self.inflight.as_ref().map(|f| f.frame.kind)
    }

    /// Queue operator data. Returns it when it should be written now.
    pub fn enqueue(&mut self, channel: u8, data: Vec<u8>, now: Instant) -> Option<Outgoing> {
        self.enqueue_kind(channel, data, HostKind::Data, now)
    }

    pub fn enqueue_kind(
        &mut self,
        channel: u8,
        data: Vec<u8>,
        kind: HostKind,
        now: Instant,
    ) -> Option<Outgoing> {
        self.queued.push_back(Outgoing {
            channel,
            data,
            kind,
        });
        self.send_next(now)
    }

    /// Per-channel view of operator data. A `G0` / `L` / `@B` in flight is not
    /// an unacknowledged packet. `tries` is the repeat count (0 on the first
    /// copy); `unack` is 1 while that copy is outstanding.
    pub fn tx_snapshots(&self) -> Vec<TxSnap> {
        let mut map: BTreeMap<u8, TxSnap> = BTreeMap::new();
        if let Some(cur) = &self.inflight {
            if cur.frame.kind == HostKind::Data {
                map.insert(
                    cur.frame.channel,
                    TxSnap {
                        channel: cur.frame.channel,
                        peer: String::new(),
                        not_sent: 0,
                        unack: 1,
                        tries: cur.tries.saturating_sub(1),
                        waiting_bytes: 0,
                    },
                );
            }
        }
        for job in &self.queued {
            if job.kind != HostKind::Data {
                continue;
            }
            let snap = map.entry(job.channel).or_insert(TxSnap {
                channel: job.channel,
                peer: String::new(),
                not_sent: 0,
                unack: 0,
                tries: 0,
                waiting_bytes: 0,
            });
            snap.not_sent += 1;
            snap.waiting_bytes += job.data.len() as u32;
        }
        map.into_values().collect()
    }

    /// Any hostmode frame from the TNC completes the outstanding command.
    pub fn on_tnc_frame(&mut self, now: Instant) -> Option<Outgoing> {
        if self.inflight.is_none() {
            return None;
        }
        self.inflight = None;
        self.send_next(now)
    }

    pub fn tick(&mut self, now: Instant) -> Option<Tick> {
        let due = self
            .inflight
            .as_ref()
            .is_some_and(|f| now.duration_since(f.sent) >= ACK_WAIT);
        if !due {
            return None;
        }
        let tries = self.inflight.as_ref().map(|f| f.tries).unwrap_or(0);
        if tries >= N2 {
            let channel = self.inflight.as_ref().map(|f| f.frame.channel).unwrap_or(0);
            self.inflight = None;
            self.queued.clear();
            return Some(Tick::GaveUp { channel });
        }
        let inflight = self.inflight.as_mut().expect("inflight");
        inflight.tries += 1;
        inflight.sent = now;
        Some(Tick::Resend(inflight.frame.clone()))
    }

    fn send_next(&mut self, now: Instant) -> Option<Outgoing> {
        if self.inflight.is_some() {
            return None;
        }
        let frame = self.queued.pop_front()?;
        self.inflight = Some(Inflight {
            frame: frame.clone(),
            sent: now,
            tries: 1,
        });
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_part_waits_for_tnc_reply() {
        let mut q = HostDelivery::new();
        let t = Instant::now();
        let first = q.enqueue(1, b"hello\n".to_vec(), t).unwrap();
        assert_eq!(first.data, b"hello\n");
        assert!(q.enqueue(1, b"world".to_vec(), t).is_none());
        let second = q.on_tnc_frame(t).unwrap();
        assert_eq!(second.data, b"world");
        assert!(q.on_tnc_frame(t).is_none());
    }

    #[test]
    fn poll_is_not_an_unacked_packet() {
        let mut q = HostDelivery::new();
        let t = Instant::now();
        q.enqueue_kind(0, b"G0".to_vec(), HostKind::Poll, t)
            .unwrap();
        assert!(q.enqueue(1, b"hello".to_vec(), t).is_none());
        let snaps = q.tx_snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].channel, 1);
        assert_eq!(snaps[0].not_sent, 1);
        assert_eq!(snaps[0].unack, 0);
        assert_eq!(snaps[0].tries, 0);
        assert_eq!(snaps[0].waiting_bytes, 5);
        let _ = q.on_tnc_frame(t).unwrap();
        let snaps = q.tx_snapshots();
        assert_eq!(snaps[0].not_sent, 0);
        assert_eq!(snaps[0].unack, 1);
        assert_eq!(snaps[0].waiting_bytes, 0);
    }
}
