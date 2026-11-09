//! Persist RX pads across restarts.
//!
//! Files under `[paths].log_dir`/`PADS/`:
//! `monitor.txt`, `ch01.txt`…, `spy01.txt`…, `bbs.txt`, `box.txt`.
//!
//! Channel, BBS, and Box-List rows: green is what we sent, white is what
//! we received. The monitor uses one colour for every `fm … to …` header
//! and light grey for the payload, send and receive alike. Orange System
//! lines are never written.
//!
//! Record format: `W|text` (own) or `G|text` (incoming).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId};

/// Monitor and channel text. Column wrap uses this size.
pub const PAD_FONT_SIZE: f32 = 16.0;
use tracing::warn;

const PAD_GREEN: Color32 = Color32::from_rgb(85, 255, 85);
/// Received text.
const PAD_INCOMING: Color32 = Color32::WHITE;
/// Sent text.
const PAD_OWN: Color32 = PAD_GREEN;
const PAD_SYSTEM: Color32 = Color32::from_rgb(255, 168, 0); // orange
/// Monitor `fm … to … ctl …` line. Same green for sent and received frames.
const MONITOR_FM: Color32 = PAD_GREEN;
/// Monitor payload under that header. Same grey for sent and received frames.
const MONITOR_PAYLOAD: Color32 = Color32::from_rgb(170, 170, 170);
/// `<0x1c>` tokens. Darker than the payload so binary stands out of the text.
const MONITOR_HEX: Color32 = Color32::from_rgb(112, 112, 120);

/// Classic GP line roles — colour carries meaning (no `***` needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    /// Received text (white) — persisted.
    Incoming,
    /// Local TX echo (green) — persisted.
    Own,
    /// Init, status, warnings (orange) — not logged.
    System,
}

impl PadKind {
    pub fn color(self) -> Color32 {
        match self {
            PadKind::Incoming => PAD_INCOMING,
            PadKind::Own => PAD_OWN,
            PadKind::System => PAD_SYSTEM,
        }
    }
}

/// One colour run inside a row. Empty `runs` means the whole `text` uses
/// the row kind colour (logs, system lines, plain text).
pub struct PadRun {
    pub text: String,
    pub color: Color32,
}

pub struct PadLine {
    pub kind: PadKind,
    pub text: String,
    runs: Vec<PadRun>,
    /// Identity of this row, so a later packet can append to it.
    seq: u64,
    /// Author's line breaks only. The monitor column wrap leaves this row alone.
    nowrap: bool,
}

pub struct TextPad {
    pub lines: VecDeque<PadLine>,
    /// Cap from `[buffers]` (monitor / channel / spy / bbs / box).
    pub max_lines: usize,
    next_seq: u64,
}

