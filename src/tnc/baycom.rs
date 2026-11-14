//! Portable BayCom frame/HDLC and half-duplex core.
//!
//! The caller supplies recovered bits, carrier state, monotonic timestamps and
//! random persistence samples. The backend must clock the transmitted bits,
//! confirm PTT changes and report completion only after all bits leave hardware.
//! AX.25 addressing, connections and retransmissions belong to the session layer.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use thiserror::Error;

pub const EXPERIMENTAL_WARNING: &str =
    "Experimental BayCom: FT232R full-handshake RS232 only, 1200 bit/s. Verify wiring and PTT with the transmitter disabled; see docs/BAYCOM.md.";
pub const BIT_RATE: u32 = 1200;
/// AX.25 frame bytes, excluding HDLC flags and the two-byte FCS.
pub const MAX_FRAME_BYTES: usize = 330;
const FLAG: u8 = 0x7e;
const MAX_STUFFED_BITS: usize = (MAX_FRAME_BYTES + 2) * 8 * 6 / 5 + 8;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BaycomError {
    #[error("BayCom frame length {0} is outside 1..={MAX_FRAME_BYTES}")]
    FrameLength(usize),
    #[error("BayCom receive frame exceeded its bounded buffer")]
    ReceiveOverflow,
    #[error("HDLC frame aborted")]
    Abort,
    #[error("invalid HDLC bit stuffing")]
    BitStuffing,
    #[error("HDLC frame did not end on a byte boundary")]
    Unaligned,
    #[error("HDLC frame is too short to contain data and FCS")]
    TooShort,
    #[error("HDLC FCS mismatch")]
    Fcs,
    #[error("BayCom transmit queue is full")]
    QueueFull,
    #[error("invalid BayCom settings: {0}")]
    Settings(&'static str),
    #[error("unexpected BayCom hardware completion or PTT state")]
    UnexpectedCompletion,
}

/// CRC-16/IBM-SDLC (AX.25 FCS), transmitted low byte first.
pub fn fcs(bytes: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn flag(bits: &mut Vec<bool>) {
    bits.extend((0..8).map(|shift| FLAG & (1 << shift) != 0));
}

fn encode(frame: &[u8], preamble: usize, tail: usize) -> Vec<bool> {
    let mut bits = Vec::new();
    for _ in 0..preamble {
        flag(&mut bits);
    }
    let mut ones = 0;
    for byte in frame.iter().copied().chain(fcs(frame).to_le_bytes()) {
        for shift in 0..8 {
            let bit = byte & (1 << shift) != 0;
            bits.push(bit);
            ones = if bit { ones + 1 } else { 0 };
            if ones == 5 {
                bits.push(false);
                ones = 0;
            }
        }
    }
    for _ in 0..tail {
        flag(&mut bits);
    }
    bits
}

/// One opening flag, stuffed data/FCS, and one closing flag; bits are LSB first.
pub fn encode_frame(frame: &[u8]) -> Result<Vec<bool>, BaycomError> {
    validate_frame(frame)?;
    Ok(encode(frame, 1, 1))
}

fn validate_frame(frame: &[u8]) -> Result<(), BaycomError> {
    if frame.is_empty() || frame.len() > MAX_FRAME_BYTES {
        Err(BaycomError::FrameLength(frame.len()))
    } else {
        Ok(())
    }
}

/// Use separate instances for transmit and receive line polarity.
pub struct Nrzi {
    level: bool,
}

impl Nrzi {
    pub fn new(initial_level: bool) -> Self {
        Self {
            level: initial_level,
        }
    }

    pub fn encode(&mut self, bit: bool) -> bool {
        self.level ^= !bit;
        self.level
    }

    pub fn decode(&mut self, level: bool) -> bool {
        let bit = level == self.level;
        self.level = level;
        bit
    }
}

/// Streaming, bounded HDLC receiver. Read boundaries have no significance.
#[derive(Default)]
pub struct Receiver {
    window: u8,
    seen: u8,
    ones: u8,
    collecting: bool,
    raw: Vec<bool>,
}

impl Receiver {
    pub fn reset(&mut self) {
        self.window = 0;
        self.seen = 0;
        self.ones = 0;
        self.collecting = false;
        self.raw.clear();
    }

    pub fn push(&mut self, bit: bool) -> Option<Result<Vec<u8>, BaycomError>> {
        self.window = (self.window >> 1) | if bit { 0x80 } else { 0 };
        self.seen = self.seen.saturating_add(1).min(8);
        self.ones = if bit { self.ones.saturating_add(1) } else { 0 };
        if self.collecting {
            self.raw.push(bit);
        }
        if self.ones >= 7 && self.collecting {
            self.collecting = false;
            self.raw.clear();
            return Some(Err(BaycomError::Abort));
        }
        if self.seen == 8 && self.window == FLAG {
            let result = if self.collecting && self.raw.len() > 8 {
                self.raw.truncate(self.raw.len() - 8);
                Some(decode(&self.raw))
            } else {
                None
            };
            self.raw.clear();
            self.collecting = true;
            return result;
        }
        if self.raw.len() > MAX_STUFFED_BITS {
            self.raw.clear();
            self.collecting = false;
            return Some(Err(BaycomError::ReceiveOverflow));
        }
        None
    }
}

fn decode(bits: &[bool]) -> Result<Vec<u8>, BaycomError> {
    let mut bytes = Vec::new();
    let (mut ones, mut used, mut byte) = (0, 0, 0u8);
    for &bit in bits {
        if ones == 5 {
            if bit {
                return Err(BaycomError::BitStuffing);
            }
            ones = 0;
            continue;
        }
        ones = if bit { ones + 1 } else { 0 };
        byte |= u8::from(bit) << used;
        used += 1;
        if used == 8 {
            bytes.push(byte);
            if bytes.len() > MAX_FRAME_BYTES + 2 {
                return Err(BaycomError::ReceiveOverflow);
            }
            used = 0;
            byte = 0;
        }
    }
    if ones == 5 {
        return Err(BaycomError::BitStuffing);
    }
    if used != 0 {
        return Err(BaycomError::Unaligned);
    }
    if bytes.len() < 3 {
        return Err(BaycomError::TooShort);
    }
    let end = bytes.len() - 2;
    if bytes[end..] != fcs(&bytes[..end]).to_le_bytes() {
        return Err(BaycomError::Fcs);
    }
    bytes.truncate(end);
    Ok(bytes)
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub tx_delay: Duration,
    pub tx_tail: Duration,
    pub turnaround: Duration,
    pub slot_time: Duration,
    pub persist: u8,
    /// Bounds include the active frame until completion or explicit failure.
    pub max_frames: usize,
    pub max_bytes: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tx_delay: Duration::from_millis(300),
            tx_tail: Duration::from_millis(30),
            turnaround: Duration::from_millis(100),
            slot_time: Duration::from_millis(100),
            persist: 63,
            max_frames: 32,
            max_bytes: 32 * MAX_FRAME_BYTES,
        }
    }
}

