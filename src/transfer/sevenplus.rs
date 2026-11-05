//! 7Plus / LCPlus **Code-AutoSave** sniffer (classic GP “7+” switch).
//!
//! This is **not** 7-Zip and **not** AutoBin. 7Plus (DG1BBQ) is an amateur
//! radio text encoding: a binary file is written as ASCII lines so it can
//! travel inside AX.25, which cannot carry every byte safely. GP watches
//! the QSO for `go_7+.` … `stop_7+.` (strings present in `GP2862K.EXE`) and
//! writes the enclosed lines into the USER directory. Joining those parts
//! back to the original file, and splitting a file before send, is
//! [`super::sevenplus_codec`] (a Rust port of 7PLUS 2.2, not the DOS exe).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Dropped into USER the first time the directory is created.
const USER_README: &str = "\
USER — Graphic Packet user directory

7Plus parts are saved here when the 7+ switch is ON.
7Plus is the amateur-radio text encoding (DG1BBQ), not 7-Zip.
It rewrites a binary file as ASCII lines so those bytes can travel
inside an AX.25 packet. A part looks like:

    go_7+.
    ...
    name.p01
    ...
    stop_7+.

//Read name.p01
    sends that file from this folder to the connected station.

//Read
    lists this folder.

//7+ ON
//7+ OFF
    same switch as the 7+ button on the session bar.
";

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

    pub fn is_capturing(&self) -> bool {
        self.active.is_some()
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
                name: part_name_from_header(line),
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
    ///
    /// The name is forced to a single file in `dir` (no `..` or other folders).
    /// Lines are stored as DOS CRLF, which `7plus` expects.
    pub fn write_part(dir: &Path, part: &SavedPart, fallback_stem: &str) -> io::Result<PathBuf> {
        ensure_user_dir(dir)?;
        let fallback = format!("{fallback_stem}.p01");
        let name = safe_part_name(
            part.suggested_name.as_deref().unwrap_or(&fallback),
            &fallback,
        );
        let path = dir.join(name);
        fs::write(&path, to_crlf_bytes(&part.body))?;
        Ok(path)
    }
}

/// Create USER (and the short explanation) if it is not there yet.
pub fn ensure_user_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let note = dir.join("README.txt");
    if !note.is_file() {
        fs::write(note, USER_README)?;
    }
    Ok(())
}

/// True when this QSO line opens a 7Plus / text / info part.
pub fn is_opening_line(line: &str) -> bool {
    detect_go(line).is_some()
}

/// One file name inside USER, or an error if the name leaves that folder.
pub fn resolve_user_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(|c| c.is_control())
    {
        return Err("use a file name from USER".into());
    }
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .ok_or_else(|| "use a file name from USER".to_string())?;
    let path = dir.join(base);
    if !path.is_file() {
        return Err(format!("not in USER: {base}"));
    }
    Ok(path)
}

/// File bytes from USER. `//Read` with an empty name lists the folder instead.
pub fn read_user_file(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = resolve_user_file(dir, name)?;
    fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

/// Names in USER, one per line, for a bare `//Read`.
pub fn list_user_dir(dir: &Path) -> String {
    let mut names = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if !path.is_file() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    if names.is_empty() {
        format!("USER {} (empty)", dir.display())
    } else {
        format!("USER {}\r{}", dir.display(), names.join("\r"))
    }
}

/// CR-only line ends for the air. DOS GP treats LF as filler and CR as the row.
pub fn to_cr_bytes(raw: &[u8]) -> Vec<u8> {
    if raw.contains(&b'\r') {
        raw.iter().copied().filter(|b| *b != b'\n').collect()
    } else {
        raw.iter()
            .copied()
            .map(|b| if b == b'\n' { b'\r' } else { b })
            .collect()
    }
}

fn detect_go(line: &str) -> Option<SevenPlusKind> {
    // hb9xar/7plus extract.c: a part opens on ` go_7+.`, ` go_text.`, or
    // ` go_info.` at the start of the line. Some BBS software drops the
    // leading space, so the check is on the trimmed line. The rest of a
    // real header is long (`001 of 003 NAME …`); do not require a short line.
    if marker_at_start(line, GO_7PLUS) {
        Some(SevenPlusKind::SevenPlus)
    } else if marker_at_start(line, GO_TEXT) {
        Some(SevenPlusKind::Text)
    } else if marker_at_start(line, GO_INFO) {
        Some(SevenPlusKind::Info)
    } else {
        None
    }
}

fn detect_stop(line: &str, kind: SevenPlusKind) -> bool {
    match kind {
        SevenPlusKind::SevenPlus => marker_at_start(line, STOP_7PLUS),
        SevenPlusKind::Text => marker_at_start(line, STOP_TEXT),
        SevenPlusKind::Info => marker_at_start(line, STOP_INFO),
    }
}

/// True when `marker` is the start of the line (optional surrounding space).
/// A code row that merely contains the letters later on is not a marker.
fn marker_at_start(line: &str, marker: &str) -> bool {
    line.trim().to_ascii_lowercase().starts_with(marker)
}

/// Part file name the way `extract.c` builds it from the go-line.
///
/// ` go_7+. 001 of 003 PICTURE.GIF …` → `picture.p01`
/// (one part, `of 001`, → `picture.7pl`). Part numbers are hex, so part 10
/// is `picture.p0a`. `go_text.` / `go_info.` keep the name token (`.err`, `.cor`).
fn part_name_from_header(line: &str) -> Option<String> {
    let t = line.trim();
    let lower = t.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix(GO_7PLUS)
        .map(|tail_lower| &t[t.len() - tail_lower.len()..])
    {
        let mut toks = rest.split_whitespace();
        let part: u32 = toks.next()?.parse().ok()?;
        let of = toks.next()?;
        if !of.eq_ignore_ascii_case("of") {
            return None;
        }
        let parts: u32 = toks.next()?.parse().ok()?;
        let dest = toks.next()?;
        let stem = dos_stem(dest);
        if stem.is_empty() {
            return None;
        }
        let name = if parts <= 1 {
            format!("{stem}.7pl")
        } else {
            format!("{stem}.p{part:02x}")
        };
        return Some(name);
    }
    if let Some(rest) = strip_marker(t, GO_TEXT).or_else(|| strip_marker(t, GO_INFO)) {
        let token = rest.split_whitespace().next()?;
        let name = safe_part_name(token, "");
        if name.is_empty() {
            return None;
        }
        return Some(name);
    }
    None
}

fn strip_marker<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    let tail = lower.strip_prefix(marker)?;
    Some(&line[line.len() - tail.len()..])
}

