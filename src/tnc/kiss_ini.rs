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
) -> Result<(), TncError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        TncError::Protocol(format!("kiss ini {}: {e}", path.display()))
    })?;
    let lines = extract_section(&text, section).ok_or_else(|| {
        TncError::Protocol(format!(
            "kiss ini {}: section [{section}] not found",
            path.display()
        ))
    })?;

    info!(
        file = %path.display(),
        section,
        lines = lines.len(),
        "applying KISS.INI"
    );

    for raw in &lines {
        let raw = expand_placeholders(raw, monitor_mode);
        let _ = events.and_then(|e| {
            e.blocking_send(TncEvent::Status(format!("kiss TX › {raw}"))).ok()
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
}
