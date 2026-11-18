//! Calls TFPCX has already heard, and the logical port they were on.
//!
//! Our own calls are never stored. The file is written only while TFPCX is running.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

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

/// `CALL port` lines in `data/TFPCX.HOSTS`.
#[derive(Debug, Clone)]
pub struct KnownHosts {
    path: PathBuf,
    hosts: BTreeMap<String, u8>,
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
                hosts.insert(normalize(call), port);
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

    /// Remember a station heard or answered on `port`. Own calls are ignored.
    pub fn remember(&mut self, call: &str, port: u8) -> bool {
        if port > 15 || self.is_own(call) {
            return false;
        }
        let call = normalize(call);
        if call.is_empty() {
            return false;
        }
        if self.hosts.get(&call) == Some(&port) {
            return false;
        }
        self.hosts.insert(call, port);
        let _ = self.save();
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
        self.hosts.get(&normalize(call)).copied()
    }

    /// Known KISS port for the first via, else the destination.
    /// Unknown names are tried on every KISS port.
    pub fn route(&self, dest: &str, vias: &[String], kiss_ports: &[u8]) -> Route {
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

    fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir).map_err(|err| err.to_string())?;
            }
        }
        let mut text = String::from("# call port — TFPCX known hosts. Own calls are not listed.\n");
        for (call, port) in &self.hosts {
            text.push_str(&format!("{call} {port}\n"));
        }
        fs::write(&self.path, text).map_err(|err| err.to_string())
    }
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
        assert_eq!(
            hosts.route("N1YOU", &[], &[0, 1, 4]),
            Route::Kiss(vec![4])
        );
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
        assert_eq!(
            hosts.route("N2BBS", &[], &[0, 2]),
            Route::Other(5)
        );
    }

    #[test]
    fn logical_ports_map_onto_wire_ports_in_order() {
        assert_eq!(wire_port(&[2, 5], 5), Some(1));
        assert_eq!(logical_port(&[2, 5], 1), Some(5));
        assert_eq!(wire_port(&[2, 5], 0), None);
    }
}