impl TextPad {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines: max_lines.max(1),
            next_seq: 1,
        }
    }

    pub fn push(&mut self, kind: PadKind, line: impl Into<String>) {
        self.push_seq(kind, line);
    }

    /// A banner the monitor must not fold to the pane width.
    /// A `\n` in the text is still a row break.
    pub fn push_unwrapped(&mut self, kind: PadKind, line: impl Into<String>) {
        self.push_line(kind, line.into(), true);
    }

    /// Monitor row. `fm …` is green; the payload under it is light grey.
    /// Non-printables are already `<0xNN>` or get rewritten to that here, in
    /// a darker grey, so they cannot open a new line. System stays orange
    /// and is not written to the log.
    pub fn push_monitor(&mut self, kind: PadKind, line: impl Into<String>) {
        let line = line.into();
        if kind == PadKind::System {
            self.push(kind, line);
            return;
        }
        let line = crate::tnc::ax25::escape_monitor_text(&line);
        let base = monitor_color(&line);
        let runs = monitor_paint_runs(&line, base);
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.lines.push_back(PadLine {
            kind,
            text: line,
            runs,
            seq,
            nowrap: false,
        });
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
    }

    /// Drop every row. The caller rewrites the pad's log so a restart stays empty.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Push a row and return its id.
    pub fn push_seq(&mut self, kind: PadKind, line: impl Into<String>) -> u64 {
        self.push_line(kind, line.into(), false)
    }

    fn push_line(&mut self, kind: PadKind, text: String, nowrap: bool) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.lines.push_back(PadLine {
            kind,
            text,
            runs: Vec::new(),
            seq,
            nowrap,
        });
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
        seq
    }

    /// Append `more`. `color` None keeps the row kind colour.
    pub fn append_styled(&mut self, seq: u64, more: &str, color: Option<Color32>) -> bool {
        let Some(line) = self.lines.back_mut().filter(|line| line.seq == seq) else {
            return false;
        };
        if more.is_empty() {
            return true;
        }
        line.text.push_str(more);
        let color = color.unwrap_or_else(|| line.kind.color());
        if line.runs.is_empty() {
            let prefix_len = line.text.len() - more.len();
            if prefix_len > 0 {
                let prefix = line.text[..prefix_len].to_string();
                line.runs.push(PadRun {
                    text: prefix,
                    color: line.kind.color(),
                });
            }
        }
        if let Some(last) = line.runs.last_mut() {
            if last.color == color {
                last.text.push_str(more);
                return true;
            }
        }
        line.runs.push(PadRun {
            text: more.to_string(),
            color,
        });
        true
    }

    /// First bytes of a row, already in `color`.
    pub fn push_styled(&mut self, kind: PadKind, line: impl Into<String>, color: Color32) -> u64 {
        let text = line.into();
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let runs = if text.is_empty() {
            Vec::new()
        } else {
            vec![PadRun {
                text: text.clone(),
                color,
            }]
        };
        self.lines.push_back(PadLine {
            kind,
            text,
            runs,
            seq,
            nowrap: false,
        });
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
        seq
    }

    /// Replace a row still on the pad. Clears colour runs so the new text paints in the row kind.
    pub fn set_text_if_seq(&mut self, seq: u64, text: &str) -> bool {
        let Some(line) = self.lines.iter_mut().find(|line| line.seq == seq) else {
            return false;
        };
        line.text = text.to_string();
        line.runs.clear();
        true
    }

    pub fn text_if_seq(&self, seq: u64) -> Option<String> {
        self.lines
            .iter()
            .find(|line| line.seq == seq)
            .map(|line| line.text.clone())
    }

    pub fn layout_job(&self) -> LayoutJob {
        self.layout_job_wrapped(usize::MAX)
    }

    /// Same pad, broken so each visual row fits in `columns` monospace cells.
    ///
    /// Payload rows (anything that is not an `fm` header or a system line)
    /// repeat the three-space indent on every continuation. A wrap prefers a
    /// space. A `<0xNN>` / `<U+NNNN>` token moves as a whole.
    pub fn layout_job_wrapped(&self, columns: usize) -> LayoutJob {
        self.layout_job_wrapped_indent(columns, true, false, false)
    }

    /// Channel / spy / mail pads: wrap at `columns` with no continuation indent.
    pub fn layout_job_wrapped_plain(&self, columns: usize) -> LayoutJob {
        self.layout_job_wrapped_indent(columns, false, false, false)
    }

    /// Channel, BBS-Menu, and Box-List. `umlaut` maps the seven Latin-1
    /// glyphs back to äöüÄÖÜß. The stored row stays CP437, so turning the
    /// button off shows the shift again.
    pub fn layout_job_channel(&self, columns: usize, umlaut: bool) -> LayoutJob {
        self.layout_job_channel_show(columns, umlaut, true)
    }

    /// Same as [`layout_job_channel`]. `show_own` false leaves our rows in the
    /// pad and omits them from the picture.
    pub fn layout_job_channel_show(
        &self,
        columns: usize,
        umlaut: bool,
        show_own: bool,
    ) -> LayoutJob {
        self.layout_job_wrapped_indent(columns, false, umlaut, !show_own)
    }

    fn layout_job_wrapped_indent(
        &self,
        columns: usize,
        monitor_indent: bool,
        umlaut: bool,
        hide_own: bool,
    ) -> LayoutJob {
        let mut job = LayoutJob::default();
        let font = FontId::monospace(PAD_FONT_SIZE);
        let mut first = true;
        for line in &self.lines {
            if hide_own && line.kind == PadKind::Own {
                continue;
            }
            let indent = if monitor_indent {
                monitor_wrap_indent(line)
            } else {
                0
            };
            let shifted;
            let shifted_runs;
            let (text, runs) = if umlaut {
                shifted = crate::ui::editor::show_channel_text(&line.text, true);
                shifted_runs = line
                    .runs
                    .iter()
                    .map(|run| PadRun {
                        text: crate::ui::editor::show_channel_text(&run.text, true),
                        color: run.color,
                    })
                    .collect::<Vec<_>>();
                (shifted.as_str(), shifted_runs.as_slice())
            } else {
                (line.text.as_str(), line.runs.as_slice())
            };
            let base = if runs.is_empty() {
                line.kind.color()
            } else {
                monitor_color(text)
            };
            let rows = if line.nowrap {
                // Keep hard `\n` breaks, but still fold each segment to the pane.
                fixed_rows(text, base)
                    .into_iter()
                    .flat_map(|seg| {
                        let text: String = seg.iter().map(|r| r.text.as_str()).collect();
                        if text.is_empty() {
                            vec![Vec::new()]
                        } else {
                            wrap_monitor_rows(&text, &[], columns, indent, base)
                        }
                    })
                    .collect()
            } else {
                wrap_monitor_rows(text, runs, columns, indent, base)
            };
            for row in rows {
                if !first {
                    job.append(
                        "\n",
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: line.kind.color(),
                            ..Default::default()
                        },
                    );
                }
                first = false;
                if row.is_empty() {
                    continue;
                }
                for run in row {
                    job.append(
                        &run.text,
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: run.color,
                            ..Default::default()
                        },
                    );
                }
            }
        }
        job
    }

    pub fn load_log(&mut self, path: &Path) {
        self.load_log_colored(path, false);
    }

    /// Monitor log: `fm` headers come back green, payload lines light grey.
    pub fn load_monitor_log(&mut self, path: &Path) {
        self.load_log_colored(path, true);
    }

    fn load_log_colored(&mut self, path: &Path, monitor: bool) {
        let Ok(file) = File::open(path) else {
            return;
        };
        for raw in BufReader::new(file).lines().map_while(Result::ok) {
            let Some((tag, text)) = raw.split_once('|') else {
                continue;
            };
            let kind = match tag {
                "W" | "w" | "O" | "o" => PadKind::Own,
                "G" | "g" | "I" | "i" => PadKind::Incoming,
                _ => continue,
            };
            if monitor {
                self.push_monitor(kind, text);
            } else {
                self.push(kind, text);
            }
        }
    }

    /// Own and incoming rows, oldest first, DOS line endings. System lines stay out.
    pub fn transcript_text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            if line.kind == PadKind::System {
                continue;
            }
            let text = line.text.replace(['\r', '\n'], " ").trim().to_string();
            if text.is_empty() {
                continue;
            }
            out.push_str(&text);
            out.push_str("\r\n");
        }
        out
    }

    pub fn save_log(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            if let Err(e) = crate::paths::ensure_dir(parent) {
                warn!(error = %e, "pad log mkdir");
                return;
            }
        }
        let Ok(mut file) = File::create(path) else {
            warn!(path = %path.display(), "pad log create failed");
            return;
        };
        for line in &self.lines {
            let tag = match line.kind {
                PadKind::Own => "W",
                PadKind::Incoming => "G",
                PadKind::System => continue,
            };
            let text = line.text.replace(['\r', '\n'], " ");
            if let Err(e) = writeln!(file, "{tag}|{text}") {
                warn!(error = %e, "pad log write");
                return;
            }
        }
    }
}

