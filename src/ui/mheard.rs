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

    /// Table for `//MHeard` / `//mh`.
    ///
    /// - no args → newest [`DEFAULT_REMOTE_LINES`] (classic default 10)
    /// - a number → that many newest rows (`//mh 50`, `//mh 5`)
    /// - a callsign → that station's row when it is on the list
    ///
    /// `port_label` turns the stored radio port into `0 Serial-link` or
    /// `1 TCP-link`. Columns are Callsign, Date, Time (`yyyy/mm/dd hh:mm`),
    /// Port, and packet count (`12`, `1.2K`, `1.0M`, `1.0B`, `1.0T`).
    pub fn remote_table(
        &self,
        args: &str,
        mut port_label: impl FnMut(Option<u8>) -> String,
    ) -> String {
        let rows = self.remote_rows(args);
        let labels: Vec<String> = rows.iter().map(|row| port_label(row.port)).collect();
        let port_w = labels
            .iter()
            .map(|label| label.chars().count())
            .max()
            .unwrap_or(0)
            .max(REMOTE_PORT_W);
        let mut lines = Vec::with_capacity(rows.len() + 2);
        lines.push(format!(
            " {}{}{}{}{}",
            fit_left("Callsign", REMOTE_CALL_W),
            fit_left("Date", REMOTE_DATE_W),
            fit_left("Time", REMOTE_TIME_W),
            fit_left("Port", port_w),
            fit_right("Pck", REMOTE_COUNT_W),
        ));
        let rule = REMOTE_CALL_W + REMOTE_DATE_W + REMOTE_TIME_W + port_w + REMOTE_COUNT_W;
        lines.push(format!(" {}", "-".repeat(rule)));
        for (row, label) in rows.iter().zip(labels) {
            let count = remote_packet_count(row.packets);
            lines.push(format!(
                " {}{}{}{}{}",
                fit_left(&row.call, REMOTE_CALL_W),
                fit_left(&row.date, REMOTE_DATE_W),
                fit_left(&row.time, REMOTE_TIME_W),
                fit_left(&label, port_w),
                fit_right(&count, REMOTE_COUNT_W),
            ));
        }
        lines.join("\r")
    }

    fn remote_rows(&self, args: &str) -> Vec<&HeardStation> {
        let args = args.trim();
        let listed = self.entries.iter().filter(|e| e.listed());
        if args.is_empty() {
            return listed.take(DEFAULT_REMOTE_LINES).collect();
        }
        if let Ok(n) = args.parse::<usize>() {
            return listed.take(n).collect();
        }
        listed
            .filter(|e| e.call.eq_ignore_ascii_case(args))
            .collect()
    }
}

/// `//MHeard` columns. Date is `yyyy/mm/dd` plus one space; time is `hh:mm` plus two.
const REMOTE_CALL_W: usize = 10;
const REMOTE_DATE_W: usize = 11;
const REMOTE_TIME_W: usize = 7;
const REMOTE_PORT_W: usize = 13;
/// `999.9T` is the longest count, plus one space so it never touches the port.
const REMOTE_COUNT_W: usize = 7;

fn fit_left(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    let n = out.chars().count();
    if n < width {
        out.extend(std::iter::repeat(' ').take(width - n));
    }
    out
}

fn fit_right(s: &str, width: usize) -> String {
    let text: String = s.chars().take(width).collect();
    let n = text.chars().count();
    let mut out = " ".repeat(width.saturating_sub(n));
    out.push_str(&text);
    out
}

/// Fixed 9-character call field so the count and message columns line up.
fn pad_call(s: &str) -> String {
    let mut call: String = s.chars().take(9).collect();
    while call.chars().count() < 9 {
        call.push(' ');
    }
    call
}

/// `//MHeard` count. Below 1000 the number is plain. From there it is one
/// decimal and a unit: `1.0K`, `1.2M`, `1.0B`, `1.0T`. The longest form is
/// `999.9T`; anything larger stays on that ceiling.
fn remote_packet_count(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let units: [(u64, char); 4] = [
        (1_000, 'K'),
        (1_000_000, 'M'),
        (1_000_000_000, 'B'),
        (1_000_000_000_000, 'T'),
    ];
    for (i, &(div, suf)) in units.iter().enumerate() {
        if n < div {
            break;
        }
        let tenths = div_round_tenths(n, div);
        if tenths >= 10_000 {
            match units.get(i + 1) {
                Some(&(next_div, next_suf)) if n < next_div => {
                    let next_tenths = div_round_tenths(n, next_div);
                    return format!("{}.{}{next_suf}", next_tenths / 10, next_tenths % 10);
                }
                Some(_) => continue,
                None => return "999.9T".into(),
            }
        }
        if units.get(i + 1).is_some_and(|&(next_div, _)| n >= next_div) {
            continue;
        }
        return format!("{}.{}{suf}", tenths / 10, tenths % 10);
    }
    "999.9T".into()
}

