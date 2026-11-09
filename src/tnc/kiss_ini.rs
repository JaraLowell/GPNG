//! KISSINIT-compatible `KISS.INI` loader (DG0FT format).
//!
//! Escapes (from `KISSINIT.doc`):
//! - `\nnn` decimal byte, `\e` ESC, `\k` KISS reset (`C0 FF C0`), `\d` 1s pause
//! - `^X` control character, `\\` literal `\`, trailing `\` suppresses CR

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::serial::SerialLink;
use super::{shutdown_requested, TncError, TncEvent};

/// Sleep in short slices so a pending app shutdown aborts KISS.INI promptly.
fn pause(ms: u64) -> Result<(), TncError> {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    loop {
        if shutdown_requested() {
            return Err(TncError::Protocol("kiss init aborted (shutdown)".into()));
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok(());
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(20)));
    }
}

#[derive(Debug, Clone)]
enum Action {
    Byte(u8),
    DelayMs(u64),
}

/// Resolve `KISS.INI` from the configured path or `data/`.
pub fn resolve_kiss_ini(configured: &str) -> PathBuf {
    crate::paths::resolve_configured(configured)
}

/// Expand `%M` → `monitor_mode` (classic GP monitor flags, e.g. IUSC).
pub fn expand_placeholders(line: &str, monitor_mode: &str) -> String {
    let mode = if monitor_mode.is_empty() {
        "IUSC"
    } else {
        monitor_mode
    };
    line.replace("%M", mode).replace("%m", mode)
}

/// Load one `[section]` from a KISSINIT file and send it on `link`.
///
/// Emits Monitor-visible TX lines and any UART echo (proves ECHO ON / baud).
pub fn apply_section(
    link: &mut SerialLink,
    path: &Path,
    section: &str,
    char_delay_ms: u64,
    line_delay_ms: u64,
    exit_delay_ms: u64,
    monitor_mode: &str,
    events: Option<&mpsc::Sender<TncEvent>>,
    paclen: Option<u16>,
) -> Result<(), TncError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| TncError::Protocol(format!("kiss ini {}: {e}", path.display())))?;
    let lines = extract_section(&text, section).ok_or_else(|| {
        TncError::Protocol(format!(
            "kiss ini {}: section [{section}] not found",
            path.display()
        ))
    })?;
    let lines = splice_paclen_command(&lines, paclen);

    info!(
        file = %path.display(),
        section,
        lines = lines.len(),
        "applying KISS.INI"
    );

    for raw in &lines {
        let raw = expand_placeholders(raw, monitor_mode);
        let _ = events.and_then(|e| {
            e.blocking_send(TncEvent::Status(format!("kiss TX › {raw}")))
                .ok()
        });
        let (actions, send_cr) = expand_line(&raw);
        for act in actions {
            match act {
                Action::Byte(b) => {
                    link.write_all(&[b])?;
                    if char_delay_ms > 0 {
                        pause(char_delay_ms)?;
                    }
                }
                Action::DelayMs(ms) => {
                    pause(ms)?;
                }
            }
        }
        if send_cr {
            link.write_all(&[b'\r'])?;
            if line_delay_ms > 0 {
                pause(line_delay_ms)?;
            }
        }
        // Collect echo / TNC reply after this line (ECHO ON should mirror text).
        let echo = read_available(link, 200);
        if !echo.is_empty() {
            let preview = String::from_utf8_lossy(&echo);
            let printable: String = preview
                .chars()
                .map(|c| {
                    if c == '\r' {
                        '↵'
                    } else if c == '\n' {
                        '↓'
                    } else if c.is_control() {
                        '.'
                    } else {
                        c
                    }
                })
                .collect();
            let _ = events.and_then(|e| {
                e.blocking_send(TncEvent::Status(format!("kiss RX ‹ {printable}")))
                    .ok()
            });
            let _ = events.and_then(|e| e.blocking_send(TncEvent::RawRx(echo)).ok());
        } else {
            let _ = events.and_then(|e| {
                e.blocking_send(TncEvent::Status(
                    "kiss RX ‹ (no echo — check baud / PACKET12 / ECHO)".into(),
                ))
                .ok()
            });
        }
        debug!(line = %raw, "kiss ini line sent");
    }

    if exit_delay_ms > 0 {
        info!(exit_delay_ms, "kiss ini settle");
        let _ = events.and_then(|e| {
            e.blocking_send(TncEvent::Status(format!(
                "kiss settle {exit_delay_ms} ms after RESTART…"
            )))
            .ok()
        });
        pause(exit_delay_ms)?;
    }
    Ok(())
}