/// `data/LOG/PADS` (created on save).
pub fn pads_dir(log_dir: &Path) -> PathBuf {
    log_dir.join("PADS")
}

pub fn monitor_path(log_dir: &Path) -> PathBuf {
    pads_dir(log_dir).join("monitor.txt")
}

pub fn channel_path(log_dir: &Path, ch: u8) -> PathBuf {
    pads_dir(log_dir).join(format!("ch{ch:02}.txt"))
}

pub fn spy_path(log_dir: &Path, slot: u8) -> PathBuf {
    pads_dir(log_dir).join(format!("spy{slot:02}.txt"))
}

pub fn bbs_path(log_dir: &Path) -> PathBuf {
    pads_dir(log_dir).join("bbs.txt")
}

pub fn box_path(log_dir: &Path) -> PathBuf {
    pads_dir(log_dir).join("box.txt")
}

/// `fm CALL to CALL ctl …`, including the `fm HH:MM` stamp.
fn monitor_color(line: &str) -> Color32 {
    if line.starts_with("fm ") || line.starts_with("FM ") {
        MONITOR_FM
    } else {
        MONITOR_PAYLOAD
    }
}

/// `<0x1c>` (two hex digits) or `<U+2028>` (four hex digits).
fn hex_token_len(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    if b.len() >= 6
        && b[0] == b'<'
        && (b[1] == b'0')
        && (b[2] == b'x' || b[2] == b'X')
        && b[5] == b'>'
        && b[3].is_ascii_hexdigit()
        && b[4].is_ascii_hexdigit()
    {
        return Some(6);
    }
    if b.len() >= 8
        && b.starts_with(b"<U+")
        && b[7] == b'>'
        && b[3..7].iter().all(|c| c.is_ascii_hexdigit())
    {
        return Some(8);
    }
    None
}