impl Settings {
    pub fn set_parameter(&mut self, command: u8, value: u8) -> Result<(), BaycomError> {
        let duration = Duration::from_millis(u64::from(value) * 10);
        match command {
            1 => self.tx_delay = duration,
            2 => self.persist = value,
            3 if value != 0 => self.slot_time = duration,
            3 => return Err(BaycomError::Settings("slot time must be positive")),
            4 => self.tx_tail = duration,
            5 if value == 0 => {}
            5 => return Err(BaycomError::Settings("BayCom supports half duplex only")),
            _ => return Err(BaycomError::Settings("unsupported radio parameter")),
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// The backend must confirm the actual line change with `ptt_changed`.
    Ptt(bool),
    /// Clock every bit at 1200 baud; includes preamble, FCS and tail flags.
    SendBits(Vec<bool>),
    /// Local bit-stream completion, NOT an RF acknowledgement.
    Transmitted(Vec<u8>),
    /// An explicitly failed active frame is removed and never replayed here.
    Failed { frame: Vec<u8>, reason: String },
}

#[derive(Clone, Copy)]
enum State {
    Listening,
    AwaitPttOn,
    Sending,
    AwaitPttOff,
    Turnaround(Instant),
}

/// One radio's FIFO, shared by every logical AX.25 channel using that radio.
pub struct Baycom {
    settings: Settings,
    queue: VecDeque<Vec<u8>>,
    active: Option<Vec<u8>>,
    buffered_bytes: usize,
    next_slot: Option<Instant>,
    state: State,
    receiver: Receiver,
}

impl Baycom {
    pub fn new(settings: Settings) -> Result<Self, BaycomError> {
        if settings.max_frames == 0 || settings.max_bytes == 0 {
            return Err(BaycomError::Settings("queue limits must be nonzero"));
        }
        if settings.slot_time.is_zero()
            || [
                settings.tx_delay,
                settings.tx_tail,
                settings.turnaround,
                settings.slot_time,
            ]
            .iter()
            .any(|&d| d > Duration::from_secs(60))
        {
            return Err(BaycomError::Settings(
                "slot time must be positive; timings must not exceed 60 seconds",
            ));
        }
        Ok(Self {
            settings,
            queue: VecDeque::new(),
            active: None,
            buffered_bytes: 0,
            next_slot: None,
            state: State::Listening,
            receiver: Receiver::default(),
        })
    }

    pub fn buffered_frames(&self) -> usize {
        self.queue.len() + usize::from(self.active.is_some())
    }

    pub fn set_parameter(&mut self, command: u8, value: u8) -> Result<(), BaycomError> {
        self.settings.set_parameter(command, value)
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    pub fn enqueue(&mut self, frame: &[u8]) -> Result<(), BaycomError> {
        validate_frame(frame)?;
        let bytes = self
            .buffered_bytes
            .checked_add(frame.len())
            .ok_or(BaycomError::QueueFull)?;
        if self.buffered_frames() >= self.settings.max_frames || bytes > self.settings.max_bytes {
            return Err(BaycomError::QueueFull);
        }
        self.queue.push_back(frame.to_vec());
        self.buffered_bytes = bytes;
        Ok(())
    }

    /// Call on carrier transitions and timer ticks. `random` is uniform 0..=255.
    pub fn poll(&mut self, now: Instant, carrier_busy: bool, random: u8) -> Vec<Action> {
        if let State::Turnaround(until) = self.state {
            if now < until {
                return Vec::new();
            }
            self.state = State::Listening;
        }
        if !matches!(self.state, State::Listening) {
            return Vec::new();
        }
        if carrier_busy || self.queue.is_empty() {
            self.next_slot = None;
            return Vec::new();
        }
        let due = *self.next_slot.get_or_insert(now + self.settings.slot_time);
        if now < due {
            return Vec::new();
        }
        self.next_slot = Some(now + self.settings.slot_time);
        if random > self.settings.persist {
            return Vec::new();
        }
        self.active = self.queue.pop_front();
        self.next_slot = None;
        self.receiver.reset();
        self.state = State::AwaitPttOn;
        vec![Action::Ptt(true)]
    }

    pub fn ptt_changed(&mut self, keyed: bool, now: Instant) -> Result<Vec<Action>, BaycomError> {
        match (self.state, keyed) {
            (State::AwaitPttOn, true) => {
                let frame = self
                    .active
                    .as_ref()
                    .ok_or(BaycomError::UnexpectedCompletion)?;
                let bits = encode(
                    frame,
                    flag_count(self.settings.tx_delay),
                    flag_count(self.settings.tx_tail),
                );
                self.state = State::Sending;
                Ok(vec![Action::SendBits(bits)])
            }
            (State::AwaitPttOff, false) => {
                self.state = State::Turnaround(now + self.settings.turnaround);
                self.receiver.reset();
                Ok(Vec::new())
            }
            _ => Err(BaycomError::UnexpectedCompletion),
        }
    }

    /// Called only after the backend has physically emitted the entire burst.
    pub fn transmit_complete(&mut self) -> Result<Vec<Action>, BaycomError> {
        if !matches!(self.state, State::Sending) {
            return Err(BaycomError::UnexpectedCompletion);
        }
        let frame = self
            .active
            .take()
            .ok_or(BaycomError::UnexpectedCompletion)?;
        self.buffered_bytes -= frame.len();
        self.state = State::AwaitPttOff;
        Ok(vec![Action::Transmitted(frame), Action::Ptt(false)])
    }

    pub fn fail_transmission(&mut self, reason: String) -> Result<Vec<Action>, BaycomError> {
        let frame = self
            .active
            .take()
            .ok_or(BaycomError::UnexpectedCompletion)?;
        self.buffered_bytes -= frame.len();
        self.state = State::AwaitPttOff;
        Ok(vec![Action::Failed { frame, reason }, Action::Ptt(false)])
    }

    /// Recovered HDLC bits, after NRZI decoding; self-reception is suppressed.
    pub fn receive_bit(&mut self, bit: bool) -> Option<Result<Vec<u8>, BaycomError>> {
        if !matches!(self.state, State::Listening) {
            return None;
        }
        self.receiver.push(bit)
    }
}

fn flag_count(duration: Duration) -> usize {
    // Settings cap durations at 60s, so this also fits a 32-bit usize.
    ((duration.as_nanos() * u128::from(BIT_RATE) + 7_999_999_999) / 8_000_000_000).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receive(bits: impl IntoIterator<Item = bool>) -> Vec<Result<Vec<u8>, BaycomError>> {
        let mut rx = Receiver::default();
        bits.into_iter().filter_map(|bit| rx.push(bit)).collect()
    }

    #[test]
    fn baycom_fcs_and_nrzi_match_known_vectors() {
        assert_eq!(fcs(b"123456789"), 0x906e);
        let mut tx = Nrzi::new(true);
        let mut rx = Nrzi::new(true);
        let bits = [true, false, false, true, false];
        let levels: Vec<_> = bits.into_iter().map(|bit| tx.encode(bit)).collect();
        assert_eq!(levels, [true, false, true, true, false]);
        assert_eq!(
            levels
                .into_iter()
                .map(|level| rx.decode(level))
                .collect::<Vec<_>>(),
            bits
        );

        let mut known_wire = Vec::new();
        flag(&mut known_wire);
        for byte in b"123456789".iter().copied().chain([0x6e, 0x90]) {
            known_wire.extend((0..8).map(|shift| byte & (1 << shift) != 0));
        }
        flag(&mut known_wire);
        assert_eq!(receive(known_wire), [Ok(b"123456789".to_vec())]);
    }

    #[test]
    fn baycom_streaming_roundtrip_handles_stuffing_shared_flags_and_binary() {
        let first = vec![0xff; MAX_FRAME_BYTES];
        let second: Vec<_> = (0..=255).collect();
        let mut bits = encode_frame(&first).unwrap();
        let mut ones = 0;
        for &bit in &bits[8..bits.len() - 8] {
            ones = if bit { ones + 1 } else { 0 };
            assert!(ones <= 5, "unescaped flag/abort pattern in payload");
        }
        bits.extend(&encode_frame(&second).unwrap()[8..]);
        let mut rx = Receiver::default();
        let mut frames = Vec::new();
        for chunk in bits.chunks(3) {
            frames.extend(chunk.iter().filter_map(|&bit| rx.push(bit)));
        }
        assert_eq!(frames, [Ok(first), Ok(second)]);
    }

    #[test]
    fn baycom_receiver_reports_corruption_abort_overflow_and_recovers() {
        let mut corrupt = encode_frame(b"hello").unwrap();
        corrupt[9] = !corrupt[9];
        assert_eq!(receive(corrupt), [Err(BaycomError::Fcs)]);
        let mut stream = Vec::new();
        flag(&mut stream);
        stream.extend([true; 8]);
        flag(&mut stream);
        stream.extend(std::iter::repeat(false).take(MAX_STUFFED_BITS + 1));
        stream.extend(encode_frame(b"recovered").unwrap());
        assert_eq!(
            receive(stream),
            [
                Err(BaycomError::Abort),
                Err(BaycomError::ReceiveOverflow),
                Ok(b"recovered".to_vec()),
            ]
        );
        let mut partial = Vec::new();
        flag(&mut partial);
        partial.push(true);
        flag(&mut partial);
        assert_eq!(receive(partial), [Err(BaycomError::Unaligned)]);
    }

    #[test]
    fn baycom_queue_bounds_include_active_frames_and_failed_data_is_not_replayed() {
        let settings = Settings {
            max_frames: 2,
            max_bytes: 6,
            ..Settings::default()
        };
        let mut modem = Baycom::new(settings).unwrap();
        assert_eq!(modem.enqueue(&[]), Err(BaycomError::FrameLength(0)));
        assert!(modem.enqueue(&[0; MAX_FRAME_BYTES + 1]).is_err());
        modem.enqueue(b"one").unwrap();
        assert_eq!(modem.enqueue(b"four"), Err(BaycomError::QueueFull));
        modem.enqueue(b"two").unwrap();
        assert_eq!(modem.enqueue(b"x"), Err(BaycomError::QueueFull));
        let now = Instant::now();
        assert!(modem.poll(now, false, 0).is_empty());
        assert_eq!(
            modem.poll(now + Duration::from_millis(100), false, 0),
            [Action::Ptt(true)]
        );
        assert_eq!((modem.buffered_frames(), modem.buffered_bytes()), (2, 6));
        modem
            .ptt_changed(true, now + Duration::from_millis(100))
            .unwrap();
        assert_eq!(
            modem.fail_transmission("backend failed".into()).unwrap(),
            [
                Action::Failed {
                    frame: b"one".to_vec(),
                    reason: "backend failed".into()
                },
                Action::Ptt(false),
            ]
        );
        assert_eq!((modem.buffered_frames(), modem.buffered_bytes()), (1, 3));
        modem
            .ptt_changed(false, now + Duration::from_millis(110))
            .unwrap();
        assert!(modem
            .poll(now + Duration::from_millis(209), false, 0)
            .is_empty());
        assert!(modem
            .poll(now + Duration::from_millis(210), false, 0)
            .is_empty());
        assert_eq!(
            modem.poll(now + Duration::from_millis(310), false, 0),
            [Action::Ptt(true)]
        );
        let actions = modem
            .ptt_changed(true, now + Duration::from_millis(310))
            .unwrap();
        let [Action::SendBits(bits)] = actions.as_slice() else {
            panic!("expected bit stream")
        };
        assert_eq!(receive(bits.iter().copied()), [Ok(b"two".to_vec())]);
    }

    #[test]
    fn baycom_half_duplex_waits_for_carrier_slots_ptt_and_actual_completion() {
        let mut modem = Baycom::new(Settings {
            tx_delay: Duration::from_millis(203),
            tx_tail: Duration::from_millis(17),
            ..Settings::default()
        })
        .unwrap();
        let now = Instant::now();
        modem.enqueue(b"first").unwrap();
        modem.enqueue(b"second").unwrap();
        assert!(modem.poll(now, true, 0).is_empty());
        assert!(modem.poll(now + Duration::from_secs(1), true, 0).is_empty());
        let clear = now + Duration::from_secs(2);
        assert!(modem.poll(clear, false, 0).is_empty());
        assert!(modem
            .poll(clear + Duration::from_millis(99), false, 0)
            .is_empty());
        assert!(modem
            .poll(clear + Duration::from_millis(100), false, 255)
            .is_empty());
        assert!(modem
            .poll(clear + Duration::from_millis(150), false, 0)
            .is_empty());
        let start = clear + Duration::from_millis(200);
        assert_eq!(modem.poll(start, false, 63), [Action::Ptt(true)]);
        let keyed = start + Duration::from_secs(1);
        assert!(modem.poll(keyed, false, 0).is_empty());
        assert_eq!(
            modem.transmit_complete(),
            Err(BaycomError::UnexpectedCompletion)
        );
        let actions = modem.ptt_changed(true, keyed).unwrap();
        let [Action::SendBits(bits)] = actions.as_slice() else {
            panic!("expected bit stream")
        };
        // 203 ms rounds up to 31 flags; 17 ms rounds up to 3 flags at 1200 baud.
        assert_eq!(
            bits.len(),
            encode_frame(b"first").unwrap().len() + (30 + 2) * 8
        );
        assert_eq!(receive(bits.iter().copied()), [Ok(b"first".to_vec())]);
        for bit in encode_frame(b"self echo").unwrap() {
            assert!(modem.receive_bit(bit).is_none());
        }
        assert_eq!(
            modem.transmit_complete().unwrap(),
            [Action::Transmitted(b"first".to_vec()), Action::Ptt(false)]
        );
        assert_eq!(
            modem.transmit_complete(),
            Err(BaycomError::UnexpectedCompletion)
        );
        let released = keyed + Duration::from_secs(1);
        assert!(modem.poll(released, false, 0).is_empty());
        modem.ptt_changed(false, released).unwrap();
        assert!(modem
            .poll(released + Duration::from_millis(99), false, 0)
            .is_empty());
        assert!(modem
            .poll(released + Duration::from_millis(100), true, 0)
            .is_empty());
        let received: Vec<_> = encode_frame(b"peer reply")
            .unwrap()
            .into_iter()
            .filter_map(|bit| modem.receive_bit(bit))
            .collect();
        assert_eq!(received, [Ok(b"peer reply".to_vec())]);
        assert_eq!((modem.buffered_frames(), modem.buffered_bytes()), (1, 6));
    }

    #[test]
    fn baycom_rejects_unbounded_or_nonprogressing_settings() {
        for settings in [
            Settings {
                slot_time: Duration::ZERO,
                ..Settings::default()
            },
            Settings {
                max_frames: 0,
                ..Settings::default()
            },
            Settings {
                max_bytes: 0,
                ..Settings::default()
            },
            Settings {
                tx_delay: Duration::from_secs(61),
                ..Settings::default()
            },
        ] {
            assert!(matches!(
                Baycom::new(settings),
                Err(BaycomError::Settings(_))
            ));
        }
    }

    #[tokio::test]
    async fn baycom_rejects_invalid_radio_configuration_before_opening_hardware() {
        for (baud, port, expected) in [(9600, 0, "1200 bit/s"), (1200, 1, "one radio only")] {
            let config = crate::config::TncConfig {
                protocol: "baycom".into(),
                kiss_port: port,
                ..crate::config::TncConfig::default()
            };
            let serial = crate::config::SerialConfig {
                port: "not-a-real-serial-port".into(),
                baud,
                ..crate::config::SerialConfig::default()
            };
            let (mut channels, worker) = crate::tnc::spawn_tnc_task(serial, config);
            let reason = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let crate::tnc::TncEvent::LinkDown(reason) = channels
                        .events
                        .recv()
                        .await
                        .expect("driver closed without an error")
                    {
                        break reason;
                    }
                }
            })
            .await
            .unwrap();
            assert!(reason.contains(expected), "{reason}");
            drop(channels);
            worker.await.unwrap();
        }
    }

