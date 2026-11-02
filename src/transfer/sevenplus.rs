//! 7Plus / LCPlus **Code-AutoSave** sniffer (classic GP “7+” switch).
//!
//! This is **not** 7-Zip and **not** AutoBin. GP watches the text QSO for
//! `go_7+.` … `stop_7+.` markers (strings present in `GP2862K.EXE`) and
//! writes the enclosed lines to a part file so external `7plus` can decode.
//!
//! Full radix-216 encode/decode stays in the DG1BBQ `7plus` tool; we only
//! need reliable capture for interoperability with DOS GP.

use std::path::{Path, PathBuf};

/// Markers observed in Graphic Packet 1.63.
pub const GO_7PLUS: &str = "go_7+.";
pub const STOP_7PLUS: &str = "stop_7+.";
pub const GO_TEXT: &str = "go_text.";
pub const STOP_TEXT: &str = "stop_text.";
pub const GO_INFO: &str = "go_info.";
pub const STOP_INFO: &str = "stop_info.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SevenPlusKind {
    SevenPlus,
    Text,
    Info,
}

#[derive(Debug, Default)]
pub struct SevenPlusAutosave {
    pub enabled: bool,
    active: Option<Capture>,
}

#[derive(Debug)]
struct Capture {
    kind: SevenPlusKind,
    /// Suggested filename if parsed from header lines; else generated.
    name: Option<String>,
    body: String,
}

#[derive(Debug, Clone)]
pub struct SavedPart {
    pub kind: SevenPlusKind,
    pub suggested_name: Option<String>,
    pub body: String,
}

impl SevenPlusAutosave {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            active: None,
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.active = None;
        }
    }

    /// Feed one RX text line (no requirement that autosave be mid-capture).
    /// Returns a finished part when a stop marker closes a capture.
    pub fn on_line(&mut self, line: &str) -> Option<SavedPart> {
        if !self.enabled {
            return None;
        }
        let trimmed = line.trim();

        if let Some(kind) = detect_go(trimmed) {
            self.active = Some(Capture {
                kind,
                name: None,
                body: String::new(),
            });
            // Keep the go-line in the saved part — 7plus expects headers.
            if let Some(cap) = self.active.as_mut() {
                cap.body.push_str(line);
                cap.body.push('\n');
            }
            return None;
        }

        if let Some(cap) = self.active.as_mut() {
            if detect_stop(trimmed, cap.kind) {
                cap.body.push_str(line);
                cap.body.push('\n');
                let finished = self.active.take().unwrap();
                return Some(SavedPart {
                    kind: finished.kind,
                    suggested_name: finished.name.or_else(|| guess_name(&finished.body)),
                    body: finished.body,
                });
            }
            if cap.name.is_none() {
                if let Some(n) = guess_name_from_line(trimmed) {
                    cap.name = Some(n);
                }
            }
            cap.body.push_str(line);
            cap.body.push('\n');
        }
        None
    }

    /// Write a captured part into `dir` using the suggested or fallback name.
    pub fn write_part(dir: &Path, part: &SavedPart, fallback_stem: &str) -> std::io::Result<PathBuf> {
        let name = part
            .suggested_name
            .clone()
            .unwrap_or_else(|| format!("{fallback_stem}.p01"));
        let path = dir.join(name);
        std::fs::write(&path, part.body.as_bytes())?;
        Ok(path)
    }
}

fn detect_go(line: &str) -> Option<SevenPlusKind> {
    let l = line.to_ascii_lowercase();
    if l.contains(GO_7PLUS) {
        Some(SevenPlusKind::SevenPlus)
    } else if l.contains(GO_TEXT) {
        Some(SevenPlusKind::Text)
    } else if l.contains(GO_INFO) {
        Some(SevenPlusKind::Info)
    } else {
        None
    }
}

fn detect_stop(line: &str, kind: SevenPlusKind) -> bool {
    let l = line.to_ascii_lowercase();
    match kind {
        SevenPlusKind::SevenPlus => l.contains(STOP_7PLUS),
        SevenPlusKind::Text => l.contains(STOP_TEXT),
        SevenPlusKind::Info => l.contains(STOP_INFO),
    }
}

fn guess_name_from_line(line: &str) -> Option<String> {
    // Heuristic: a token ending in .pNN / .7pl / .err / .cor
    for tok in line.split_whitespace() {
        let lower = tok.to_ascii_lowercase();
        if lower.ends_with(".7pl")
            || lower.ends_with(".err")
            || lower.ends_with(".cor")
            || (lower.contains(".p")
                && lower
                    .rsplit('.')
                    .next()
                    .is_some_and(|ext| ext.starts_with('p') && ext.len() == 3
                        && ext[1..].bytes().all(|b| b.is_ascii_digit())))
        {
            return Some(tok.trim_matches(|c: char| c == '"' || c == '\'').to_string());
        }
    }
    None
}

fn guess_name(body: &str) -> Option<String> {
    body.lines().find_map(guess_name_from_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_between_markers() {
        let mut auto = SevenPlusAutosave::new(true);
        assert!(auto.on_line(" go_7+. ").is_none());
        assert!(auto.on_line("demo.p01").is_none());
        assert!(auto.on_line("payload-line").is_none());
        let part = auto.on_line(" stop_7+. ").unwrap();
        assert_eq!(part.suggested_name.as_deref(), Some("demo.p01"));
        assert!(part.body.contains("payload-line"));
        assert!(part.body.contains(GO_7PLUS) || part.body.to_ascii_lowercase().contains("go_7+"));
    }

    #[test]
    fn disabled_ignores() {
        let mut auto = SevenPlusAutosave::new(false);
        assert!(auto.on_line(" go_7+. ").is_none());
        assert!(auto.on_line(" stop_7+. ").is_none());
    }
}
