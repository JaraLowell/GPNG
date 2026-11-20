//! MHeard — stations heard (APRS / AX.25 UI), persisted as JSON.
//!
//! The pane is a sheet: Date, Time, Port (when TFPCX has more than one
//! radio), From, To, Packets, Last Payload, and Via. Packet counts stay
//! short (`30`, `1.3k`, `26m`). The payload is kept in full; the sheet
//! clips it for the cell. Older `MHEARD.json` files omit `packets`,
//! `last_msg`, and `port`; those load as one packet, an empty message,
//! and no port.
//!
//! Station names live in `NAMES.GP`, not on this list. `%N` and `//Name`
//! read and write that file.
//!
//! Optional `locator` / `lat` / `lon` come from payload filters (APRS position
//! or a 6-/8-character Maidenhead tag). Longer locators are kept when a later
//! packet only has a shorter tag. Older files omit those fields. When a fix is
//! known, the sheet shows a second line under Last Payload:
//! `GRID  Region  Country  N Km from QTH`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::locator::{self, PositionFix};

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
    /// Packets heard from this station.
    ///
    /// Older files omit the field. Missing means one (the station is on the
    /// list, so it was heard at least once).
    #[serde(default = "one_packet")]
    pub packets: u64,
    /// Last information text, kept in full. The list shows at most 60 characters.
    #[serde(default)]
    pub last_msg: String,
    /// Radio port this station was last heard on. Absent in older files.
    #[serde(default)]
    pub port: Option<u8>,
    /// Last known Maidenhead locator (6 or 8 characters) from the information field.
    #[serde(default)]
    pub locator: Option<String>,
    /// Latitude when an APRS (or locator-centre) fix is known.
    #[serde(default)]
    pub lat: Option<f64>,
    /// Longitude when an APRS (or locator-centre) fix is known.
    #[serde(default)]
    pub lon: Option<f64>,
}

fn one_packet() -> u64 {
    1
}

fn apply_fix(entry: &mut HeardStation, fix: PositionFix) {
    if !locator::should_replace_locator(entry.locator.as_deref(), &fix) {
        return;
    }
    entry.locator = Some(fix.locator);
    entry.lat = Some(fix.lat);
    entry.lon = Some(fix.lon);
}

impl HeardStation {
    pub fn from_ax25(frame: &Ax25UiFrame, when: DateTime<Local>, port: Option<u8>) -> Self {
        Self {
            date: when.format("%Y/%m/%d").to_string(),
            time: when.format("%H:%M").to_string(),
            call: frame.src.clone(),
            to: frame.dest.clone(),
            via: frame.via.join(","),
            heard_ms: when.timestamp_millis(),
            packets: 1,
            last_msg: normalize_last_msg(&frame.info),
            port,
            locator: None,
            lat: None,
            lon: None,
        }
    }

    /// Shown in the MHeard pane and in `//MHeard`.
    pub fn listed(&self) -> bool {
        !self.call.is_empty() && !self.date.is_empty()
    }

    /// `yyyy/mm/dd hh:mm  CALL_____ > TO_______  COUNT message  via PATH`
    ///
    /// `COUNT` is five columns. `message` is at most 60 characters.
    /// `show_port` inserts a two-column radio port after the time.
    pub fn display_line(&self) -> String {
        self.format_line(false)
    }

    pub fn format_line(&self, show_port: bool) -> String {
        let from = pad_call(&self.call);
        let to = pad_call(&self.to);
        let count = format_packet_count(self.packets);
        let msg = clip_last_msg(&self.last_msg);
        let mut line = format!("{} {}", self.date, self.time);
        if show_port {
            match self.port {
                Some(port) => line.push_str(&format!("  {port:>2}")),
                None => line.push_str("    "),
            }
        }
        line.push_str(&format!("  {from} > {to}  {count}"));
        if !msg.is_empty() {
            line.push(' ');
            line.push_str(&msg);
        }
        if !self.via.is_empty() {
            line.push_str("  via ");
            line.push_str(&self.via);
        }
        line
    }