    #[test]
    fn baycom_live_parameters_preserve_queue_and_reject_invalid_simplex_settings() {
        let mut modem = Baycom::new(Settings::default()).unwrap();
        modem.enqueue(b"queued").unwrap();
        modem.set_parameter(1, 1).unwrap();
        modem.set_parameter(2, 255).unwrap();
        modem.set_parameter(4, 0).unwrap();
        for (command, value) in [(3, 0), (5, 1), (6, 0)] {
            assert!(modem.set_parameter(command, value).is_err());
        }
        let now = Instant::now();
        assert!(modem.poll(now, false, 0).is_empty());
        assert!(modem
            .poll(now + Duration::from_millis(99), false, 0)
            .is_empty());
        assert_eq!(
            modem.poll(now + Duration::from_millis(100), false, 0),
            [Action::Ptt(true)]
        );
        let actions = modem
            .ptt_changed(true, now + Duration::from_millis(100))
            .unwrap();
        let [Action::SendBits(bits)] = actions.as_slice() else {
            panic!("missing burst");
        };
        assert_eq!(receive(bits.iter().copied()), [Ok(b"queued".to_vec())]);
        assert_eq!(bits.len(), encode_frame(b"queued").unwrap().len() + 8);
        assert_eq!(modem.buffered_frames(), 1);
    }
}
