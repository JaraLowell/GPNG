//! Persist RX pads across restarts.
//!
//! Files under `[paths].log_dir`/`PADS/`:
//! `monitor.txt`, `ch01.txt`…, `spy01.txt`…, `bbs.txt`, `box.txt`.
//!
//! Channel, BBS, and Box-List rows: green is what we sent, white is what
//! we received. The monitor paints every `fm … to …` header in the output
//! window's foreground and the payload in its inverse colour, send and
//! receive alike. Those two colours are applied when the pad is drawn, so
//! a setup change recolours the lines already on screen. Orange System
//! lines are never written.
//!
//! Record format: `W|text` (own) or `G|text` (incoming).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId};

/// Monitor, channel, spy, BBS, and Box-List pads. Column wrap uses this size.
pub const PAD_FONT_SIZE: f32 = 14.0;
use tracing::warn;

const PAD_GREEN: Color32 = Color32::from_rgb(85, 255, 85);
/// Received text.
const PAD_INCOMING: Color32 = Color32::WHITE;
/// Sent text.
const PAD_OWN: Color32 = PAD_GREEN;
const PAD_SYSTEM: Color32 = Color32::from_rgb(255, 168, 0); // orange
/// Stored role for an `fm … to … ctl …` line. Drawn as the output foreground.
const MONITOR_FM: Color32 = PAD_GREEN;
/// Stored role for the payload under that header. Drawn as the inverse colour.
const MONITOR_PAYLOAD: Color32 = Color32::from_rgb(170, 170, 170);
/// `<0x1c>` tokens. Drawn darker than the payload so binary stands out.
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
    ansi: Option<crate::ui::ansi::AnsiStyle>,
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

    /// Monitor row. `fm …` and the payload keep role colours here; the output
    /// window's foreground and inverse replace them when the pad is drawn.
    /// Non-printables are already `<0xNN>` or get rewritten to that here, so
    /// they cannot open a new line. System stays orange and is not logged.
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

    /// A monitor payload with complete SGR sequences retained by the byte decoder.
    pub fn push_monitor_sgr(&mut self, kind: PadKind, line: &str, allow_color: bool) {
        let mut decoder = crate::ui::ansi::AnsiDecoder::new();
        let mut text = String::new();
        let mut runs = Vec::new();
        for (part, style) in decoder.push_text(line, allow_color) {
            let part = crate::tnc::ax25::escape_monitor_text(&part);
            text.push_str(&part);
            for mut run in monitor_paint_runs(&part, MONITOR_PAYLOAD) {
                if run.color != MONITOR_HEX && (style.fg.is_some() || style.bold) {
                    run.ansi = Some(style);
                }
                runs.push(run);
            }
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
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
                    ansi: None,
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
            ansi: None,
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
                ansi: None,
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
    /// Header and payload keep their stored role colours.
    pub fn layout_job_wrapped(&self, columns: usize) -> LayoutJob {
        self.layout_job_wrapped_indent(columns, true, false, false, None, None, None)
    }

    /// Monitor pad drawn with the output-window colours.
    /// `header` is Foreground (`fm … to …`). `payload` is Inverse.
    /// `info` is Info / Error (TNC, COM, warnings, "Local call set to").
    pub fn layout_job_monitor(
        &self,
        columns: usize,
        header: Color32,
        payload: Color32,
        info: Color32,
    ) -> LayoutJob {
        self.layout_job_wrapped_indent(
            columns,
            true,
            false,
            false,
            Some((header, payload)),
            Some(info),
            None,
        )
    }

    /// Spy pad. Heard lines use `fg`; the other side uses `inverse`.
    pub fn layout_job_spy(&self, columns: usize, fg: Color32, inverse: Color32) -> LayoutJob {
        self.layout_job_wrapped_indent(
            columns,
            false,
            false,
            false,
            None,
            None,
            Some((fg, inverse)),
        )
    }

    /// Channel / spy / mail pads: wrap at `columns` with no continuation indent.
    pub fn layout_job_wrapped_plain(&self, columns: usize) -> LayoutJob {
        self.layout_job_wrapped_indent(columns, false, false, false, None, None, None)
    }

    /// Channel, BBS-Menu, and Box-List. `umlaut` maps the seven Latin-1
    /// glyphs back to äöüÄÖÜß. The stored row stays CP437, so turning the
    /// button off shows the shift again.
    pub fn layout_job_channel(&self, columns: usize, umlaut: bool) -> LayoutJob {
        self.layout_job_channel_show(columns, umlaut, true, None)
    }

    /// Same as [`layout_job_channel`]. `show_own` false leaves our rows in the
    /// pad and omits them from the picture. `system` recolours warning lines.
    pub fn layout_job_channel_show(
        &self,
        columns: usize,
        umlaut: bool,
        show_own: bool,
        system: Option<Color32>,
    ) -> LayoutJob {
        self.layout_job_wrapped_indent(columns, false, umlaut, !show_own, None, system, None)
    }

    fn layout_job_wrapped_indent(
        &self,
        columns: usize,
        monitor_indent: bool,
        umlaut: bool,
        hide_own: bool,
        monitor_colors: Option<(Color32, Color32)>,
        system_color: Option<Color32>,
        spy_colors: Option<(Color32, Color32)>,
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
            let text = if umlaut {
                shifted = crate::ui::editor::show_channel_text(&line.text, true);
                shifted.as_str()
            } else {
                line.text.as_str()
            };
            let shifted_runs;
            let runs = if umlaut || monitor_colors.is_some() {
                shifted_runs = line
                    .runs
                    .iter()
                    .map(|run| {
                        let color = match monitor_colors {
                            Some((header, payload)) => match run.ansi {
                                Some(style) => style.paint(payload),
                                None => monitor_paint_color(run.color, header, payload),
                            },
                            None => run.color,
                        };
                        PadRun {
                            text: if umlaut {
                                crate::ui::editor::show_channel_text(&run.text, true)
                            } else {
                                run.text.clone()
                            },
                            color,
                            ansi: None,
                        }
                    })
                    .collect::<Vec<_>>();
                shifted_runs.as_slice()
            } else {
                line.runs.as_slice()
            };
            let base = if line.runs.is_empty() {
                line.kind.color()
            } else {
                monitor_color(text)
            };
            let base = match monitor_colors {
                Some((header, payload)) => monitor_paint_color(base, header, payload),
                None => base,
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
                    let mut color = run.color;
                    if run.color == PAD_SYSTEM {
                        if let Some(info) = system_color {
                            color = info;
                        }
                    }
                    if let Some((fg, inverse)) = spy_colors {
                        if run.color == PAD_INCOMING {
                            color = fg;
                        } else if run.color == PAD_OWN {
                            color = inverse;
                        }
                    }
                    job.append(
                        &run.text,
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color,
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

    /// Monitor log: rows come back with header and payload roles. The live
    /// foreground and inverse colours are applied when the pad is drawn.
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

/// Role colour stored on the row. Foreground and inverse replace these at draw time.
fn monitor_paint_color(stored: Color32, header: Color32, payload: Color32) -> Color32 {
    if stored == MONITOR_FM {
        header
    } else if stored == MONITOR_PAYLOAD {
        payload
    } else if stored == MONITOR_HEX {
        dim_payload(payload)
    } else {
        stored
    }
}

/// `<0xNN>` stays about 112/170 of the payload, which is the old dark grey
/// when the payload is light grey.
fn dim_payload(color: Color32) -> Color32 {
    let scale = |v: u8| ((u16::from(v) * 112) / 170) as u8;
    Color32::from_rgb(scale(color.r()), scale(color.g()), scale(color.b()))
}

/// `fm CALL to CALL ctl …`, including `fm2` (KISS port) and the `fm HH:MM` stamp.
fn monitor_color(line: &str) -> Color32 {
    if crate::tnc::ax25::split_monitor_fm(line).is_some() {
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
    if crate::tnc::ax25::split_monitor_fm(&line.text).is_some() {
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
                    ansi: None,
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
            ansi: None,
        });
    }
}

fn pieces_of(text: &str, runs: &[PadRun], fallback: Color32) -> Vec<Piece> {
    let owned;
    let runs = if runs.is_empty() {
        owned = vec![PadRun {
            text: text.to_string(),
            color: fallback,
            ansi: None,
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
    runs.push(PadRun {
        text,
        color,
        ansi: None,
    });
}
