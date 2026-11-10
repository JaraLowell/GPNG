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
use std::time::Instant;

use super::sevenplus_codec::{self, DecodedFile};

/// Classic 7plus `-sb` for `//Read` encode (about 3000 source bytes per part).
pub const DEFAULT_PART_BYTES: u32 = 3000;

/// Result of joining a complete part set in USER.
#[derive(Debug, Clone)]
pub struct JoinedSet {
    pub path: PathBuf,
    pub name: String,
    pub deleted_parts: Vec<PathBuf>,
}

/// Outcome of trying to assemble parts after a Code-AutoSave.
#[derive(Debug, Clone)]
pub enum AssembleOutcome {
    /// Not a usable 7Plus part (ignore).
    Ignored,
    /// Set is not complete yet. `missing` holds names like `readme.p09`.
    Incomplete { missing: Vec<String> },
    /// Original file written; part files removed.
    Joined(JoinedSet),
}

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
    sends that part file as text on the channel.

//Read name.txt
    encodes name.txt with 7Plus into .p01 / .7pl parts here, then
    reports that the parts are ready. The peer pulls each part with
    //Read name.p01 and so on.

//7DECODE name.p01
//7DECODE name.txt
    joins every part of that set in this folder into the original file
    (or name (2).ext if that name is taken). Reports missing parts.

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
    /// Expected part size from the `go_7+.` span field (hex). Plain text
    /// / info captures have no total, so the progress strip stays closed.
    expected: Option<usize>,
    started: Instant,
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

    /// Drop the open part. The 7+ switch stays as it was.
    pub fn abort_capture(&mut self) -> bool {
        self.active.take().is_some()
    }

    /// Bytes in / expected / part name / start time while a sized `go_7+.`
    /// capture is open. No total → no progress strip (plain text / info).
    pub fn progress(&self) -> Option<(u64, u64, String, Instant)> {
        let cap = self.active.as_ref()?;
        let expected = cap.expected?;
        if expected == 0 {
            return None;
        }
        let got = cap.body.len() as u64;
        let total = (expected as u64).max(got);
        let name = cap.name.clone().unwrap_or_else(|| "7plus".into());
        Some((got, total, name, cap.started))
    }

    /// Feed one RX text line (no requirement that autosave be mid-capture).
    /// Returns a finished part when a stop marker closes a capture.
    pub fn on_line(&mut self, line: &str) -> Option<SavedPart> {
        if !self.enabled {
            return None;
        }
        let trimmed = line.trim();

        if let Some(kind) = detect_go(trimmed) {
            let name = part_name_from_header(line);
            let expected = expected_part_bytes(line, kind);
            self.active = Some(Capture {
                kind,
                name,
                body: String::new(),
                expected,
                started: Instant::now(),
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
    crate::paths::ensure_dir(dir)?;
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

/// True when `name` is already a 7Plus / LCPlus part (`.p01`…`.pff`, `.7pl`, `.err`, `.cor`).
pub fn is_part_filename(name: &str) -> bool {
    let base = Path::new(name.trim())
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let lower = base.to_ascii_lowercase();
    if lower.ends_with(".7pl") || lower.ends_with(".err") || lower.ends_with(".cor") {
        return true;
    }
    lower.rsplit_once('.').is_some_and(|(_, ext)| {
        ext.len() == 3
            && ext.as_bytes()[0] == b'p'
            && ext.as_bytes()[1].is_ascii_hexdigit()
            && ext.as_bytes()[2].is_ascii_hexdigit()
    })
}

/// Windows-style unique path: `name.ext` → `name (2).ext` when taken.
pub fn unique_user_path(dir: &Path, name: &str) -> PathBuf {
    let safe = safe_part_name(name, "file.bin");
    let primary = dir.join(&safe);
    if !primary.exists() {
        return primary;
    }
    let (stem, ext) = split_stem_ext(&safe);
    for n in 2..10_000 {
        let candidate = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let path = dir.join(&candidate);
        if !path.exists() {
            return path;
        }
    }
    dir.join(format!("{stem} (copy)"))
}

/// Encode a USER file into `.pXX` / `.7pl` parts beside it. Returns how many parts.
pub fn encode_user_file(dir: &Path, name: &str, data: &[u8]) -> Result<usize, String> {
    ensure_user_dir(dir).map_err(|e| format!("USER: {e}"))?;
    let parts = sevenplus_codec::encode_parts(name, data, DEFAULT_PART_BYTES)?;
    for part in &parts {
        let path = dir.join(&part.name);
        fs::write(&path, &part.body).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(parts.len())
}

/// After a part was saved: if every part of that set is in `dir`, decode, write the
/// original (or `name (2).ext`), and delete the part files. If some parts are still
/// missing, return their file names (`readme.p09`, …) so the UI can report them.
pub fn try_decode_complete_set(dir: &Path, just_saved: &Path) -> Result<AssembleOutcome, String> {
    assemble_parts(dir, just_saved, false)
}

/// `//7DECODE <file>` — look in USER, read part count from the first part, join
/// when complete. Always reports missing part names when the set is incomplete.
pub fn decode_user_parts(dir: &Path, name: &str) -> Result<AssembleOutcome, String> {
    let seed = resolve_seed_part(dir, name)?;
    assemble_parts(dir, &seed, true)
}

fn assemble_parts(
    dir: &Path,
    just_saved: &Path,
    always_list_missing: bool,
) -> Result<AssembleOutcome, String> {
    let raw = fs::read(just_saved).map_err(|e| format!("read {}: {e}", just_saved.display()))?;
    let meta = match sevenplus_codec::part_meta(&raw) {
        Ok(m) => m,
        Err(_) => return Ok(AssembleOutcome::Ignored),
    };
    let stem = dos_stem(&meta.name);
    if stem.is_empty() {
        return Ok(AssembleOutcome::Ignored);
    }
    let present = present_part_mask(dir, &stem, meta.parts);
    let missing: Vec<String> = (0..meta.parts)
        .filter(|&i| !present[i])
        .map(|i| part_disk_name(&stem, i + 1, meta.parts))
        .collect();
    if !missing.is_empty() {
        // AutoSave stays quiet mid-transfer; //7DECODE always lists gaps.
        let last_seen = meta.part == meta.parts || present[meta.parts - 1];
        return Ok(AssembleOutcome::Incomplete {
            missing: if always_list_missing || last_seen {
                missing
            } else {
                Vec::new()
            },
        });
    }
    let Some(part_paths) = find_part_files(dir, &stem, meta.parts) else {
        return Ok(AssembleOutcome::Incomplete {
            missing: missing_part_names(dir, &stem, meta.parts),
        });
    };
    let mut bodies = Vec::with_capacity(part_paths.len());
    for path in &part_paths {
        bodies.push(fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?);
    }
    let DecodedFile { name, data } = sevenplus_codec::decode_parts(&bodies)?;
    let out = unique_user_path(dir, &name);
    let out_name = out
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&name)
        .to_string();
    fs::write(&out, &data).map_err(|e| format!("write {}: {e}", out.display()))?;
    let mut deleted = Vec::new();
    for path in part_paths {
        match fs::remove_file(&path) {
            Ok(()) => deleted.push(path),
            Err(e) => {
                return Err(format!(
                    "decoded {out_name} but could not delete {}: {e}",
                    path.display()
                ));
            }
        }
    }
    Ok(AssembleOutcome::Joined(JoinedSet {
        path: out,
        name: out_name,
        deleted_parts: deleted,
    }))
}

/// Pick a seed part in USER from a part name, original name, or stem.
fn resolve_seed_part(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("7DECODE needs a file name".into());
    }
    if name.contains('/')
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
    let direct = dir.join(base);
    if direct.is_file() && is_part_filename(base) {
        return Ok(direct);
    }
    let stem = dos_stem(base);
    if stem.is_empty() {
        return Err(format!("not in USER: {base}"));
    }
    for candidate in [part_disk_name(&stem, 1, 2), part_disk_name(&stem, 1, 1)] {
        if let Some(path) = find_named_file(dir, &candidate) {
            return Ok(path);
        }
    }
    Err(format!("not in USER: {base}"))
}

fn find_named_file(dir: &Path, want: &str) -> Option<PathBuf> {
    let want_l = want.to_ascii_lowercase();
    let direct = dir.join(want);
    if direct.is_file() {
        return Some(direct);
    }
    let rd = fs::read_dir(dir).ok()?;
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(&want_l))
        {
            return Some(path);
        }
    }
    None
}

/// Part file name for index `part` of `parts` (`stem.p09`, `stem.7pl`).
pub fn part_disk_name(stem: &str, part: usize, parts: usize) -> String {
    let stem = stem.to_ascii_lowercase();
    if parts <= 1 {
        format!("{stem}.7pl")
    } else {
        format!("{stem}.p{part:02x}")
    }
}

fn missing_part_names(dir: &Path, stem: &str, parts: usize) -> Vec<String> {
    let present = present_part_mask(dir, stem, parts);
    let mut missing = Vec::new();
    for i in 0..parts {
        if !present[i] {
            missing.push(part_disk_name(stem, i + 1, parts));
        }
    }
    missing
}

fn present_part_mask(dir: &Path, stem: &str, parts: usize) -> Vec<bool> {
    let stem_l = stem.to_ascii_lowercase();
    let mut present = vec![false; parts];
    let Ok(rd) = fs::read_dir(dir) else {
        return present;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if parts <= 1 {
            if lower == format!("{stem_l}.7pl") {
                present[0] = true;
            }
            continue;
        }
        let Some(rest) = lower.strip_prefix(&format!("{stem_l}.p")) else {
            continue;
        };
        if rest.len() != 2 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(n) = usize::from_str_radix(rest, 16) else {
            continue;
        };
        if n >= 1 && n <= parts {
            present[n - 1] = true;
        }
    }
    present
}

fn split_stem_ext(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            (stem.to_string(), ext.to_string())
        }
        _ => (name.to_string(), String::new()),
    }
}