fn read_available(link: &mut SerialLink, wait_ms: u64) -> Vec<u8> {
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms);
    let mut out = Vec::new();
    let mut buf = [0u8; 128];
    while std::time::Instant::now() < deadline {
        match link.try_read(&mut buf) {
            Ok(0) => std::thread::sleep(Duration::from_millis(20)),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    out
}

fn extract_section(text: &str, section: &str) -> Option<Vec<String>> {
    let want = section.trim().to_ascii_lowercase();
    let mut in_section = want.is_empty();
    let mut out = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if in_section && !want.is_empty() {
                break; // next section ends us
            }
            let name = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            in_section = want.is_empty() || name == want;
            continue;
        }
        if !in_section {
            continue;
        }
        // Full-line comment
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        // Strip inline `; comment`
        let code = match trimmed.find(';') {
            Some(i) => trimmed[..i].trim_end(),
            None => trimmed.trim_end(),
        };
        if !code.is_empty() {
            out.push(code.to_string());
        }
    }

    if out.is_empty() && !want.is_empty() {
        // Section missing or empty
        return None;
    }
    if out.is_empty() {
        warn!("kiss ini section empty");
    }
    Some(out)
}

/// On-air packet speed from `HB` / `HBAUD` in `section`. 1200 when unset.
pub fn section_hbaud(text: &str, section: &str) -> u32 {
    let Some(lines) = extract_section(text, section) else {
        return 1200;
    };
    for line in lines.iter().rev() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("").to_ascii_uppercase();
        if key == "HB" || key == "HBAUD" {
            if let Some(n) = parts.next().and_then(|s| s.parse::<u32>().ok()) {
                if (300..=115_200).contains(&n) {
                    return n;
                }
            }
        }
    }
    1200
}

/// Read [`section_hbaud`] from a KISS.INI file. Missing file → 1200.
pub fn hbaud_from_file(path: &Path, section: &str) -> u32 {
    match std::fs::read_to_string(path) {
        Ok(text) => section_hbaud(&text, section),
        Err(_) => 1200,
    }
}

/// One row of `KISS.INI` `[params]`: Esc letter, TAPR command, current arg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KissParam {
    pub letter: char,
    pub cmd: u8,
    pub value: u8,
    pub name: String,
}

impl KissParam {
    /// Legacy T/P/W mapping when `[params]` is absent. `None` for other cmds.
    pub fn known(cmd: u8, value: u8) -> Option<Self> {
        let (letter, name) = match cmd {
            1 => ('T', "TXDELAY"),
            2 => ('P', "PERSIST"),
            3 => ('W', "SLOTTIME"),
            4 => ('Y', "TXTAIL"),
            5 => ('D', "FULLDUP"),
            _ => return None,
        };
        Some(Self {
            letter,
            cmd,
            value,
            name: name.to_string(),
        })
    }
}

/// TAPR name for a command nibble. Unknown cmds stay `PARAM`.
pub fn cmd_label(cmd: u8) -> &'static str {
    match cmd {
        1 => "TXDELAY",
        2 => "PERSIST",
        3 => "SLOTTIME",
        4 => "TXTAIL",
        5 => "FULLDUP",
        _ => "PARAM",
    }
}

/// `Letter <value>` lines so the timing banner can fill T / P / W.
pub fn open_lines(params: &[KissParam]) -> Vec<String> {
    params
        .iter()
        .map(|p| format!("{} {}", p.letter, p.value))
        .collect()
}

/// What an Esc TNC line means for KISS `[params]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscKiss {
    /// Not a single letter, or not a letter plus a number (`M IUSC`, `C CQ`).
    Ignore,
    /// A number was there, and it does not fit in the KISS arg byte (or PACLEN).
    BadValue,
    /// `W` with no number — show the stored value. KISS cannot read it back.
    Show(char),
    /// `W 30` / `W30` — send that arg and store it.
    Set { letter: char, value: u8 },
    /// `PACLEN` with no number — show the stored info size.
    ShowPaclen,
    /// `PACLEN 128`. `0` means 256 bytes. `256` is stored as 256 and sent as `PACLEN 0`.
    SetPaclen(u16),
}

