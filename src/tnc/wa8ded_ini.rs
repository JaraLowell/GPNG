//! WA8DED.INI — open / status / close hostmode command sections.
//!
//! Same idea as KISS.INI, but each non-comment line is one channel-0 hostmode
//! payload (see your Python `ser.write(b'\x00\x01…')` sequences).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::hostmode::{self, HostCode, sync_datetime, write_host_frame};
use super::serial::SerialLink;
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
    link: &mut SerialLink,
    path: &Path,
    section: &str,
    line_delay_ms: u64,
    monitor_mode: &str,
    events: Option<&mpsc::Sender<TncEvent>>,
) -> Result<Vec<(String, String)>, TncError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        TncError::Protocol(format!("wa8ded ini {}: {e}", path.display()))
    })?;
    let lines = extract_section(&text, section).ok_or_else(|| {
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
    let mut stream = Vec::new();

    for raw in &lines {
        let cmd = expand_placeholders(raw.trim(), monitor_mode);
        if cmd.is_empty() {
            continue;
        }
        let _ = events.and_then(|e| {
            e.blocking_send(TncEvent::Status(format!("wa8ded TX › {cmd}")))
                .ok()
        });

        if cmd.eq_ignore_ascii_case("DateTime") {
            sync_datetime(link)?;
        } else if cmd.eq_ignore_ascii_case("JHOST0") {
            // Hostmode leave: frame + ASCII fallback (classic GP).
            let _ = write_host_frame(link, 0, b"JHOST0");
            let _ = link.write_terminal("JHOST0");
        } else {
            write_host_frame(link, 0, cmd.as_bytes())?;
        }

        let reply = wait_reply(link, &mut stream, events, line_delay_ms);
        if let Some(text) = reply {
            let _ = events.and_then(|e| {
                e.blocking_send(TncEvent::Status(format!("wa8ded RX ‹ {text}")))
                    .ok()
            });
            replies.push((cmd.to_string(), text));
        } else {
            let _ = events.and_then(|e| {
                e.blocking_send(TncEvent::Status(
                    "wa8ded RX ‹ (no reply)".into(),
                ))
                .ok()
            });
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
            .find(|l| l.to_ascii_uppercase().starts_with(&prefix.to_ascii_uppercase()))
            .map(|l| {
                l.split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string()
            })
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

    format!(
        "Timing-Parameters of {mycall}:\r\n\
         P-Persistance: {p:<9}Slottime: {w}\r\n\
         Frack: {frac}\r\n\
         T2-Timer: {t2:<14}T3-Timer: {t3}\r\n\
         TXDelay: {t:<15}MaxUnack: {o}"
    )
}

fn wait_reply(
    link: &mut SerialLink,
    stream: &mut Vec<u8>,
    events: Option<&mpsc::Sender<TncEvent>>,
    wait_ms: u64,
) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(wait_ms.max(50));
    let mut buf = [0u8; 256];
    let mut got: Option<String> = None;

    while Instant::now() < deadline {
        match link.try_read(&mut buf) {
            Ok(0) => std::thread::sleep(Duration::from_millis(15)),
            Ok(n) => {
                let chunk = &buf[..n];
                if let Some(e) = events {
                    let _ = e.blocking_send(TncEvent::RawRx(chunk.to_vec()));
                }
                stream.extend_from_slice(chunk);
                // Pull frames; keep last Data/Ok payload as the "readline" text.
                loop {
                    if stream.len() < 3 {
                        break;
                    }
                    let len = stream[2] as usize;
                    if stream.len() < 3 + len {
                        break;
                    }
                    let code = HostCode::from_u8(stream[1]);
                    let payload = stream[3..3 + len].to_vec();
                    stream.drain(..3 + len);
                    if let Some(e) = events {
                        let _ = e.blocking_send(TncEvent::HostFrame {
                            channel: 0,
                            code,
                            payload: payload.clone(),
                        });
                    }
                    if matches!(code, HostCode::Data | HostCode::Ok | HostCode::Other(_))
                        && !payload.is_empty()
                    {
                        let s = String::from_utf8_lossy(&payload)
                            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                            .to_string();
                        if !s.is_empty() {
                            got = Some(s);
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    // Prefer a successful reply; Ok with empty payload is common for SETs.
    got
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

#[allow(dead_code)]
pub fn apply_command_list_fallback(
    link: &mut SerialLink,
    cmds: &[String],
) -> Result<(), TncError> {
    hostmode::apply_command_list(link, cmds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_open_and_close() {
        let text = "\
[open]
A
B
[status]
F
[close]
MN
JHOST0
";
        assert_eq!(extract_section(text, "open").unwrap(), vec!["A", "B"]);
        assert_eq!(extract_section(text, "close").unwrap(), vec!["MN", "JHOST0"]);
    }
}
