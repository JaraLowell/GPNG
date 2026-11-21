//! Calls TFPCX has already heard, and the logical port they were on.
//!
//! The table stays in memory while the switch is running. It is loaded from
//! `data/TFPCX.HOSTS` at start and written back when the switch closes.
//! Our own calls are never stored. A station not heard for four days is dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};

/// Where to send a frame whose destination is not one of our calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// One known KISS port, or every KISS port when nothing is known yet.
    Kiss(Vec<u8>),
    /// The call or a via is stored on a port this send cannot use (WA8DED).
    Other(u8),
}

#[derive(Debug, Clone)]
struct Probe {
    calls: Vec<String>,
}

/// How long a heard station stays in the table after its last frame.
const KEEP: Duration = Duration::days(4);

#[derive(Debug, Clone)]
struct Heard {
    port: u8,
    seen: DateTime<Local>,
}

/// Heard stations. `CALL port yyyy-mm-dd HH:MM` lines in `data/TFPCX.HOSTS`.
#[derive(Debug, Clone)]
pub struct KnownHosts {
    path: PathBuf,
    hosts: BTreeMap<String, Heard>,
    own: BTreeSet<String>,
    own_bases: BTreeSet<String>,
    pending: Vec<Probe>,
}

impl KnownHosts {
    pub fn load(path: impl Into<PathBuf>, own: &[String]) -> Self {
        let path = path.into();
        let mut hosts = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let Some(call) = parts.next() else { continue };
                let Some(port) = parts.next().and_then(|p| p.parse::<u8>().ok()) else {
                    continue;
                };
                if port > 15 {
                    continue;
                }
                let seen = match (parts.next(), parts.next()) {
                    (None, _) => Local::now(),
                    (Some(date), Some(time)) => match parse_seen(date, time) {
                        Some(seen) => seen,
                        None => continue,
                    },
                    _ => continue,
                };
                if parts.next().is_some() || !fresh(seen, Local::now()) {
                    continue;
                }
                hosts.insert(normalize(call), Heard { port, seen });
            }
        }
        let mut book = Self {
            path,
            hosts,
            own: BTreeSet::new(),
            own_bases: BTreeSet::new(),
            pending: Vec::new(),
        };
        for call in own {
            book.own(call);
        }
        let drop_calls: Vec<String> = book
            .hosts
            .keys()
            .filter(|call| book.is_own(call))
            .cloned()
            .collect();
        for call in drop_calls {
            book.hosts.remove(&call);
        }
        book
    }

    pub fn own(&mut self, call: &str) {
        let call = normalize(call);
        if call.is_empty() {
            return;
        }
        self.own_bases.insert(base(&call).to_string());
        self.own.insert(call);
    }

    pub fn is_own(&self, call: &str) -> bool {
        let call = normalize(call);
        self.own.contains(&call) || self.own_bases.contains(base(&call))
    }

    /// Remember a station heard on `port`. Own calls are ignored.
    /// The file is left untouched until [`Self::save`].
    pub fn remember(&mut self, call: &str, port: u8) -> bool {
        if port > 15 || self.is_own(call) {
            return false;
        }
        let call = normalize(call);
        if call.is_empty() {
            return false;
        }
        self.hosts.insert(
            call,
            Heard {
                port,
                seen: Local::now(),
            },
        );
        true
    }

    /// First send to an unknown call or via. Later answers lock every name in it.
    pub fn note_probe(&mut self, dest: &str, vias: &[String]) {
        let calls: Vec<String> = std::iter::once(dest)
            .chain(vias.iter().map(String::as_str))
            .map(normalize)
            .filter(|call| !call.is_empty() && !self.is_own(call))
            .collect();
        if calls.is_empty() {
            return;
        }
        if self.pending.iter().any(|probe| probe.calls == calls) {
            return;
        }
        self.pending.push(Probe { calls });
    }

    /// A frame from `src` arrived on `port`. Lock that call, and any probe it answers.
    pub fn answer(&mut self, src: &str, port: u8) {
        let src = normalize(src);
        if src.is_empty() || self.is_own(&src) {
            return;
        }
        let mut rest = Vec::new();
        let mut locked = vec![src];
        for probe in self.pending.drain(..) {
            if probe.calls.iter().any(|call| call == &locked[0]) {
                locked.extend(probe.calls);
            } else {
                rest.push(probe);
            }
        }
        self.pending = rest;
        for call in locked {
            self.remember(&call, port);
        }
    }

    pub fn port_of(&self, call: &str) -> Option<u8> {
        if self.is_own(call) {
            return None;
        }
        let heard = self.hosts.get(&normalize(call))?;
        fresh(heard.seen, Local::now()).then_some(heard.port)
    }

    /// Known KISS port for the first via, else the destination.
    /// Unknown names are tried on every KISS port.
    /// Stations last heard four days ago are dropped before the choice is made.
    pub fn route(&mut self, dest: &str, vias: &[String], kiss_ports: &[u8]) -> Route {
        self.prune();
        let known = vias
            .iter()
            .chain(std::iter::once(&dest.to_string()))
            .find_map(|call| self.port_of(call));
        match known {
            Some(port) if kiss_ports.contains(&port) => Route::Kiss(vec![port]),
            Some(port) => Route::Other(port),
            None => Route::Kiss(kiss_ports.to_vec()),
        }
    }

    /// Write the table. Called when the switch closes, not on each heard frame.
    pub fn save(&mut self) -> Result<(), String> {
        self.prune();
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir).map_err(|err| err.to_string())?;
            }
        }
        let mut text = String::from(
            "# call port yyyy-mm-dd HH:MM — TFPCX known hosts. Own calls are not listed.\n",
        );
        for (call, heard) in &self.hosts {
            text.push_str(&format!(
                "{call} {} {}\n",
                heard.port,
                heard.seen.format("%Y-%m-%d %H:%M")
            ));
        }
        fs::write(&self.path, text).map_err(|err| err.to_string())
    }

    fn prune(&mut self) {
        let now = Local::now();
        self.hosts.retain(|_, heard| fresh(heard.seen, now));
    }
}