/// Parse an Esc TNC line into a `[params]` letter and optional new value.
pub fn parse_esc(line: &str) -> EscKiss {
    let line = line.trim();
    let mut words = line.split_whitespace();
    if words
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("PACLEN"))
    {
        return match words.next() {
            None => EscKiss::ShowPaclen,
            Some(arg) if words.next().is_none() => match arg.parse::<u16>() {
                Ok(n) if n <= 256 => EscKiss::SetPaclen(n),
                _ => EscKiss::BadValue,
            },
            Some(_) => EscKiss::Ignore,
        };
    }
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return EscKiss::Ignore;
    };
    if !first.is_ascii_alphabetic() {
        return EscKiss::Ignore;
    }
    let letter = first.to_ascii_uppercase();
    let rest = chars.as_str().trim();
    if rest.is_empty() {
        return EscKiss::Show(letter);
    }
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return EscKiss::Ignore;
    }
    match rest.parse::<u16>() {
        Ok(n) if n <= 255 => EscKiss::Set {
            letter,
            value: n as u8,
        },
        _ => EscKiss::BadValue,
    }
}

/// `[params]` from `text`. `None` when the section header is absent.
pub fn parse_params_section(text: &str) -> Option<Vec<KissParam>> {
    let mut in_section = false;
    let mut found = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section {
                break;
            }
            let name = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim();
            in_section = name.eq_ignore_ascii_case("params");
            if in_section {
                found = true;
            }
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        let code = match trimmed.find(';') {
            Some(i) => trimmed[..i].trim(),
            None => trimmed,
        };
        if paclen_on_line(code).is_some() {
            continue;
        }
        if let Some(param) = parse_param_line(code) {
            out.push(param);
        } else if !code.is_empty() {
            warn!(line = code, "kiss [params] line ignored");
        }
    }
    if found {
        Some(out)
    } else {
        None
    }
}

fn parse_param_line(line: &str) -> Option<KissParam> {
    let mut parts = line.split_whitespace();
    let letter_tok = parts.next()?;
    let mut letters = letter_tok.chars();
    let letter = letters.next()?;
    if letters.next().is_some() || !letter.is_ascii_alphabetic() {
        return None;
    }
    let cmd: u8 = parts.next()?.parse().ok()?;
    if !(1..=5).contains(&cmd) {
        return None;
    }
    let value: u8 = parts.next()?.parse().ok()?;
    let name = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    Some(KissParam {
        letter: letter.to_ascii_uppercase(),
        cmd,
        value,
        name: name.unwrap_or_else(|| cmd_label(cmd).to_string()),
    })
}

/// Info-field size in bytes. A TNC `PACLEN 0` means 256.
pub fn payload_len(paclen: u16) -> u32 {
    if paclen == 0 {
        256
    } else {
        u32::from(paclen.min(256))
    }
}

/// Argument for the command-mode line. 256 bytes is `PACLEN 0` on TNC2 / Kenwood.
pub fn paclen_command_arg(paclen: u16) -> u16 {
    if paclen == 0 || paclen >= 256 {
        0
    } else {
        paclen
    }
}

fn paclen_on_line(line: &str) -> Option<u16> {
    let mut parts = line.split_whitespace();
    let key = parts.next()?;
    if !key.eq_ignore_ascii_case("PACLEN") {
        return None;
    }
    let n: u16 = parts.next()?.parse().ok()?;
    if n > 256 || parts.next().is_some() {
        return None;
    }
    Some(n)
}

/// `PACLEN` from `[params]`. `None` when the section or the line is missing.
pub fn paclen_in_params(text: &str) -> Option<u16> {
    let mut in_section = false;
    let mut found = false;
    let mut value = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section {
                break;
            }
            let name = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim();
            in_section = name.eq_ignore_ascii_case("params");
            if in_section {
                found = true;
            }
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        let code = match trimmed.find(';') {
            Some(i) => trimmed[..i].trim(),
            None => trimmed,
        };
        if let Some(n) = paclen_on_line(code) {
            value = Some(n);
        }
    }
    if found {
        value
    } else {
        None
    }
}

pub fn load_paclen(path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    paclen_in_params(&text)
}

/// While KISS is the selected protocol, `[buffers] packet_payload` follows PACLEN.
pub fn apply_configured_paclen(cfg: &mut crate::config::Config) {
    if cfg.tnc.protocol_kind() != crate::config::HostProtocol::Kiss {
        return;
    }
    let path = resolve_kiss_ini(&cfg.tnc.kiss_ini);
    if let Some(n) = load_paclen(&path) {
        cfg.buffers.packet_payload = payload_len(n);
    }
}

