//! MHeard — stations heard (APRS / AX.25 UI), persisted as JSON.
//!
//! Station names live in `NAMES.GP`, not on this list. `%N` and `//Name`
//! read and write that file.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::session::Ax25UiFrame;

const MAX_ENTRIES: usize = 500;
/// Auto Clean stops at one year.
pub const CLEAN_DAYS_MAX: u32 = 365;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Classic DOS GP: bare `//MHeard` sends this many newest rows.
const DEFAULT_REMOTE_LINES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeardStation {
    /// `yyyy/mm/dd`
    pub date: String,
    /// `hh:mm` (local)
    pub time: String,
    pub call: String,
    pub to: String,
    /// Digipeater path, comma-separated (no leading comma).
    #[serde(default)]
    pub via: String,
    /// Sort key — last heard UTC millis.
    pub heard_ms: i64,
}

impl HeardStation {
    pub fn from_ax25(frame: &Ax25UiFrame, when: DateTime<Local>) -> Self {
        Self {
            date: when.format("%Y/%m/%d").to_string(),
            time: when.format("%H:%M").to_string(),
            call: frame.src.clone(),
            to: frame.dest.clone(),
            via: frame.via.join(","),
            heard_ms: when.timestamp_millis(),
        }
    }

    /// Shown in the MHeard pane and in `//MHeard`.
    pub fn listed(&self) -> bool {
        !self.call.is_empty() && !self.date.is_empty()
    }

    /// `yyyy/mm/dd hh:mm  CALL_____ > TO______  via PATH`
    pub fn display_line(&self) -> String {
        let from = pad_call(&self.call);
        let to = pad_call(&self.to);
        if self.via.is_empty() {
            format!("{} {}  {} > {}", self.date, self.time, from, to)
        } else {
            format!(
                "{} {}  {} > {}  via {}",
                self.date, self.time, from, to, self.via
            )
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MheardList {
    pub entries: Vec<HeardStation>,
    path: PathBuf,
    dirty: bool,
}

impl MheardList {
    pub fn load_default() -> Self {
        Self::load_from(default_mheard_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let mut list = Self {
            entries: Vec::new(),
            path: path.clone(),
            dirty: false,
        };
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(entries) = serde_json::from_str::<Vec<HeardStation>>(&raw) {
                list.entries = entries;
            }
        }
        // Older lists kept `//Name` rows that had never been heard.
        list.entries.retain(|e| e.listed());
        list.sort_newest_first();
        list
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn sort_newest_first(&mut self) {
        self.entries.sort_by(|a, b| b.heard_ms.cmp(&a.heard_ms));
    }

    /// Insert or refresh a call. On re-hear, replace date, time, to, and via
    /// (including clearing via when there is no digi path), then move to top.
    pub fn hear(&mut self, frame: &Ax25UiFrame) {
        let now = Local::now();
        let entry = HeardStation::from_ax25(frame, now);
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.call.eq_ignore_ascii_case(&entry.call))
        {
            let unchanged = {
                let prev = &self.entries[pos];
                prev.date == entry.date
                    && prev.time == entry.time
                    && prev.to.eq_ignore_ascii_case(&entry.to)
                    && prev.via == entry.via
            };
            if unchanged {
                return;
            }
            // Full replace of stamp + destination + path (via may become empty).
            {
                let prev = &mut self.entries[pos];
                prev.date = entry.date;
                prev.time = entry.time;
                prev.to = entry.to;
                prev.via = entry.via; // "" when heard direct / no digis
                prev.heard_ms = entry.heard_ms;
            }
            let updated = self.entries.remove(pos);
            self.entries.insert(0, updated);
        } else {
            self.entries.insert(0, entry);
            if self.entries.len() > MAX_ENTRIES {
                self.entries.truncate(MAX_ENTRIES);
            }
        }
        self.dirty = true;
        let _ = self.save();
    }

    /// Drop every station and rewrite the file.
    pub fn clear(&mut self) -> Result<(), String> {
        self.entries.clear();
        self.dirty = true;
        self.save()
    }

    /// Remove stations last heard more than `days` ago.
    /// `0` does nothing. Returns how many rows were removed.
    pub fn drop_older_than(&mut self, days: u32, now_ms: i64) -> usize {
        if days == 0 || self.entries.is_empty() {
            return 0;
        }
        let days = days.min(CLEAN_DAYS_MAX) as i64;
        let cutoff = now_ms.saturating_sub(days.saturating_mul(DAY_MS));
        let before = self.entries.len();
        self.entries.retain(|e| e.heard_ms >= cutoff);
        let removed = before - self.entries.len();
        if removed > 0 {
            self.dirty = true;
            let _ = self.save();
        }
        removed
    }

    pub fn save(&mut self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let raw =
            serde_json::to_string_pretty(&self.entries).map_err(|e| format!("mheard json: {e}"))?;
        fs::write(&self.path, raw).map_err(|e| format!("mheard save: {e}"))?;
        self.dirty = false;
        Ok(())
    }

    /// Rows drawn in the MHeard pane.
    pub fn listed_len(&self) -> usize {
        self.entries.iter().filter(|e| e.listed()).count()
    }

    pub fn lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.listed())
            .map(HeardStation::display_line)
            .collect()
    }

    /// Lines for `//MHeard` / `//mh`.
    ///
    /// - no args → newest [`DEFAULT_REMOTE_LINES`] (classic default 10)
    /// - a number → that many newest rows (`//mh 50`, `//mh 5`)
    /// - a callsign → that station's row when it is on the list
    pub fn remote_lines(&self, args: &str) -> Vec<String> {
        let args = args.trim();
        if args.is_empty() {
            return self.newest_lines(DEFAULT_REMOTE_LINES);
        }
        if let Ok(n) = args.parse::<usize>() {
            return self.newest_lines(n);
        }
        self.entries
            .iter()
            .filter(|e| e.listed() && e.call.eq_ignore_ascii_case(args))
            .map(HeardStation::display_line)
            .collect()
    }

    fn newest_lines(&self, n: usize) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.listed())
            .take(n)
            .map(HeardStation::display_line)
            .collect()
    }
}