    /// `//MHeard` row. No packet count and no last message.
    /// The radio port is included only when this station was heard on one.
    pub fn remote_line(&self) -> String {
        let from = pad_call(&self.call);
        let to = pad_call(&self.to);
        let mut line = format!("{} {}", self.date, self.time);
        if let Some(port) = self.port {
            line.push_str(&format!("  {port:>2}"));
        }
        line.push_str(&format!("  {from} > {to}"));
        if !self.via.is_empty() {
            line.push_str("  via ");
            line.push_str(&self.via);
        }
        line
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

    /// Insert or refresh a call. Each packet increments the count, replaces
    /// date, time, to, and via (via may become empty), and moves the station
    /// to the top. An empty information field leaves the previous message.
    /// `port` replaces the stored radio port when it is known.
    pub fn hear(&mut self, frame: &Ax25UiFrame, port: Option<u8>) {
        let now = Local::now();
        let mut entry = HeardStation::from_ax25(frame, now, port);
        let raw = if frame.info_raw.is_empty() {
            frame.info.as_bytes()
        } else {
            frame.info_raw.as_slice()
        };
        let fix = locator::extract_position(raw, &frame.src, &frame.dest);
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.call.eq_ignore_ascii_case(&entry.call))
        {
            {
                let prev = &mut self.entries[pos];
                prev.date = entry.date;
                prev.time = entry.time;
                prev.to = entry.to;
                prev.via = entry.via; // "" when heard direct / no digis
                prev.heard_ms = entry.heard_ms;
                if entry.port.is_some() {
                    prev.port = entry.port;
                }
                prev.packets = prev.packets.saturating_add(1);
                if !entry.last_msg.is_empty() {
                    prev.last_msg = entry.last_msg;
                }
                if let Some(fix) = fix {
                    apply_fix(prev, fix);
                }
            }
            let updated = self.entries.remove(pos);
            self.entries.insert(0, updated);
        } else {
            if let Some(fix) = fix {
                apply_fix(&mut entry, fix);
            }
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

    pub fn lines(&self, show_port: bool) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.listed())
            .map(|e| e.format_line(show_port))
            .collect()
    }

    /// Lines for `//MHeard` / `//mh`.
    ///
    /// - no args → newest [`DEFAULT_REMOTE_LINES`] (classic default 10)
    /// - a number → that many newest rows (`//mh 50`, `//mh 5`)
    /// - a callsign → that station's row when it is on the list
    ///
    /// Each line is date, time, the radio port when known, call, to, and via.
    pub fn remote_lines(&self, args: &str) -> Vec<String> {
        let args = args.trim();
        let listed = self.entries.iter().filter(|e| e.listed());
        if args.is_empty() {
            return listed.take(DEFAULT_REMOTE_LINES).map(|e| e.remote_line()).collect();
        }
        if let Ok(n) = args.parse::<usize>() {
            return listed.take(n).map(|e| e.remote_line()).collect();
        }
        listed
            .filter(|e| e.call.eq_ignore_ascii_case(args))
            .map(|e| e.remote_line())
            .collect()
    }
}

/// Fixed 9-character call field so the count and message columns line up.
fn pad_call(s: &str) -> String {
    let mut call: String = s.chars().take(9).collect();
    while call.chars().count() < 9 {
        call.push(' ');
    }
    call
}

/// Packet count in at most five characters: `30`, `1.3k`, `26m`.
/// The returned field is right-aligned to five columns.
pub fn format_packet_count(n: u64) -> String {
    let body = compact_count(n);
    format!("{body:>5}")
}

fn compact_count(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let (div, suf) = if n < 1_000_000 {
        (1_000, 'k')
    } else if n < 1_000_000_000 {
        (1_000_000, 'm')
    } else if n < 1_000_000_000_000 {
        (1_000_000_000, 'b')
    } else {
        return "999b+".to_string();
    };
    // Rounded tenths of the chosen unit. 1_300 → 13 → `1.3k`. 26_000_000 → `26m`.
    let tenths = n.saturating_mul(10).saturating_add(div / 2) / div;
    if tenths >= 10_000 {
        return match suf {
            'k' => compact_count(1_000_000),
            'm' => compact_count(1_000_000_000),
            _ => "999b+".to_string(),
        };
    }
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 || whole >= 100 {
        format!("{whole}{suf}")
    } else {
        format!("{whole}.{frac}{suf}")
    }
}

