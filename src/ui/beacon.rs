//! Monitor beacon clock.
//!
//! The period is wall-clock time and the send runs on its own thread. The
//! window paint loop is the wrong place for it: egui only runs `update` when
//! Windows delivers a paint, and a minimized or covered window stops getting
//! those. `Instant` also stands still across sleep, so a 60-minute setting
//! would quietly never come due. This thread keeps sending one UI frame per
//! period either way. A long gap (sleep, a stalled window) sends once, then
//! waits the full period again. When that send goes out and `data/MAIL` has
//! `{CALL}.GPM` files, a second UI frame follows to the same address.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use chrono::Local;
use eframe::egui::Context;
use tokio::sync::mpsc;

use crate::config::{clip_beacon, BeaconConfig, BEACON_FILE_MAX, BEACON_TEXT_MAX};
use crate::remote::gpi;
use crate::remote::macros::{self, MacroContext};
use crate::tnc::ax25;
use crate::tnc::TncCommand;

/// Values the paint loop copies in. The thread reads a clone and does not
/// hold the lock while it reads the beacon file.
#[derive(Clone)]
pub struct BeaconSnap {
    pub kiss: bool,
    pub beacon: BeaconConfig,
    pub my_call: String,
    pub qrg: String,
    pub version: String,
    pub cmd_tx: mpsc::Sender<TncCommand>,
    /// `data/MAIL`. Read when a beacon goes out, so a closed window still
    /// sees mail that arrived since the last paint.
    pub mail_dir: PathBuf,
}

/// Background period. Dropping it asks the thread to stop.
pub struct BeaconSchedule {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

struct Shared {
    snap: BeaconSnap,
    note: Option<String>,
}

impl BeaconSchedule {
    pub fn start(ctx: Context, snap: BeaconSnap) -> Self {
        let shared = Arc::new(Mutex::new(Shared { snap, note: None }));
        let stop = Arc::new(AtomicBool::new(false));
        let shared_thread = Arc::clone(&shared);
        let stop_thread = Arc::clone(&stop);
        thread::Builder::new()
            .name("beacon".into())
            .spawn(move || run(ctx, shared_thread, stop_thread))
            .expect("beacon thread");
        Self { shared, stop }
    }

    /// Latest setup, callsign, QRG, and radio sender.
    pub fn publish(&self, snap: BeaconSnap) {
        lock(&self.shared).snap = snap;
    }

    /// Send independently of the scheduled clock.
    pub fn send_now(&self) -> String {
        let snap = lock(&self.shared).snap.clone();
        send_now(&snap)
    }

    /// Status from the last send. One take, so a later frame does not repeat it.
    pub fn take_note(&self) -> Option<String> {
        lock(&self.shared).note.take()
    }
}

impl Drop for BeaconSchedule {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn run(ctx: Context, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    let mut clock = BeaconClock::default();
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(1));
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let snap = lock(&shared).snap.clone();
        let now = SystemTime::now();
        let period = period_of(&snap.beacon);
        if !snap.beacon.armed() || period.is_zero() {
            clock.disarm();
            continue;
        }
        if !clock.due(period, now) {
            continue;
        }
        // Hostmode stores the beacon and does not invent a TNC command.
        // Count the period anyway so switching to KISS later is not a burst.
        if !snap.kiss {
            clock.commit(now);
            continue;
        }
        match transmit(&snap) {
            Attempt::Sent(note) | Attempt::Skip(note) => {
                clock.commit(now);
                lock(&shared).note = Some(note);
                ctx.request_repaint();
            }
            // The radio queue did not take the frame. Stay due and try again.
            Attempt::Retry => {}
        }
    }
}

/// Wait since the beacon was armed, or since the last send.
#[derive(Debug, Default)]
struct BeaconClock {
    wait_from: Option<SystemTime>,
}

