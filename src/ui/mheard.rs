//! MHeard — stations heard (APRS / AX.25 UI), persisted as JSON.
//!
//! `//Name` stores the opposite station's name on the same disk list.
//! That name is not drawn in the MHeard pane; `%N` reads it from here.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::session::Ax25UiFrame;

const MAX_ENTRIES: usize = 500;

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
    /// Name from `//Name`. Kept on disk for `%N`; omitted from the pane.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Row exists only to remember a name for a call that has not been heard.
    /// Saved with the list, hidden in the software MHeard view.
    #[serde(default, skip_serializing_if = "is_false")]
    pub name_only: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
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
            name: String::new(),
            name_only: false,
        }
    }

    /// Shown in the MHeard pane and in `//MHeard`. Name-only rows stay on disk.
    pub fn listed(&self) -> bool {
        !self.name_only
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
                prev.name_only = false;
                // `name` stays — a later hear must not wipe `//Name`.
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

    pub fn save(&mut self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let raw = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| format!("mheard json: {e}"))?;
        fs::write(&self.path, raw).map_err(|e| format!("mheard save: {e}"))?;
        self.dirty = false;
        Ok(())
    }

    /// Remember `name` for `call`. An already-heard row keeps its place and
    /// stays visible; a call that has never been heard is stored hidden.
    /// Returns false when `call` or `name` is empty after cleanup.
    pub fn set_name(&mut self, call: &str, name: &str) -> bool {
        let call = call.trim();
        let name = normalize_name(name);
        if call.is_empty() || name.is_empty() {
            return false;
        }
        if let Some(prev) = self
            .entries
            .iter_mut()
            .find(|e| e.call.eq_ignore_ascii_case(call))
        {
            if prev.name == name {
                return true;
            }
            prev.name = name;
        } else {
            self.entries.push(HeardStation {
                date: String::new(),
                time: String::new(),
                call: call.to_string(),
                to: String::new(),
                via: String::new(),
                heard_ms: 0,
                name,
                name_only: true,
            });
        }
        self.dirty = true;
        let _ = self.save();
        true
    }

    /// Stored `//Name` for `call`, if one was set.
    pub fn name_for(&self, call: &str) -> Option<String> {
        let call = call.trim();
        if call.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| e.call.eq_ignore_ascii_case(call))
            .map(|e| e.name.trim())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    }

    /// Rows drawn in the MHeard pane (excludes disk-only names).
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
}

/// One line, no control characters. Long enough for a personal name.
fn normalize_name(raw: &str) -> String {
    const MAX: usize = 40;
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX {
        collapsed
    } else {
        collapsed.chars().take(MAX).collect::<String>().trim_end().to_string()
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
        let path = std::env::temp_dir().join(format!(
            "gpng-mheard-{tag}-{}.json",
            std::process::id()
        ));
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
    fn name_only_stays_off_the_list_until_heard() {
        let path = scratch("name-only");
        let mut list = MheardList::load_from(path.clone());
        assert!(list.set_name("NL0MSK", "Jara"));
        assert_eq!(list.name_for("nl0msk").as_deref(), Some("Jara"));
        assert!(list.lines().is_empty());
        assert_eq!(list.listed_len(), 0);

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("Jara"));
        assert!(raw.contains("NL0MSK"));
        assert!(raw.contains("name_only"));

        list.hear(&ui("NL0MSK"));
        assert_eq!(list.listed_len(), 1);
        assert_eq!(list.name_for("NL0MSK").as_deref(), Some("Jara"));
        let line = &list.lines()[0];
        assert!(line.contains("NL0MSK"));
        assert!(!line.contains("Jara"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn hear_keeps_name_and_display_omits_it() {
        let path = scratch("keep-name");
        let mut list = MheardList::load_from(path.clone());
        list.hear(&ui("NL0MSK"));
        assert!(list.set_name("nl0msk", "  Jara  "));
        let line = list.lines()[0].clone();
        assert!(line.contains("NL0MSK"));
        assert!(!line.contains("Jara"));
        list.hear(&ui("NL0MSK"));
        assert_eq!(list.name_for("NL0MSK").as_deref(), Some("Jara"));
        assert_eq!(list.listed_len(), 1);
        let _ = fs::remove_file(&path);
    }
}