/// Put `PACLEN n` in a command-mode script, just before `KISS ON`.
/// An existing `PACLEN` line is replaced. `None` leaves the script alone.
pub fn splice_paclen_command(lines: &[String], paclen: Option<u16>) -> Vec<String> {
    let Some(n) = paclen else {
        return lines.to_vec();
    };
    let cmd = format!("PACLEN {}", paclen_command_arg(n));
    let mut out = Vec::with_capacity(lines.len() + 1);
    let mut wrote = false;
    for line in lines {
        let key = line.split_whitespace().next().unwrap_or("");
        if key.eq_ignore_ascii_case("PACLEN") {
            if !wrote {
                out.push(cmd.clone());
                wrote = true;
            }
            continue;
        }
        if !wrote && key.eq_ignore_ascii_case("KISS") {
            out.push(cmd.clone());
            wrote = true;
        }
        out.push(line.clone());
    }
    if !wrote {
        out.push(cmd);
    }
    out
}

pub fn store_paclen(path: &Path, value: u16) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = rewrite_paclen(&text, value)?;
    std::fs::write(path, updated).map_err(|e| format!("write {}: {e}", path.display()))
}

pub fn rewrite_paclen(text: &str, value: u16) -> Result<String, String> {
    if value > 256 {
        return Err("PACLEN is a number from 0 to 256".into());
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_nl = text.ends_with('\n');
    let mut in_section = false;
    let mut found_section = false;
    let mut replaced = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section && !replaced {
                out.push(format!("PACLEN {value}"));
                replaced = true;
            }
            if in_section {
                in_section = false;
            } else {
                let name = trimmed
                    .trim_start_matches('[')
                    .split(']')
                    .next()
                    .unwrap_or("")
                    .trim();
                in_section = name.eq_ignore_ascii_case("params");
                if in_section {
                    found_section = true;
                }
            }
            out.push(line.to_string());
            continue;
        }
        let code = match trimmed.find(';') {
            Some(i) => trimmed[..i].trim(),
            None => trimmed,
        };
        if in_section && !replaced && paclen_on_line(code).is_some() {
            if let Some(rewritten) = replace_second_token(line, &value.to_string()) {
                out.push(rewritten);
                replaced = true;
                continue;
            }
        }
        out.push(line.to_string());
    }
    if in_section && !replaced {
        out.push(format!("PACLEN {value}"));
        replaced = true;
    }
    if !found_section {
        return Err("KISS.INI has no [params] section".into());
    }
    if !replaced {
        return Err("KISS.INI [params] has no PACLEN".into());
    }
    let mut joined = out.join(newline);
    if ends_nl && !joined.ends_with('\n') {
        joined.push_str(newline);
    }
    Ok(joined)
}

fn replace_second_token(line: &str, new: &str) -> Option<String> {
    let comment_at = line.find(';');
    let code_end = comment_at.unwrap_or(line.len());
    let code = &line[..code_end];
    let bytes = code.as_bytes();
    let mut idx = 0;
    let mut token = 0;
    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }
        let start = idx;
        while idx < bytes.len() && !bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if token == 1 {
            let mut updated = String::new();
            updated.push_str(&line[..start]);
            updated.push_str(new);
            updated.push_str(&line[idx..]);
            return Some(updated);
        }
        token += 1;
    }
    None
}

/// Load `[params]`. `None` if the file or the section is missing.
pub fn load_params(path: &Path) -> Option<Vec<KissParam>> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_params_section(&text)
}

/// Replace the stored arg for `letter` inside `[params]`. The rest of the
/// file, including comments, stays as it was.
pub fn store_param(path: &Path, letter: char, value: u8) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = rewrite_param_value(&text, letter, value)?;
    std::fs::write(path, updated).map_err(|e| format!("write {}: {e}", path.display()))
}