impl BeaconClock {
    /// `true` when the period has elapsed.
    ///
    /// The first call after [`Self::disarm`] starts the wait and returns
    /// false, so turning the beacon on does not transmit immediately.
    /// A clock step backwards restarts that wait instead of panicking.
    fn due(&mut self, period: Duration, now: SystemTime) -> bool {
        if period.is_zero() {
            self.wait_from = None;
            return false;
        }
        let Some(start) = self.wait_from else {
            self.wait_from = Some(now);
            return false;
        };
        match now.duration_since(start) {
            Ok(elapsed) if elapsed >= period => true,
            Ok(_) => false,
            Err(_) => {
                self.wait_from = Some(now);
                false
            }
        }
    }

    fn commit(&mut self, now: SystemTime) {
        self.wait_from = Some(now);
    }

    fn disarm(&mut self) {
        self.wait_from = None;
    }
}

fn period_of(beacon: &BeaconConfig) -> Duration {
    Duration::from_secs(u64::from(beacon.period_minutes) * 60)
}

#[derive(Debug)]
enum Attempt {
    Sent(String),
    /// Nothing to put on the air. Wait the period before saying so again.
    Skip(String),
    /// `try_send` did not take the frame. Try on the next tick.
    Retry,
}

/// Send once even when the periodic beacon is disabled.
pub fn send_now(snap: &BeaconSnap) -> String {
    if !snap.kiss {
        return "Manual beacon requires KISS mode".into();
    }
    match transmit(snap) {
        Attempt::Sent(note) | Attempt::Skip(note) => note,
        Attempt::Retry => "Beacon not queued: radio queue is full or closed".into(),
    }
}

fn transmit(snap: &BeaconSnap) -> Attempt {
    let raw = match snap.beacon.body_for_transmit() {
        Ok(Some(text)) => text,
        Ok(None) => {
            if snap.beacon.uses_file() {
                return Attempt::Skip("Beacon file is empty".into());
            }
            return Attempt::Skip("Beacon is empty".into());
        }
        Err(err) => return Attempt::Skip(err),
    };
    let text = beacon_on_air(&raw, snap.beacon.uses_file(), &macro_ctx(snap));
    if text.is_empty() {
        return Attempt::Skip("Beacon is empty".into());
    }
    let (dest, vias) = ax25::parse_unproto(&snap.beacon.address);
    let via_ref: Vec<&str> = vias.iter().map(String::as_str).collect();
    let frame = ax25::build_ui(&dest, &snap.my_call, &via_ref, text.as_bytes());
    match snap.cmd_tx.try_send(TncCommand::HostFrame {
        channel: 0,
        data: frame,
    }) {
        Ok(()) => {
            let where_to = ax25::unproto_to(&dest, &vias);
            let note = match queue_mail_beacon(snap, &dest, &via_ref) {
                MailSend::None => format!("beacon {} {}", snap.my_call, where_to),
                MailSend::Sent(line) => format!("beacon {} {}; {line}", snap.my_call, where_to),
                // The beacon already left. Do not retry it; the mail line
                // waits for the next period.
                MailSend::NotQueued => {
                    format!("beacon {} {} (mail notice not queued)", snap.my_call, where_to)
                }
            };
            Attempt::Sent(note)
        }
        Err(_) => Attempt::Retry,
    }
}

enum MailSend {
    None,
    Sent(String),
    NotQueued,
}

/// Same destination as the beacon that just queued. No frame when nobody
/// has a `.GPM` file.
fn queue_mail_beacon(snap: &BeaconSnap, dest: &str, via: &[&str]) -> MailSend {
    let calls = crate::remote::mail::waiting_calls(&snap.mail_dir);
    let Some(line) = mail_beacon_line(&snap.my_call, &calls) else {
        return MailSend::None;
    };
    let frame = ax25::build_ui(dest, &snap.my_call, via, line.as_bytes());
    match snap.cmd_tx.try_send(TncCommand::HostFrame {
        channel: 0,
        data: frame,
    }) {
        Ok(()) => MailSend::Sent(line),
        Err(_) => MailSend::NotQueued,
    }
}

/// `New Mail at {call} for: …`
///
/// Up to seven callsigns, separated by `", "`. More than seven drops the
/// seventh call and ends with ` and others` (no comma before it).
fn mail_beacon_line(mycall: &str, calls: &[String]) -> Option<String> {
    let mycall = mycall.trim();
    if mycall.is_empty() || calls.is_empty() {
        return None;
    }
    let list = if calls.len() > 7 {
        let shown: Vec<&str> = calls.iter().take(6).map(String::as_str).collect();
        format!("{} and others", shown.join(", "))
    } else {
        calls.join(", ")
    };
    Some(format!("New Mail at {mycall} for: {list}"))
}

