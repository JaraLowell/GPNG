//! MHeard — stations heard (APRS / AX.25 UI), persisted as JSON.

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

    /// `yyyy/mm/dd hh:mm  CALL > TO  VIA…`
    pub fn display_line(&self) -> String {
        if self.via.is_empty() {
            format!("{} {}  {} > {}", self.date, self.time, self.call, self.to)
        } else {
            format!(
                "{} {}  {} > {}  {}",
                self.date, self.time, self.call, self.to, self.via
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
        let path = default_mheard_path();
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

    /// Insert or refresh a call; keeps newest-first order. Saves JSON when changed.
    pub fn hear(&mut self, frame: &Ax25UiFrame) {
        let now = Local::now();
        // Avoid second-level spam: same call within same minute → bump time only.
        let entry = HeardStation::from_ax25(frame, now);
        let call_up = entry.call.to_ascii_uppercase();
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.call.eq_ignore_ascii_case(&call_up))
        {
            let prev = &self.entries[pos];
            if prev.date == entry.date
                && prev.time == entry.time
                && prev.to == entry.to
                && prev.via == entry.via
            {
                return; // identical minute stamp
            }
            self.entries.remove(pos);
        }
        self.entries.insert(0, entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.truncate(MAX_ENTRIES);
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

    pub fn lines(&self) -> Vec<String> {
        self.entries.iter().map(HeardStation::display_line).collect()
    }
}

pub fn default_mheard_path() -> PathBuf {
    let candidates = [
        PathBuf::from("tools").join("MHEARD.json"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tools")
            .join("MHEARD.json"),
    ];
    for p in &candidates {
        if let Some(parent) = p.parent() {
            if parent.is_dir() {
                return p.clone();
            }
        }
    }
    candidates[0].clone()
}