/// 8.3 stem: drop the extension, keep 8 DOS characters, lower case.
fn dos_stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = match base.rfind('.') {
        Some(dot) if dot > 0 => &base[..dot],
        _ => base,
    };
    stem.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn safe_part_name(name: &str, fallback: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '(' | ')'))
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn to_crlf_bytes(body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 16);
    for ch in body.chars() {
        if ch == '\n' {
            out.push(b'\r');
            out.push(b'\n');
        } else if (ch as u32) <= 0xFF {
            out.push(ch as u8);
        } else {
            out.push(b'?');
        }
    }
    out
}

fn guess_name_from_line(line: &str) -> Option<String> {
    // Heuristic: a token ending in .pNN / .7pl / .err / .cor
    for tok in line.split_whitespace() {
        let lower = tok.to_ascii_lowercase();
        if lower.ends_with(".7pl")
            || lower.ends_with(".err")
            || lower.ends_with(".cor")
            || (lower.contains(".p")
                && lower.rsplit('.').next().is_some_and(|ext| {
                    // `.p01` … `.pff` — part number is hex (part 10 → `.p0a`).
                    ext.len() == 3
                        && ext.starts_with('p')
                        && ext[1..].bytes().all(|b| b.is_ascii_hexdigit())
                }))
        {
            return Some(
                tok.trim_matches(|c: char| c == '"' || c == '\'')
                    .to_string(),
            );
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

    #[test]
    fn code_row_containing_the_word_is_not_a_stop() {
        let mut auto = SevenPlusAutosave::new(true);
        assert!(auto.on_line(" go_7+. ").is_none());
        let data = format!(" x{}", "stop_7+.".repeat(8));
        assert!(auto.on_line(&data).is_none());
        assert!(auto.is_capturing());
        assert!(auto.on_line(" stop_7+. (PICTURE.P01/03) [1A2B]").is_some());
    }

    #[test]
    fn header_names_part_like_extract_c() {
        let mut auto = SevenPlusAutosave::new(true);
        let go = " go_7+. 001 of 003 PICTURE.GIF 00015234 04A0 00F2 (7PLUS v2.2)";
        assert!(auto.on_line(go).is_none());
        auto.on_line(" code-line");
        let part = auto
            .on_line(" stop_7+. (PICTURE.P01/03) [1A2B3C4D]")
            .unwrap();
        assert_eq!(part.suggested_name.as_deref(), Some("picture.p01"));

        let mut one = SevenPlusAutosave::new(true);
        one.on_line(" go_7+. 001 of 001 README.TXT 00000012 0040 0001 (7PLUS v2.2)");
        let part = one.on_line(" stop_7+. (README.7PL) [0]").unwrap();
        assert_eq!(part.suggested_name.as_deref(), Some("readme.7pl"));

        let mut tenth = SevenPlusAutosave::new(true);
        tenth.on_line(" go_7+. 010 of 012 MAP.GIF 00099999 0100 0010 (7PLUS v2.2)");
        let part = tenth.on_line(" stop_7+.").unwrap();
        assert_eq!(part.suggested_name.as_deref(), Some("map.p0a"));
    }

    #[test]
    fn writes_crlf_inside_user_dir_only() {
        let dir = std::env::temp_dir().join(format!("gpng-7plus-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut auto = SevenPlusAutosave::new(true);
        auto.on_line(" go_7+. ");
        auto.on_line(" ..\\secret.p01 ");
        auto.on_line("line two");
        let part = auto.on_line(" stop_7+. ").unwrap();
        let path = SevenPlusAutosave::write_part(&dir, &part, "fallback").unwrap();
        assert_eq!(path.parent().unwrap(), dir.as_path());
        assert_eq!(path.file_name().unwrap(), "secret.p01");
        let saved = fs::read(&path).unwrap();
        assert!(saved.windows(2).any(|w| w == b"\r\n"));
        assert!(!saved.contains(&b'\n') || saved.windows(2).any(|w| w == b"\r\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_rejects_paths_outside_user() {
        let dir = std::env::temp_dir().join(format!("gpng-user-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_user_dir(&dir).unwrap();
        assert!(resolve_user_file(&dir, r"..\Config.toml").is_err());
        assert!(resolve_user_file(&dir, "README.txt").is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