fn macro_ctx(snap: &BeaconSnap) -> MacroContext {
    let now = Local::now();
    MacroContext {
        version: snap.version.clone(),
        peer_call: String::new(),
        peer_name: String::new(),
        my_call: snap.my_call.clone(),
        channel: 0,
        time: now.format("%H:%M:%S").to_string(),
        date: now.format("%d-%m-%Y").to_string(),
        zone: macros::local_zone(),
        news_hint: if gpi::news_available() {
            "News available — //NEws".into()
        } else {
            String::new()
        },
        origin_line: gpi::random_origin_line(),
        digi: String::new(),
        qrg: snap.qrg.clone(),
        activity: String::new(),
        name_unknown: false,
        home: macros::user_home(
            &snap
                .mail_dir
                .parent()
                .map(|dir| dir.join("USER"))
                .unwrap_or_else(|| std::path::PathBuf::from("data").join("USER")),
        ),
    }
}

/// Expand `%` macros, then keep the beacon inside one UI frame.
pub fn beacon_on_air(raw: &str, from_file: bool, ctx: &MacroContext) -> String {
    let max = if from_file {
        BEACON_FILE_MAX
    } else {
        BEACON_TEXT_MAX
    };
    clip_beacon(macros::expand(raw, ctx).trim(), max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> (BeaconSnap, mpsc::Receiver<TncCommand>) {
        let (cmd_tx, rx) = mpsc::channel(2);
        let beacon = BeaconConfig {
            address: "CQ WIDE1-1".into(),
            text: "Hello beacon".into(),
            ..BeaconConfig::default()
        };
        (
            BeaconSnap {
                kiss: true,
                beacon,
                my_call: "N0CALL".into(),
                qrg: "145.500".into(),
                version: "test".into(),
                cmd_tx,
                mail_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            },
            rx,
        )
    }

    #[test]
    fn manual_beacon_sends_with_timer_disabled() {
        let (snap, mut rx) = snapshot();
        assert!(send_now(&snap).starts_with("beacon N0CALL"));
        let TncCommand::HostFrame { channel, data } = rx.try_recv().unwrap() else {
            panic!("expected beacon frame");
        };
        assert_eq!(channel, 0);
        let frame = ax25::parse_frame(&data).unwrap();
        assert_eq!(frame.dest, "CQ");
        assert_eq!(frame.src, "N0CALL");
        assert_eq!(frame.via, ["WIDE1-1"]);
        assert_eq!(frame.info, b"Hello beacon");
    }

    #[test]
    fn manual_beacon_does_not_change_schedule() {
        let (mut snap, _rx) = snapshot();
        snap.beacon.period_minutes = 1;
        let start = SystemTime::UNIX_EPOCH;
        let mut clock = BeaconClock::default();
        let period = period_of(&snap.beacon);
        assert!(!clock.due(period, start));
        assert!(send_now(&snap).starts_with("beacon N0CALL"));
        assert!(!clock.due(period, start + Duration::from_secs(59)));
        assert!(clock.due(period, start + Duration::from_secs(60)));
    }

    #[test]
    fn manual_beacon_reports_unsupported_empty_and_full_queue() {
        let (mut snap, mut rx) = snapshot();
        snap.kiss = false;
        assert_eq!(send_now(&snap), "Manual beacon requires KISS mode");
        assert!(rx.try_recv().is_err());
        snap.kiss = true;
        snap.beacon.text.clear();
        assert_eq!(send_now(&snap), "Beacon is empty");
        assert!(rx.try_recv().is_err());
        snap.beacon.text = "Hello beacon".into();
        assert!(send_now(&snap).starts_with("beacon N0CALL"));
        assert!(send_now(&snap).starts_with("beacon N0CALL"));
        assert_eq!(
            send_now(&snap),
            "Beacon not queued: radio queue is full or closed"
        );
    }
}
