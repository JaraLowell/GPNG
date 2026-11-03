//! Persist RX pads across restarts (white Own / green Incoming only).
//!
//! Files under `[paths].log_dir`/`pads/`:
//! `monitor.txt`, `ch01.txt`…, `spy01.txt`…, `bbs.txt`, `box.txt`.
//!
//! Record format: `W|text` (own/white) or `G|text` (incoming/green).
//! Orange System lines are never written.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId};
use tracing::warn;

const PAD_INCOMING: Color32 = Color32::from_rgb(85, 255, 85); // light green
const PAD_OWN: Color32 = Color32::WHITE;
const PAD_SYSTEM: Color32 = Color32::from_rgb(255, 168, 0); // orange

/// Classic GP line roles — colour carries meaning (no `***` needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    /// RX / decoded APRS / monitored frames (green) — persisted.
    Incoming,
    /// Local TX echo (white) — persisted.
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

    /// Push a row and return its id.
    pub fn push_seq(&mut self, kind: PadKind, line: impl Into<String>) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.lines.push_back(PadLine {
            kind,
            text: line.into(),
            runs: Vec::new(),
            seq,
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
        });
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
        seq
    }

    pub fn text_if_seq(&self, seq: u64) -> Option<String> {
        self.lines
            .back()
            .filter(|line| line.seq == seq)
            .map(|line| line.text.clone())
    }

    pub fn layout_job(&self) -> LayoutJob {
        let mut job = LayoutJob::default();
        let font = FontId::monospace(14.0);
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
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
            if line.runs.is_empty() {
                job.append(
                    &line.text,
                    0.0,
                    TextFormat {
                        font_id: font.clone(),
                        color: line.kind.color(),
                        ..Default::default()
                    },
                );
            } else {
                for run in &line.runs {
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
            self.push(kind, text);
        }
    }

    pub fn save_log(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
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

/// `data/log/pads` (created on save).
pub fn pads_dir(log_dir: &Path) -> PathBuf {
    log_dir.join("pads")
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
