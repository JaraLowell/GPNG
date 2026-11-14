//! WA8DED.INI — open / status / close hostmode command sections.
//!
//! Same idea as KISS.INI, but each non-comment line is one channel-0 hostmode
//! payload (see your Python `ser.write(b'\x00\x01…')` sequences).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::hostmode::{exchange, expand_command, HostCode, HostIo};
use super::{TncError, TncEvent};

pub fn resolve_wa8ded_ini(configured: &str) -> PathBuf {
    crate::paths::resolve_configured(configured)
}

/// Expand `%M` → monitor flags from Config (`monitor_mode`, default IUSC).
pub fn expand_placeholders(line: &str, monitor_mode: &str) -> String {
    let mode = if monitor_mode.is_empty() {
        "IUSC"
    } else {
        monitor_mode
    };
    line.replace("%M", mode).replace("%m", mode)
}

/// Apply `[open]`, `[status]`, or `[close]` from WA8DED.INI.
///
/// Returns collected reply payloads for `[status]` queries (F / @T2 / @T3…).
pub fn apply_section(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    path: &Path,
    section: &str,
    line_delay_ms: u64,
    monitor_mode: &str,
    events: Option<&mpsc::Sender<TncEvent>>,
) -> Result<Vec<(String, String)>, TncError> {
    let lines = read_section(path, section)?.ok_or_else(|| {
        TncError::Protocol(format!(
            "wa8ded ini {}: section [{section}] not found",
            path.display()
        ))
    })?;

    info!(
        file = %path.display(),
        section,
        lines = lines.len(),
        "applying WA8DED.INI"
    );
    let _ = events.and_then(|e| {
        e.blocking_send(TncEvent::Status(format!(
            "wa8ded [{}] {} ({} lines)",
            section,
            path.display(),
            lines.len()
        )))
        .ok()
    });

    let mut replies = Vec::new();

    for raw in &lines {
        let cmd = expand_placeholders(raw.trim(), monitor_mode);
        if cmd.is_empty() {
            continue;
        }
        let _ = events.and_then(|e| {
            e.blocking_send(TncEvent::Status(format!("wa8ded TX › {cmd}")))
                .ok()
        });

        for wire in expand_command(&cmd) {
            let reply = exchange(link, stream, 0, true, wire.as_bytes(), events)?;
            let text = String::from_utf8_lossy(&reply.payload).into_owned();
            if !matches!(reply.code, HostCode::Ok | HostCode::CommandReply) {
                return Err(TncError::Protocol(format!(
                    "{wire}: {:?} {text}",
                    reply.code
                )));
            }
            let _ = events.and_then(|e| {
                e.blocking_send(TncEvent::Status(format!("wa8ded RX ‹ {text}")))
                    .ok()
            });
            replies.push((wire, text));
            if line_delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(line_delay_ms));
            }
        }
        debug!(cmd, "wa8ded ini line done");
    }

    Ok(replies)
}

/// Build the timing banner your Python printed after querying F / @T2 / @T3.
pub fn format_timing_banner(
    mycall: &str,
    replies: &[(String, String)],
    open_lines: &[String],
) -> String {
    let find = |key: &str| {
        replies
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_else(|| "?".into())
    };
    let from_open = |prefix: &str| {
        open_lines
            .iter()
            .find(|l| {
                l.to_ascii_uppercase()
                    .starts_with(&prefix.to_ascii_uppercase())
            })
            .map(|l| l.split_whitespace().nth(1).unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "?".into())
    };

    let frac = find("F");
    let t2 = find("@T2");
    let t3 = find("@T3");
    let p = from_open("P");
    let w = from_open("W");
    let t = from_open("T");
    let o = from_open("O");

    // On-air / pad text uses CR (classic GP / the Python remote dump).
    format!(
        "Timing-Parameters of {mycall}:\r\
         P-Persistance: {p:<9}Slottime: {w}\r\
         Frack: {frac}\r\
         T2-Timer: {t2:<14}T3-Timer: {t3}\r\
         TXDelay: {t:<15}MaxUnack: {o}"
    )
}

/// Banner from `[open]` (or `init[]`) alone — live F/@T2/@T3 show as `?`.
pub fn timing_banner_fallback(mycall: &str, open_or_init: &[String], monitor_mode: &str) -> String {
    let open: Vec<String> = open_or_init
        .iter()
        .map(|l| expand_placeholders(l, monitor_mode))
        .collect();
    format_timing_banner(mycall, &[], &open)
}

fn extract_section(text: &str, section: &str) -> Option<Vec<String>> {
    let want = section.trim().to_ascii_lowercase();
    let mut in_section = false;
    let mut out = Vec::new();
    let mut found = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if in_section {
                break;
            }
            let name = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            in_section = name == want;
            if in_section {
                found = true;
            }
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        let code = match trimmed.find(';') {
            Some(i) => trimmed[..i].trim_end(),
            None => trimmed.trim_end(),
        };
        if !code.is_empty() {
            out.push(code.to_string());
        }
    }

    if !found {
        return None;
    }
    if out.is_empty() {
        warn!("wa8ded ini section [{want}] empty");
    }
    Some(out)
}

/// Load section lines without sending (for banner field lookup).
pub fn section_lines(path: &Path, section: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    extract_section(&text, section).unwrap_or_default()
}

pub fn read_section(path: &Path, section: &str) -> Result<Option<Vec<String>>, TncError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| TncError::Protocol(format!("wa8ded ini {}: {e}", path.display())))?;
    Ok(extract_section(&text, section))
}