fn find_part_files(dir: &Path, stem: &str, parts: usize) -> Option<Vec<PathBuf>> {
    let stem_l = stem.to_ascii_lowercase();
    let mut found: Vec<Option<PathBuf>> = (0..parts).map(|_| None).collect();
    let rd = fs::read_dir(dir).ok()?;
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if parts <= 1 {
            if lower == format!("{stem_l}.7pl") {
                found[0] = Some(path);
            }
            continue;
        }
        let Some(rest) = lower.strip_prefix(&format!("{stem_l}.p")) else {
            continue;
        };
        if rest.len() != 2 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(n) = usize::from_str_radix(rest, 16) else {
            continue;
        };
        if n >= 1 && n <= parts {
            found[n - 1] = Some(path);
        }
    }
    if found.iter().all(|p| p.is_some()) {
        Some(found.into_iter().map(|p| p.expect("checked")).collect())
    } else {
        None
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

/// Size estimate for the progress bar from the `go_7+.` span field.
///
/// Header shape: `go_7+. 001 of 003 NAME size span blocklines …`
/// `span` and `blocklines` are hex (same as 7PLUS 2.2). The part file is
/// roughly the go-line, the encoded span, and a short stop trailer.
fn expected_part_bytes(line: &str, kind: SevenPlusKind) -> Option<usize> {
    if kind != SevenPlusKind::SevenPlus {
        return None;
    }
    let t = line.trim();
    let lower = t.to_ascii_lowercase();
    let rest = lower
        .strip_prefix(GO_7PLUS)
        .map(|tail| &t[t.len() - tail.len()..])?;
    let mut toks = rest.split_whitespace();
    let _part = toks.next()?;
    let of = toks.next()?;
    if !of.eq_ignore_ascii_case("of") {
        return None;
    }
    let _parts = toks.next()?;
    let _name = toks.next()?;
    let _size = toks.next()?;
    let span_tok = toks.next()?;
    let span = usize::from_str_radix(span_tok, 16).ok()?;
    if span == 0 {
        return None;
    }
    Some(span.saturating_add(t.len()).saturating_add(80))
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
    fn abort_drops_the_open_part() {
        let mut auto = SevenPlusAutosave::new(true);
        auto.on_line(" go_7+. 001 of 001 README.TXT 00000012 0040 0001 (7PLUS v2.2)");
        auto.on_line("data line");
        assert!(auto.is_capturing());
        assert!(auto.abort_capture());
        assert!(!auto.is_capturing());
        assert!(auto.on_line("more").is_none());
        assert!(!auto.abort_capture());
    }

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
        let (got, total, name, _) = auto.progress().expect("sized 7+ has progress");
        assert_eq!(name, "picture.p01");
        assert!(got > 0);
        assert!(total >= got);
        // span 0x04A0 = 1184, plus header overhead
        assert!(total >= 1184);
        auto.on_line(" code-line");
        let part = auto
            .on_line(" stop_7+. (PICTURE.P01/03) [1A2B3C4D]")
            .unwrap();
        assert_eq!(part.suggested_name.as_deref(), Some("picture.p01"));
        assert!(auto.progress().is_none());

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

    #[test]
    fn part_filename_detects_p_and_7pl() {
        assert!(is_part_filename("README.P01"));
        assert!(is_part_filename("map.p0a"));
        assert!(is_part_filename("note.7pl"));
        assert!(is_part_filename(r"data\USER\pic.P0F"));
        assert!(!is_part_filename("README.TXT"));
        assert!(!is_part_filename("photo.jpg"));
        assert!(!is_part_filename("thing.p0g"));
    }

    #[test]
    fn unique_path_adds_windows_style_copy_number() {
        let dir = std::env::temp_dir().join(format!("gpng-uniq-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("README.TXT"), b"one").unwrap();
        let second = unique_user_path(&dir, "README.TXT");
        assert_eq!(second.file_name().unwrap(), "README (2).TXT");
        fs::write(&second, b"two").unwrap();
        let third = unique_user_path(&dir, "README.TXT");
        assert_eq!(third.file_name().unwrap(), "README (3).TXT");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn encode_user_file_writes_parts_ready_for_read() {
        let dir = std::env::temp_dir().join(format!("gpng-enc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let data = b"Hello from USER for 7plus encode.\r\n".repeat(200);
        let n = encode_user_file(&dir, "HELLO.TXT", &data).unwrap();
        assert!(n >= 1);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case("hello.p01")
                || n.eq_ignore_ascii_case("hello.7pl")),
            "{names:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_set_decodes_and_deletes_parts() {
        let dir = std::env::temp_dir().join(format!("gpng-join-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0u8..200).collect();
        let parts = sevenplus_codec::encode_parts("demo.bin", &data, 80).unwrap();
        assert!(parts.len() > 1);
        for (i, part) in parts.iter().enumerate() {
            let path = dir.join(&part.name);
            fs::write(&path, &part.body).unwrap();
            if i + 1 < parts.len() {
                match try_decode_complete_set(&dir, &path).unwrap() {
                    AssembleOutcome::Incomplete { missing } => assert!(missing.is_empty()),
                    other => panic!("expected quiet incomplete, got {other:?}"),
                }
                assert!(path.is_file());
            } else {
                match try_decode_complete_set(&dir, &path).unwrap() {
                    AssembleOutcome::Joined(joined) => {
                        assert_eq!(joined.name, "demo.bin");
                        assert_eq!(fs::read(&joined.path).unwrap(), data);
                        assert!(!path.is_file(), "last part should be deleted");
                    }
                    other => panic!("expected join, got {other:?}"),
                }
            }
        }
        for part in &parts {
            assert!(
                !dir.join(&part.name).is_file(),
                "part {} should be gone",
                part.name
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_set_uses_copy_number_when_original_exists() {
        let dir = std::env::temp_dir().join(format!("gpng-join2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("demo.bin"), b"already here").unwrap();
        let data = b"replacement payload for collision test!!".to_vec();
        let parts = sevenplus_codec::encode_parts("demo.bin", &data, 0).unwrap();
        assert_eq!(parts.len(), 1);
        let path = dir.join(&parts[0].name);
        fs::write(&path, &parts[0].body).unwrap();
        match try_decode_complete_set(&dir, &path).unwrap() {
            AssembleOutcome::Joined(joined) => {
                assert_eq!(joined.name, "demo (2).bin");
                assert_eq!(fs::read(&joined.path).unwrap(), data);
                assert_eq!(fs::read(dir.join("demo.bin")).unwrap(), b"already here");
                assert!(!path.is_file());
            }
            other => panic!("expected join, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manual_7decode_lists_missing_and_joins() {
        let dir = std::env::temp_dir().join(format!("gpng-7dec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0u8..200).collect();
        let parts = sevenplus_codec::encode_parts("demo.bin", &data, 80).unwrap();
        assert!(parts.len() >= 3);
        fs::write(dir.join(&parts[0].name), &parts[0].body).unwrap();
        match decode_user_parts(&dir, "demo.bin").unwrap() {
            AssembleOutcome::Incomplete { missing } => {
                assert!(missing.contains(&parts[1].name));
            }
            other => panic!("expected missing, got {other:?}"),
        }
        for part in &parts {
            fs::write(dir.join(&part.name), &part.body).unwrap();
        }
        match decode_user_parts(&dir, &parts[0].name).unwrap() {
            AssembleOutcome::Joined(joined) => {
                assert_eq!(joined.name, "demo.bin");
                assert_eq!(fs::read(&joined.path).unwrap(), data);
            }
            other => panic!("expected join, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_part_reports_missing_earlier_parts() {
        let dir = std::env::temp_dir().join(format!("gpng-miss-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0u8..200).collect();
        let parts = sevenplus_codec::encode_parts("demo.bin", &data, 80).unwrap();
        assert!(parts.len() >= 3);
        // Save part 1 and the last part; skip part 2.
        fs::write(dir.join(&parts[0].name), &parts[0].body).unwrap();
        let last = parts.last().unwrap();
        let last_path = dir.join(&last.name);
        fs::write(&last_path, &last.body).unwrap();
        match try_decode_complete_set(&dir, &last_path).unwrap() {
            AssembleOutcome::Incomplete { missing } => {
                assert!(
                    missing.iter().any(|m| m == &parts[1].name),
                    "missing={missing:?} expected {}",
                    parts[1].name
                );
                assert!(!missing.iter().any(|m| m == &parts[0].name));
            }
            other => panic!("expected missing list, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