/// One display line of the information field. Controls become spaces.
fn normalize_last_msg(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_control() || c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    out
}

/// At most 60 characters. Longer text keeps 57 and ends with `...`.
pub fn clip_last_msg(msg: &str) -> String {
    let n = msg.chars().count();
    if n <= 60 {
        msg.to_string()
    } else {
        let head: String = msg.chars().take(57).collect();
        format!("{head}...")
    }
}

pub fn default_mheard_path() -> PathBuf {
    crate::paths::data_file("MHEARD.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(packets: u64, last_msg: &str, via: &str) -> HeardStation {
        HeardStation {
            date: "2026/11/18".into(),
            time: "11:12".into(),
            call: "DL1ABC".into(),
            to: "APRS".into(),
            via: via.into(),
            heard_ms: 1,
            packets,
            last_msg: last_msg.into(),
            port: None,
            locator: None,
            lat: None,
            lon: None,
        }
    }

    fn test_ui(dest: &str, src: &str, info: &str) -> Ax25UiFrame {
        Ax25UiFrame {
            dest: dest.into(),
            src: src.into(),
            via: vec![],
            info: info.into(),
            info_raw: info.as_bytes().to_vec(),
        }
    }

    #[test]
    fn packet_count_fits_in_five_columns() {
        let samples = [
            (0, "    0"),
            (30, "   30"),
            (999, "  999"),
            (1_000, "   1k"),
            (1_300, " 1.3k"),
            (10_400, "10.4k"),
            (26_000, "  26k"),
            (26_000_000, "  26m"),
            (26_400_000, "26.4m"),
            (999_000_000_000, " 999b"),
            (1_000_000_000_000, "999b+"),
        ];
        for (n, want) in samples {
            let got = format_packet_count(n);
            assert_eq!(got, want, "count {n}");
            assert_eq!(got.chars().count(), 5, "width {n}");
        }
    }

    #[test]
    fn display_line_places_count_and_clips_message() {
        assert_eq!(
            station(30, "hello", "").display_line(),
            "2026/11/18 11:12  DL1ABC    > APRS          30 hello"
        );
        assert_eq!(
            station(1_300, "hello", "").display_line(),
            "2026/11/18 11:12  DL1ABC    > APRS        1.3k hello"
        );
        assert_eq!(
            station(26_000_000, "hello", "DIGI").display_line(),
            "2026/11/18 11:12  DL1ABC    > APRS         26m hello  via DIGI"
        );
        let long = "A".repeat(61);
        let line = station(30, &long, "").display_line();
        let msg = line.split(" 30 ").nth(1).unwrap();
        assert_eq!(msg.chars().count(), 60);
        assert!(msg.ends_with("..."));
        assert_eq!(msg.chars().take(57).collect::<String>(), "A".repeat(57));
        let exact = station(30, &"B".repeat(60), "").display_line();
        assert!(exact.ends_with(&"B".repeat(60)));
    }

    #[test]
    fn display_line_inserts_the_radio_port_after_the_time() {
        let mut row = station(30, "hello", "");
        row.port = Some(0);
        assert_eq!(
            row.format_line(true),
            "2026/11/18 11:12   0  DL1ABC    > APRS          30 hello"
        );
        row.port = Some(11);
        assert_eq!(
            row.format_line(true),
            "2026/11/18 11:12  11  DL1ABC    > APRS          30 hello"
        );
        row.port = None;
        assert_eq!(
            row.format_line(true),
            "2026/11/18 11:12      DL1ABC    > APRS          30 hello"
        );
        assert_eq!(row.display_line(), row.format_line(false));
    }

    #[test]
    fn remote_line_keeps_the_port_and_drops_count_and_message() {
        let mut row = station(30, "hello packet", "DIGI");
        assert_eq!(
            row.remote_line(),
            "2026/11/18 11:12  DL1ABC    > APRS       via DIGI"
        );
        row.port = Some(2);
        assert_eq!(
            row.remote_line(),
            "2026/11/18 11:12   2  DL1ABC    > APRS       via DIGI"
        );
        assert!(!row.remote_line().contains("30"));
        assert!(!row.remote_line().contains("hello"));
    }

    #[test]
    fn hear_stores_locator_from_payload_and_rejects_callsign_tag() {
        let path = std::env::temp_dir().join(format!(
            "gpng-mheard-loc-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_file(&path);
        let mut list = MheardList::load_from(path.clone());
        list.hear(&test_ui("APRS", "PI12NN", "hello PI12NN de JO32KX"), None);
        assert_eq!(list.entries[0].locator.as_deref(), Some("JO32KX"));
        list.hear(&test_ui("APRS", "PI12NN", "qth JO32mw49"), None);
        assert_eq!(list.entries[0].locator.as_deref(), Some("JO32MW49"));
        list.hear(&test_ui("APRS", "PI12NN", "still JO32KX only"), None);
        // Keep the more specific 8-char when a later packet only has 6.
        assert_eq!(list.entries[0].locator.as_deref(), Some("JO32MW49"));
        list.hear(&test_ui("APRS", "PI12NN", "!5231.00N/01324.00E-move"), None);
        assert!(list.entries[0].locator.as_ref().unwrap().starts_with("JO62"));
        assert_eq!(list.entries[0].locator.as_ref().unwrap().len(), 8);
        assert!(list.entries[0].lat.is_some());
        // Mic-E needs dest + raw info bytes.
        list.hear(
            &Ax25UiFrame {
                dest: "S32UVT".into(),
                src: "N7YV-9".into(),
                via: vec![],
                info: "`(_fn\"Oj/".into(),
                info_raw: b"`(_fn\"Oj/".to_vec(),
            },
            None,
        );
        assert_eq!(list.entries[0].call, "N7YV-9");
        assert!((list.entries[0].lat.unwrap() - 33.427).abs() < 0.01);
        assert!((list.entries[0].lon.unwrap() - (-112.129)).abs() < 0.01);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn old_mheard_file_loads_and_new_fields_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "gpng-mheard-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let old = r#"[{
            "date": "2020/01/02",
            "time": "03:04",
            "call": "DL1ABC",
            "to": "APRS",
            "via": "DIGI",
            "heard_ms": 5
        }]"#;
        fs::write(&path, old).unwrap();
        let loaded = MheardList::load_from(path.clone());
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].packets, 1);
        assert_eq!(loaded.entries[0].last_msg, "");
        assert_eq!(loaded.entries[0].via, "DIGI");
        assert_eq!(loaded.entries[0].port, None);
        let _ = fs::remove_file(&path);

        let path = std::env::temp_dir().join(format!(
            "gpng-mheard-hear-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut list = MheardList::load_from(path.clone());
        list.hear(
            &Ax25UiFrame {
                dest: "APRS".into(),
                src: "DL1ABC".into(),
                via: vec!["DIGI".into()],
                info: "hello\r\nthere".into(),
                info_raw: b"hello\r\nthere".to_vec(),
            },
            Some(0),
        );
        list.hear(
            &Ax25UiFrame {
                dest: "BEACON".into(),
                src: "dl1abc".into(),
                via: vec![],
                info: String::new(),
                info_raw: vec![],
            },
            Some(2),
        );
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].packets, 2);
        assert_eq!(list.entries[0].last_msg, "hello there");
        assert_eq!(list.entries[0].to, "BEACON");
        assert_eq!(list.entries[0].port, Some(2));
        assert!(list.entries[0].via.is_empty());
        let again = MheardList::load_from(path.clone());
        assert_eq!(again.entries[0].packets, 2);
        assert_eq!(again.entries[0].last_msg, "hello there");
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"packets\""));
        assert!(raw.contains("\"last_msg\""));
        let _ = fs::remove_file(&path);
    }
}