/// Fixed-width call field so MHeard lines align as a table.
fn pad_call(s: &str) -> String {
    format!("{:<9}", s)
}

pub fn default_mheard_path() -> PathBuf {
    crate::paths::data_file("MHEARD.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("gpng-mheard-{tag}-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    fn ui(src: &str) -> Ax25UiFrame {
        Ax25UiFrame {
            dest: "APRS".into(),
            src: src.into(),
            via: Vec::new(),
            info: String::new(),
        }
    }

    #[test]
    fn hear_lists_the_call() {
        let path = scratch("hear");
        let mut list = MheardList::load_from(path.clone());
        list.hear(&ui("NL0MSK"));
        assert_eq!(list.listed_len(), 1);
        let line = &list.lines()[0];
        assert!(line.contains("NL0MSK"));
        list.hear(&ui("NL0MSK"));
        assert_eq!(list.listed_len(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn remote_default_is_ten_newest() {
        let path = scratch("remote-default");
        let mut list = MheardList::load_from(path.clone());
        for i in 0..20 {
            list.hear(&ui(&format!("N{i:02}CALL")));
        }
        assert_eq!(list.remote_lines("").len(), 10);
        assert_eq!(list.remote_lines("5").len(), 5);
        assert_eq!(list.remote_lines("50").len(), 20);
        let one = list.remote_lines("N19CALL");
        assert_eq!(one.len(), 1);
        assert!(one[0].contains("N19CALL"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn clear_empties_the_file() {
        let path = scratch("clear");
        let mut list = MheardList::load_from(path.clone());
        list.hear(&ui("NL0MSK"));
        list.clear().unwrap();
        assert_eq!(list.listed_len(), 0);
        let again = MheardList::load_from(path.clone());
        assert_eq!(again.listed_len(), 0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn auto_clean_drops_stations_older_than_the_limit() {
        let path = scratch("clean");
        let mut list = MheardList::load_from(path.clone());
        list.hear(&ui("OLD1"));
        list.hear(&ui("NEW1"));
        let now = list.entries[0].heard_ms;
        let old = list
            .entries
            .iter_mut()
            .find(|e| e.call == "OLD1")
            .unwrap();
        old.heard_ms = now - 10 * DAY_MS;
        assert_eq!(list.drop_older_than(0, now), 0);
        assert_eq!(list.listed_len(), 2);
        assert_eq!(list.drop_older_than(7, now), 1);
        assert_eq!(list.listed_len(), 1);
        assert!(list.lines()[0].contains("NEW1"));
        assert_eq!(list.drop_older_than(7, now), 0);
        let _ = fs::remove_file(&path);
    }
}