fn monitor_paint_runs(text: &str, base: Color32) -> Vec<PadRun> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(len) = hex_token_len(&text[i..]) {
            push_run(&mut runs, text[i..i + len].to_string(), MONITOR_HEX);
            i += len;
            continue;
        }
        let start = i;
        i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        while i < text.len() && hex_token_len(&text[i..]).is_none() {
            let ch = text[i..].chars().next().unwrap();
            i += ch.len_utf8();
        }
        push_run(&mut runs, text[start..i].to_string(), base);
    }
    runs
}

/// `fm` headers and system rows wrap at column 0. Payload keeps three spaces.
fn monitor_wrap_indent(line: &PadLine) -> usize {
    if line.kind == PadKind::System {
        return 0;
    }
    if line.text.starts_with("fm ") || line.text.starts_with("FM ") {
        0
    } else {
        3
    }
}

struct Piece {
    text: String,
    color: Color32,
    /// `<0xNN>` / `<U+NNNN>` — never split, and never left behind a space.
    token: bool,
}

struct WrapRow {
    runs: Vec<PadRun>,
    used: usize,
    /// Cells already taken by the continuation indent. Real text starts after this.
    floor: usize,
    /// Continuation indent is written on the first piece, not before we know one exists.
    fresh: bool,
}

/// Keep `\n` as the only row break. Do not fold the text to `columns`.
fn fixed_rows(text: &str, color: Color32) -> Vec<Vec<PadRun>> {
    if text.is_empty() {
        return vec![Vec::new()];
    }
    text.split('\n')
        .map(|part| {
            let part = part.trim_end_matches('\r');
            if part.is_empty() {
                Vec::new()
            } else {
                vec![PadRun {
                    text: part.to_string(),
                    color,
                }]
            }
        })
        .collect()
}

fn wrap_monitor_rows(
    text: &str,
    runs: &[PadRun],
    columns: usize,
    indent: usize,
    indent_color: Color32,
) -> Vec<Vec<PadRun>> {
    let columns = columns.max(indent.saturating_add(1));
    if text.is_empty() {
        return vec![Vec::new()];
    }
    let pieces = pieces_of(text, runs, indent_color);
    let mut rows: Vec<Vec<PadRun>> = Vec::new();
    let mut row = WrapRow {
        runs: Vec::new(),
        used: 0,
        floor: 0,
        fresh: false,
    };
    let mut i = 0;
    while i < pieces.len() {
        row.ensure_indent(indent, indent_color);
        let piece = &pieces[i];
        let width = piece.text.chars().count();
        if piece.text == " " {
            let next = pieces
                .get(i + 1)
                .map(|p| p.text.chars().count())
                .unwrap_or(0);
            if row.used > row.floor && row.used + 1 + next > columns {
                rows.push(std::mem::take(&mut row.runs));
                row = WrapRow::continuation();
                i += 1;
                continue;
            }
        }
        if row.used + width <= columns {
            row.push(&piece.text, piece.color);
            i += 1;
            continue;
        }
        if row.used > row.floor {
            rows.push(std::mem::take(&mut row.runs));
            row = WrapRow::continuation();
            continue;
        }
        if piece.token {
            row.push(&piece.text, piece.color);
            i += 1;
            continue;
        }
        for ch in piece.text.chars() {
            row.ensure_indent(indent, indent_color);
            if row.used + 1 > columns && row.used > row.floor {
                rows.push(std::mem::take(&mut row.runs));
                row = WrapRow::continuation();
                row.ensure_indent(indent, indent_color);
            }
            row.push(&ch.to_string(), piece.color);
        }
        i += 1;
    }
    if !row.runs.is_empty() || rows.is_empty() {
        rows.push(row.runs);
    }
    rows
}