pub fn rewrite_param_value(text: &str, letter: char, value: u8) -> Result<String, String> {
    let letter = letter.to_ascii_uppercase();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_nl = text.ends_with('\n');
    let mut in_section = false;
    let mut found_section = false;
    let mut replaced = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if !replaced {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                if in_section {
                    in_section = false;
                } else {
                    let name = trimmed
                        .trim_start_matches('[')
                        .split(']')
                        .next()
                        .unwrap_or("")
                        .trim();
                    in_section = name.eq_ignore_ascii_case("params");
                    if in_section {
                        found_section = true;
                    }
                }
            } else if in_section && !trimmed.is_empty() && !trimmed.starts_with(';') {
                if let Some(rewritten) = rewrite_param_line(line, letter, value) {
                    out.push(rewritten);
                    replaced = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if !found_section {
        return Err("KISS.INI has no [params] section".into());
    }
    if !replaced {
        return Err(format!("KISS.INI [params] has no Esc {letter}"));
    }
    let mut joined = out.join(newline);
    if ends_nl && !joined.ends_with('\n') {
        joined.push_str(newline);
    }
    Ok(joined)
}

/// Replace the third field when the first field is `letter`. `None` if this
/// line is a different parameter.
fn rewrite_param_line(line: &str, letter: char, value: u8) -> Option<String> {
    let comment_at = line.find(';');
    let code_end = comment_at.unwrap_or(line.len());
    let code = &line[..code_end];
    let bytes = code.as_bytes();
    let mut idx = 0;
    let mut token = 0;
    let mut first: Option<String> = None;
    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }
        let start = idx;
        while idx < bytes.len() && !bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if token == 0 {
            first = Some(code[start..idx].to_string());
        }
        if token == 2 {
            let got = first.as_deref()?.chars().next()?;
            if first.as_deref()?.chars().nth(1).is_some() || !got.eq_ignore_ascii_case(&letter) {
                return None;
            }
            let mut updated = String::new();
            updated.push_str(&line[..start]);
            updated.push_str(&value.to_string());
            updated.push_str(&line[idx..]);
            return Some(updated);
        }
        token += 1;
    }
    None
}

