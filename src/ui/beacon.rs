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

fn transmit(snap: &BeaconSnap) -> Attempt {
    let raw = match snap.beacon.body() {
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
        name_unknown: false,
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
    use super::{beacon_on_air, mail_beacon_line, transmit, Attempt, BeaconClock, BeaconSnap};
    use crate::config::BeaconConfig;
    use crate::remote::macros::MacroContext;
    use crate::tnc::ax25::{self, Ax25FrameType};
    use crate::tnc::TncCommand;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn period_waits_then_a_long_gap_sends_once() {
        let mut clock = BeaconClock::default();
        let period = Duration::from_secs(60 * 60);
        let t0 = at(1_000_000);
        assert!(!clock.due(period, t0));
        assert!(!clock.due(period, t0 + Duration::from_secs(60 * 60 - 1)));
        let later = t0 + Duration::from_secs(3 * 60 * 60);
        assert!(clock.due(period, later));
        clock.commit(later);
        assert!(!clock.due(period, later + Duration::from_secs(30)));
        assert!(clock.due(period, later + period));
    }

    #[test]
    fn disarm_starts_a_fresh_period() {
        let mut clock = BeaconClock::default();
        let period = Duration::from_secs(60);
        let t0 = at(5_000);
        assert!(!clock.due(period, t0));
        assert!(clock.due(period, t0 + period));
        clock.disarm();
        let again = t0 + period + Duration::from_secs(5);
        assert!(!clock.due(period, again));
        assert!(!clock.due(period, again + period - Duration::from_secs(1)));
        assert!(clock.due(period, again + period));
    }

    #[test]
    fn backwards_clock_restarts_the_wait() {
        let mut clock = BeaconClock::default();
        let period = Duration::from_secs(60);
        let t0 = at(8_000);
        assert!(!clock.due(period, t0));
        let back = t0 - Duration::from_secs(10);
        assert!(!clock.due(period, back));
        assert!(!clock.due(period, back + period - Duration::from_secs(1)));
        assert!(clock.due(period, back + period));
    }

    #[test]
    fn text_and_file_expand_the_same_macros() {
        let mut ctx = MacroContext::demo();
        ctx.my_call = "NL0MSK".into();
        ctx.qrg = "430.625".into();
        let line = "de %Y qrg %Q";
        assert_eq!(beacon_on_air(line, false, &ctx), "de NL0MSK qrg 430.625");
        assert_eq!(beacon_on_air(line, true, &ctx), "de NL0MSK qrg 430.625");
        let long = format!("{}%Y", "x".repeat(80));
        assert_eq!(beacon_on_air(&long, false, &ctx).chars().count(), 81);
    }

    fn snap(text: &str, tx: tokio::sync::mpsc::Sender<TncCommand>) -> BeaconSnap {
        let mut beacon = BeaconConfig::default();
        beacon.period_minutes = 60;
        beacon.text = text.into();
        BeaconSnap {
            kiss: true,
            beacon,
            my_call: "NL0MSK".into(),
            qrg: "430.625".into(),
            version: "test".into(),
            cmd_tx: tx,
            mail_dir: PathBuf::from("no-such-mail-dir"),
        }
    }

    fn ui_info(cmd: TncCommand) -> Vec<u8> {
        let TncCommand::HostFrame { channel, data } = cmd else {
            panic!("expected a host frame");
        };
        assert_eq!(channel, 0);
        ax25::parse_frame(&data).unwrap().info
    }

    #[test]
    fn transmit_queues_one_ui_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        match transmit(&snap("de %Y", tx)) {
            Attempt::Sent(note) => assert_eq!(note, "beacon NL0MSK to CQ"),
            other => panic!("expected a send, got {other:?}"),
        }
        let TncCommand::HostFrame { channel, data } = rx.try_recv().unwrap() else {
            panic!("expected a host frame");
        };
        assert_eq!(channel, 0);
        let frame = ax25::parse_frame(&data).unwrap();
        assert_eq!(frame.kind, Ax25FrameType::Ui);
        assert_eq!(frame.src, "NL0MSK");
        assert_eq!(frame.dest, "CQ");
        assert_eq!(frame.info, b"de NL0MSK");
    }

    #[test]
    fn full_radio_queue_retries_without_dropping_the_period() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        tx.try_send(TncCommand::Shutdown).unwrap();
        assert!(matches!(transmit(&snap("hi", tx)), Attempt::Retry));
    }

    #[test]
    fn mail_line_lists_seven_and_folds_the_rest() {
        let one = vec!["NL1AS".into()];
        assert_eq!(
            mail_beacon_line("NL0MSK", &one).as_deref(),
            Some("New Mail at NL0MSK for: NL1AS")
        );
        assert!(mail_beacon_line("NL0MSK", &[]).is_none());
        assert!(mail_beacon_line("  ", &one).is_none());
        let seven: Vec<String> = (1..=7).map(|n| format!("N{n}CALL")).collect();
        assert_eq!(
            mail_beacon_line("NL0MSK", &seven).unwrap(),
            "New Mail at NL0MSK for: N1CALL, N2CALL, N3CALL, N4CALL, N5CALL, N6CALL, N7CALL"
        );
        let eight: Vec<String> = (1..=8).map(|n| format!("N{n}CALL")).collect();
        assert_eq!(
            mail_beacon_line("NL0MSK", &eight).unwrap(),
            "New Mail at NL0MSK for: N1CALL, N2CALL, N3CALL, N4CALL, N5CALL, N6CALL and others"
        );
    }

    #[test]
    fn transmit_follows_with_a_mail_beacon() {
        let dir = std::env::temp_dir().join(format!("gpng-beacon-mail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("NL1AS.GPM"), b"hello").unwrap();
        std::fs::write(dir.join("PD4MV.GPM"), b"hello").unwrap();
        std::fs::write(dir.join("README.txt"), b"skip").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut beacon = snap("de %Y", tx);
        beacon.mail_dir = dir.clone();
        match transmit(&beacon) {
            Attempt::Sent(note) => {
                assert!(note.contains("beacon NL0MSK to CQ"));
                assert!(note.contains("New Mail at NL0MSK for: NL1AS, PD4MV"));
            }
            other => panic!("expected a send, got {other:?}"),
        }
        assert_eq!(ui_info(rx.try_recv().unwrap()), b"de NL0MSK");
        let TncCommand::HostFrame { data, .. } = rx.try_recv().unwrap() else {
            panic!("expected the mail frame");
        };
        let mail = ax25::parse_frame(&data).unwrap();
        assert_eq!(mail.dest, "CQ");
        assert_eq!(mail.src, "NL0MSK");
        assert!(mail.via.is_empty());
        assert_eq!(mail.info, b"New Mail at NL0MSK for: NL1AS, PD4MV");
        assert!(rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_beacon_also_sends_the_mail_notice() {
        let dir = std::env::temp_dir().join(format!(
            "gpng-beacon-mail-file-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body = dir.join("beacon.txt");
        std::fs::write(&body, b"on air").unwrap();
        std::fs::write(dir.join("NL1AS.GPM"), b"hello").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut beacon = snap("", tx);
        beacon.beacon.set_uses_file(true);
        beacon.beacon.file = body.display().to_string();
        beacon.mail_dir = dir.clone();
        assert!(matches!(transmit(&beacon), Attempt::Sent(_)));
        assert_eq!(ui_info(rx.try_recv().unwrap()), b"on air");
        assert_eq!(
            ui_info(rx.try_recv().unwrap()),
            b"New Mail at NL0MSK for: NL1AS"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_skips_the_period() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let mut beacon = snap("", tx);
        beacon.beacon.set_uses_file(true);
        beacon.beacon.file = "no-such-beacon-file".into();
        match transmit(&beacon) {
            Attempt::Skip(note) => assert!(note.contains("cannot be read")),
            other => panic!("expected a skip, got {other:?}"),
        }
    }
}