/// Rounded tenths of `div`: 1_200 and 1_000 → 12 (`1.2`).
fn div_round_tenths(n: u64, div: u64) -> u64 {
    n.saturating_mul(10).saturating_add(div / 2) / div
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

    fn heard(call: &str, time: &str, port: Option<u8>, packets: u64) -> HeardStation {
        let mut row = station(packets, "hello packet", "DIGI");
        row.call = call.into();
        row.date = "2026/11/21".into();
        row.time = time.into();
        row.port = port;
        row
    }

    fn sample_list() -> MheardList {
        MheardList {
            entries: vec![
                heard("NL0MSK", "17:35", Some(0), 12),
                heard("NL1WNB", "17:35", Some(0), 184),
                heard("NL0PEL", "17:28", Some(0), 1_200),
                heard("NL0AS", "17:17", Some(0), 88),
                heard("NL1AS", "17:17", Some(1), 1),
            ],
            path: PathBuf::new(),
            dirty: false,
        }
    }

    fn port_label(port: Option<u8>) -> String {
        match port {
            Some(1) => "1 TCP-link".into(),
            Some(port) => format!("{port} Serial-link"),
            None => "-".into(),
        }
    }

    #[test]
    fn remote_table_shows_date_time_port_and_count() {
        let table = sample_list().remote_table("", port_label);
        let header = " Callsign  Date       Time   Port             Pck";
        let rule = format!(" {}", "-".repeat(header.len() - 1));
        let expected = [
            header,
            &rule,
            " NL0MSK    2026/11/21 17:35  0 Serial-link     12",
            " NL1WNB    2026/11/21 17:35  0 Serial-link    184",
            " NL0PEL    2026/11/21 17:28  0 Serial-link   1.2K",
            " NL0AS     2026/11/21 17:17  0 Serial-link     88",
            " NL1AS     2026/11/21 17:17  1 TCP-link         1",
        ]
        .join("\r");
        assert_eq!(table, expected);
        assert!(!table.contains("hello"));
        assert!(!table.contains("DIGI"));
        assert!(!table.contains("APRS"));
    }

    #[test]
    fn remote_table_limits_rows_and_finds_one_call() {
        let list = sample_list();
        let two = list.remote_table("2", port_label);
        assert!(two.contains("NL0MSK"));
        assert!(two.contains("NL1WNB"));
        assert!(!two.contains("NL0PEL"));
        let one = list.remote_table("nl0as", port_label);
        assert!(one.contains("NL0AS"));
        assert!(!one.contains("NL0MSK"));
        assert!(one.contains("     88"));
    }

    #[test]
    fn remote_packet_count_scales_and_keeps_a_leading_space() {
        let samples = [
            (0, "0"),
            (12, "12"),
            (999, "999"),
            (1_000, "1.0K"),
            (1_200, "1.2K"),
            (999_949, "999.9K"),
            (999_950, "1.0M"),
            (1_200_000, "1.2M"),
            (999_949_999, "999.9M"),
            (1_000_000_000, "1.0B"),
            (1_200_000_000, "1.2B"),
            (1_000_000_000_000, "1.0T"),
            (1_200_000_000_000, "1.2T"),
            (u64::MAX, "999.9T"),
        ];
        for (n, want) in samples {
            let got = remote_packet_count(n);
            assert_eq!(got, want, "count {n}");
            assert!(got.chars().count() <= 6, "{got} is wider than 999.9T");
            let field = fit_right(&got, REMOTE_COUNT_W);
            assert_eq!(field.chars().count(), REMOTE_COUNT_W);
            assert!(field.starts_with(' '), "{field} touches the port column");
        }
        assert_eq!(fit_right("999.9K", REMOTE_COUNT_W), " 999.9K");
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
        assert!(list.entries[0]
            .locator
            .as_ref()
            .unwrap()
            .starts_with("JO62"));
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