fn fresh(seen: DateTime<Local>, now: DateTime<Local>) -> bool {
    now.signed_duration_since(seen) < KEEP
}

fn parse_seen(date: &str, time: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M").ok()?;
    Local.from_local_datetime(&naive).single()
}

fn normalize(call: &str) -> String {
    call.trim().to_ascii_uppercase()
}

fn base(call: &str) -> &str {
    call.split('-').next().unwrap_or(call)
}

pub fn default_path() -> PathBuf {
    PathBuf::from("data/TFPCX.HOSTS")
}

/// Wire port `0, 1, …` lines up with the logical ports the operator chose.
pub fn logical_port(logical_ports: &[u8], wire: u8) -> Option<u8> {
    logical_ports.get(usize::from(wire)).copied()
}

pub fn wire_port(logical_ports: &[u8], logical: u8) -> Option<u8> {
    logical_ports
        .iter()
        .position(|port| *port == logical)
        .and_then(|index| u8::try_from(index).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(own: &[&str]) -> KnownHosts {
        let path = std::env::temp_dir().join(format!(
            "gpng-hosts-{}-{}",
            std::process::id(),
            own.join("-")
        ));
        let _ = fs::remove_file(&path);
        let mut hosts = KnownHosts::load(path, &[]);
        for call in own {
            hosts.own(call);
        }
        hosts
    }

    #[test]
    fn unknown_call_is_tried_on_every_kiss_port_then_locked() {
        let mut hosts = book(&["N0ME", "N0ME-1"]);
        assert_eq!(
            hosts.route("N1YOU", &["DIGI".into()], &[0, 1, 4]),
            Route::Kiss(vec![0, 1, 4])
        );
        hosts.note_probe("N1YOU", &["DIGI".into()]);
        assert!(!hosts.remember("N0ME-2", 1));
        hosts.answer("DIGI", 4);
        assert_eq!(hosts.port_of("DIGI"), Some(4));
        assert_eq!(hosts.port_of("N1YOU"), Some(4));
        assert_eq!(hosts.port_of("N0ME"), None);
        assert_eq!(hosts.route("N1YOU", &[], &[0, 1, 4]), Route::Kiss(vec![4]));
    }

    #[test]
    fn a_via_on_a_known_port_wins_over_a_guess() {
        let mut hosts = book(&["N0ME"]);
        hosts.remember("DIGI", 2);
        assert_eq!(
            hosts.route("N1YOU", &["DIGI".into()], &[0, 2]),
            Route::Kiss(vec![2])
        );
        hosts.remember("N2BBS", 5);
        assert_eq!(hosts.route("N2BBS", &[], &[0, 2]), Route::Other(5));
    }

    #[test]
    fn logical_ports_map_onto_wire_ports_in_order() {
        assert_eq!(wire_port(&[2, 5], 5), Some(1));
        assert_eq!(logical_port(&[2, 5], 1), Some(5));
        assert_eq!(wire_port(&[2, 5], 0), None);
    }

    #[test]
    fn heard_calls_stay_in_memory_until_save_and_drop_after_four_days() {
        let path = std::env::temp_dir().join(format!(
            "gpng-hosts-age-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_file(&path);
        let mut hosts = KnownHosts::load(&path, &["N0ME".into()]);
        assert!(hosts.remember("NL1MSK", 1));
        assert!(!path.exists());
        hosts.answer("NL1MSK", 1);
        hosts.save().unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("NL1MSK 1 "));
        hosts.remember("NL2NEW", 0);
        assert_eq!(fs::read_to_string(&path).unwrap(), written);

        let stale = (Local::now() - Duration::days(5)).format("%Y-%m-%d %H:%M");
        let fresh = Local::now().format("%Y-%m-%d %H:%M");
        fs::write(
            &path,
            format!("# call port yyyy-mm-dd HH:MM\nOLD1 2 {stale}\nNL1MSK 1 {fresh}\n"),
        )
        .unwrap();
        let loaded = KnownHosts::load(&path, &["N0ME".into()]);
        assert_eq!(loaded.port_of("OLD1"), None);
        assert_eq!(loaded.port_of("NL1MSK"), Some(1));
        assert_eq!(loaded.port_of("N0ME"), None);
        let _ = fs::remove_file(&path);
    }
}