/// Expand one KISS.INI command line into paced actions + whether to append CR.
fn expand_line(line: &str) -> (Vec<Action>, bool) {
    let mut actions = Vec::new();
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut suppress_cr = false;

    while i < bytes.len() {
        match bytes[i] {
            '\\' => {
                i += 1;
                if i >= bytes.len() {
                    // Trailing `\` → no CR
                    suppress_cr = true;
                    break;
                }
                let c = bytes[i];
                match c.to_ascii_lowercase() {
                    'e' => actions.push(Action::Byte(0x1b)),
                    'k' => {
                        // KISS reset: FEND 0xFF FEND
                        actions.push(Action::Byte(0xc0));
                        actions.push(Action::Byte(0xff));
                        actions.push(Action::Byte(0xc0));
                    }
                    'd' => actions.push(Action::DelayMs(1000)),
                    '0'..='9' => {
                        let mut n = 0u16;
                        let mut digits = 0;
                        while i < bytes.len() && bytes[i].is_ascii_digit() && digits < 3 {
                            n = n * 10 + (bytes[i] as u8 - b'0') as u16;
                            i += 1;
                            digits += 1;
                        }
                        i -= 1; // loop will +1
                        actions.push(Action::Byte((n.min(255)) as u8));
                    }
                    other => actions.push(Action::Byte(other as u8)),
                }
                i += 1;
            }
            '^' => {
                i += 1;
                if i < bytes.len() {
                    let c = bytes[i].to_ascii_uppercase();
                    if c.is_ascii_uppercase() {
                        actions.push(Action::Byte((c as u8) & 0x1f));
                    }
                    i += 1;
                }
            }
            ch => {
                actions.push(Action::Byte(ch as u8));
                i += 1;
            }
        }
    }

    (actions, !suppress_cr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_kiss_reset_and_delay() {
        let (acts, cr) = expand_line(r"\k\d");
        assert!(cr);
        assert!(matches!(acts[0], Action::Byte(0xc0)));
        assert!(matches!(acts[1], Action::Byte(0xff)));
        assert!(matches!(acts[2], Action::Byte(0xc0)));
        assert!(matches!(acts[3], Action::DelayMs(1000)));
    }

    #[test]
    fn hbaud_comes_from_the_active_section() {
        let text = "\
[TM-D710]
HB 1200
KISS ON
[TM-D710_9600pkt]
HB 9600
";
        assert_eq!(section_hbaud(text, "TM-D710"), 1200);
        assert_eq!(section_hbaud(text, "TM-D710_9600pkt"), 9600);
        assert_eq!(section_hbaud(text, "missing"), 1200);
    }

    #[test]
    fn extracts_section() {
        let text = "\
[A]
ONE
[B]
TWO
THREE
[C]
FOUR
";
        let b = extract_section(text, "B").unwrap();
        assert_eq!(b, vec!["TWO", "THREE"]);
    }

    #[test]
    fn trailing_backslash_suppresses_cr() {
        let (acts, cr) = expand_line(r"^X\e@K\");
        assert!(!cr);
        assert!(matches!(acts[0], Action::Byte(0x18)));
        assert!(matches!(acts[1], Action::Byte(0x1b)));
    }

    #[test]
    fn params_section_maps_esc_letters_to_kiss_cmds() {
        let text = "\
; note
[params]
; cmd 1 is TXDELAY
T 1 50 TXDELAY
P 2 160 PERSIST
W 3 30 SLOTTIME   ; slot
[close]
\\k
";
        let params = parse_params_section(text).unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(params[0].letter, 'T');
        assert_eq!(params[0].cmd, 1);
        assert_eq!(params[0].value, 50);
        assert_eq!(params[2].letter, 'W');
        assert_eq!(params[2].cmd, 3);
        assert_eq!(params[2].value, 30);
        assert_eq!(
            open_lines(&params),
            vec!["T 50".to_string(), "P 160".into(), "W 30".into()]
        );
        assert!(parse_params_section("[close]\n\\k\n").is_none());
    }

    #[test]
    fn esc_line_parses_letter_and_number() {
        assert_eq!(
            parse_esc("W 30"),
            EscKiss::Set {
                letter: 'W',
                value: 30
            }
        );
        assert_eq!(
            parse_esc("p160"),
            EscKiss::Set {
                letter: 'P',
                value: 160
            }
        );
        assert_eq!(parse_esc("T"), EscKiss::Show('T'));
        assert_eq!(parse_esc("W 300"), EscKiss::BadValue);
        assert_eq!(parse_esc("M IUSC"), EscKiss::Ignore);
        assert_eq!(parse_esc("C CQ"), EscKiss::Ignore);
        assert_eq!(parse_esc("PACLEN"), EscKiss::ShowPaclen);
        assert_eq!(parse_esc("paclen 200"), EscKiss::SetPaclen(200));
        assert_eq!(parse_esc("PACLEN 0"), EscKiss::SetPaclen(0));
        assert_eq!(parse_esc("PACLEN 256"), EscKiss::SetPaclen(256));
        assert_eq!(parse_esc("PACLEN 300"), EscKiss::BadValue);
    }

    #[test]
    fn paclen_is_payload_not_a_kiss_frame() {
        let text = "\
[params]
T 1 50 TXDELAY
PACLEN 128
W 3 3 SLOTTIME
";
        let params = parse_params_section(text).unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(paclen_in_params(text), Some(128));
        assert_eq!(payload_len(0), 256);
        assert_eq!(payload_len(128), 128);
        assert_eq!(paclen_command_arg(256), 0);
        assert_eq!(paclen_command_arg(0), 0);
        assert_eq!(paclen_command_arg(128), 128);
        let script = splice_paclen_command(
            &["HB 1200".into(), "KISS ON".into(), "RESTART".into()],
            Some(256),
        );
        assert_eq!(
            script,
            vec![
                "HB 1200".to_string(),
                "PACLEN 0".into(),
                "KISS ON".into(),
                "RESTART".into()
            ]
        );
        let replaced = splice_paclen_command(&["PACLEN 64".into(), "KISS ON".into()], Some(128));
        assert_eq!(replaced, vec!["PACLEN 128".to_string(), "KISS ON".into()]);
    }

    #[test]
    fn rewrite_paclen_keeps_the_other_lines() {
        let text = "\
[params]
T 1 50 TXDELAY
PACLEN 128 ; info bytes
W 3 3 SLOTTIME
[close]
\\k
";
        let updated = rewrite_paclen(text, 200).unwrap();
        assert!(updated.contains("PACLEN 200 ; info bytes"));
        assert!(updated.contains("T 1 50 TXDELAY"));
        assert!(updated.contains("W 3 3 SLOTTIME"));
        assert_eq!(paclen_in_params(&updated), Some(200));
    }

    #[test]
    fn rewrite_keeps_comments_and_other_sections() {
        let text = "\
[TM-D710]
HB 1200
[params]
T 1 50 TXDELAY
W 3 30 SLOTTIME   ; 10 ms units
[close]
\\k
";
        let updated = rewrite_param_value(text, 'w', 12).unwrap();
        assert!(updated.contains("T 1 50 TXDELAY"));
        assert!(updated.contains("W 3 12 SLOTTIME   ; 10 ms units"));
        assert!(updated.contains("[TM-D710]"));
        assert!(updated.contains("HB 1200"));
        assert!(rewrite_param_value(text, 'O', 4).is_err());
    }
}