impl WrapRow {
    fn continuation() -> Self {
        Self {
            runs: Vec::new(),
            used: 0,
            floor: 0,
            fresh: true,
        }
    }

    fn ensure_indent(&mut self, indent: usize, color: Color32) {
        if !self.fresh {
            return;
        }
        self.fresh = false;
        self.floor = indent;
        if indent > 0 {
            self.push(&" ".repeat(indent), color);
        }
    }

    fn push(&mut self, text: &str, color: Color32) {
        if text.is_empty() {
            return;
        }
        self.used += text.chars().count();
        if let Some(last) = self.runs.last_mut() {
            if last.color == color {
                last.text.push_str(text);
                return;
            }
        }
        self.runs.push(PadRun {
            text: text.to_string(),
            color,
        });
    }
}

fn pieces_of(text: &str, runs: &[PadRun], fallback: Color32) -> Vec<Piece> {
    let owned;
    let runs = if runs.is_empty() {
        owned = vec![PadRun {
            text: text.to_string(),
            color: fallback,
        }];
        owned.as_slice()
    } else {
        runs
    };
    let mut out = Vec::new();
    for run in runs {
        push_pieces(&mut out, &run.text, run.color);
    }
    out
}

fn push_pieces(out: &mut Vec<Piece>, text: &str, color: Color32) {
    let mut i = 0;
    while i < text.len() {
        if let Some(len) = hex_token_len(&text[i..]) {
            out.push(Piece {
                text: text[i..i + len].to_string(),
                color,
                token: true,
            });
            i += len;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        if ch == ' ' {
            out.push(Piece {
                text: " ".to_string(),
                color,
                token: false,
            });
            i += ch.len_utf8();
            continue;
        }
        let start = i;
        i += ch.len_utf8();
        while i < text.len() && hex_token_len(&text[i..]).is_none() {
            let next = text[i..].chars().next().unwrap();
            if next == ' ' {
                break;
            }
            i += next.len_utf8();
        }
        out.push(Piece {
            text: text[start..i].to_string(),
            color,
            token: false,
        });
    }
}

fn push_run(runs: &mut Vec<PadRun>, text: String, color: Color32) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.color == color {
            last.text.push_str(&text);
            return;
        }
    }
    runs.push(PadRun { text, color });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_fm_is_green_and_payload_is_grey() {
        let mut pad = TextPad::new(8);
        pad.push_monitor(PadKind::Own, "fm 21:14 NL0MSK to NL0MSK-2 ctl I00^");
        pad.push_monitor(PadKind::Incoming, "   hallo");
        pad.push_monitor(PadKind::System, "*** CONNECTED");
        let lines: Vec<_> = pad.lines.iter().collect();
        assert_eq!(lines[0].runs[0].color, MONITOR_FM);
        assert_eq!(lines[1].runs[0].color, MONITOR_PAYLOAD);
        assert!(lines[2].runs.is_empty());
        assert_eq!(lines[2].kind, PadKind::System);
    }

    #[test]
    fn nonprintable_is_a_darker_hex_token_on_the_same_row() {
        let mut pad = TextPad::new(4);
        pad.push_monitor(PadKind::Incoming, "   ab\u{1c}cd\nb");
        assert_eq!(pad.lines.len(), 1);
        assert_eq!(pad.lines[0].text, "   ab<0x1c>cd<0x0a>b");
        let runs = &pad.lines[0].runs;
        assert_eq!(runs[0].text, "   ab");
        assert_eq!(runs[0].color, MONITOR_PAYLOAD);
        assert_eq!(runs[1].text, "<0x1c>");
        assert_eq!(runs[1].color, MONITOR_HEX);
        assert_eq!(runs[2].text, "cd");
        assert_eq!(runs[3].text, "<0x0a>");
        assert_eq!(runs[3].color, MONITOR_HEX);
        assert_eq!(runs[4].text, "b");
    }

    fn shown(rows: &[Vec<PadRun>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn payload_wraps_at_a_space_and_keeps_the_indent() {
        let text = "   one two three";
        let runs = monitor_paint_runs(text, MONITOR_PAYLOAD);
        let rows = wrap_monitor_rows(text, &runs, 10, 3, MONITOR_PAYLOAD);
        assert_eq!(
            shown(&rows),
            vec!["   one two".to_string(), "   three".to_string()]
        );
        assert!(rows.iter().all(|row| {
            row.iter()
                .map(|run| run.text.chars().count())
                .sum::<usize>()
                <= 10
        }));
    }

    #[test]
    fn hex_token_moves_whole_onto_the_next_indented_line() {
        let text = "   abcdef<0x0a>Z";
        let runs = monitor_paint_runs(text, MONITOR_PAYLOAD);
        let rows = wrap_monitor_rows(text, &runs, 12, 3, MONITOR_PAYLOAD);
        assert_eq!(
            shown(&rows),
            vec!["   abcdef".to_string(), "   <0x0a>Z".to_string()]
        );
        assert_eq!(rows[1][1].text, "<0x0a>");
        assert_eq!(rows[1][1].color, MONITOR_HEX);
    }

    #[test]
    fn header_wraps_without_the_payload_indent() {
        let text = "fm ABCDEFGHIJKLMNO";
        let runs = monitor_paint_runs(text, MONITOR_FM);
        let rows = wrap_monitor_rows(text, &runs, 10, 0, MONITOR_FM);
        let shown = shown(&rows);
        assert_eq!(shown[0], "fm");
        assert_eq!(shown[1], "ABCDEFGHIJ");
        assert_eq!(shown[2], "KLMNO");
        assert!(shown.iter().all(|row| row.chars().count() <= 10));
        assert!(shown.iter().all(|row| !row.starts_with("   ")));
    }

    #[test]
    fn hiding_our_rows_leaves_them_stored() {
        let mut pad = TextPad::new(8);
        pad.push(PadKind::Own, "our line");
        pad.push(PadKind::Incoming, "their line");
        let hidden = pad.layout_job_channel_show(40, false, false);
        assert!(!hidden.text.contains("our line"));
        assert!(hidden.text.contains("their line"));
        let shown = pad.layout_job_channel_show(40, false, true);
        assert!(shown.text.contains("our line"));
        assert!(shown.text.contains("their line"));
        assert_eq!(pad.lines.len(), 2);
    }

    #[test]
    fn startup_credits_keep_their_own_line_break() {
        let text = "Graphic Packet NG v2.01 by MichTronics (PD4MV) and Jara Lowel, based on 1994 Graphic Packet (GP),\n the classic MS-Dos AX.25 terminal by Ulf Saran (DH1DAE)";
        let mut pad = TextPad::new(4);
        pad.push_unwrapped(PadKind::System, text);
        let job = pad.layout_job_wrapped(40);
        // Hard `\n` still splits; each side also folds to the pane width.
        assert!(job.text.contains("Ulf Saran"));
        assert!(job.text.lines().all(|row| row.chars().count() <= 40));
    }

    #[test]
    fn long_payload_with_hex_tokens_stays_within_columns() {
        let text = "   e gebruik ik op een oude P3 200 MHz,<0x0a>onder DOS. als je ook van die oude goede tijd wild genieten neem dan,<0x0a>";
        let runs = monitor_paint_runs(text, MONITOR_PAYLOAD);
        let rows = wrap_monitor_rows(text, &runs, 48, 3, MONITOR_PAYLOAD);
        assert!(rows.iter().all(|row| {
            row.iter()
                .map(|run| run.text.chars().count())
                .sum::<usize>()
                <= 48
        }));
        assert!(shown(&rows).iter().any(|r| r.contains("<0x0a>")));
        assert!(shown(&rows).iter().all(|r| r.starts_with("   ")));
    }
}
