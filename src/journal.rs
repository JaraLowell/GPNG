//! Station journal. One `<base call>.GPL` per base call, not per SSID.
//!
//! The clock is the PC clock plus `UTC` minutes (`UTC = -120` means the
//! log is two hours behind the PC). `QRG` is at most 7 characters.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local};

use crate::config::normalize_mycall;

#[derive(Debug, Clone)]
pub struct OpenQso {
    pub start: DateTime<Local>,
    pub peer: String,
    pub mycall: String,
}

/// PC local time shifted by the GP `UTC` offset (minutes).
pub fn clock(offset_min: i32) -> DateTime<Local> {
    Local::now() + Duration::minutes(i64::from(offset_min))
}

/// `NL0MSK-2` → `NL0MSK`. A call that does not parse is used as typed.
pub fn base_call(call: &str) -> String {
    let call = normalize_mycall(call).unwrap_or_else(|_| call.trim().to_ascii_uppercase());
    call.split_once('-')
        .map(|(base, _)| base.to_string())
        .unwrap_or(call)
}

pub fn gpl_path(log_dir: &Path, mycall: &str) -> PathBuf {
    log_dir.join(format!("{}.GPL", base_call(mycall)))
}

pub fn clip_qrg(text: &str) -> String {
    text.trim().chars().take(7).collect()
}

pub fn format_line(open: &OpenQso, end: DateTime<Local>, qrg: &str) -> String {
    let secs = end.signed_duration_since(open.start).num_seconds().max(0);
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let peer: String = open.peer.chars().take(9).collect();
    format!(
        "{date} {time} {peer:<9} {qrg:<7} {hours:02}:{mins:02}\r\n",
        date = open.start.format("%d.%m.%y"),
        time = open.start.format("%H:%M"),
        qrg = clip_qrg(qrg),
    )
}

pub fn append(path: &Path, open: &OpenQso, end: DateTime<Local>, qrg: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(format_line(open, end, qrg).as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn base_call_drops_ssid() {
        assert_eq!(base_call("nl0msk-2"), "NL0MSK");
        assert_eq!(base_call("NL0MSK"), "NL0MSK");
    }

    #[test]
    fn line_is_date_time_call_qrg_duration() {
        let start = Local
            .with_ymd_and_hms(2026, 11, 4, 11, 16, 0)
            .single()
            .unwrap();
        let end = start + Duration::minutes(12);
        let open = OpenQso {
            start,
            peer: "NL1AS".into(),
            mycall: "NL0MSK-2".into(),
        };
        let line = format_line(&open, end, "430.625");
        assert_eq!(line, "04.11.26 11:16 NL1AS     430.625 00:12\r\n");
    }

    #[test]
    fn qrg_is_at_most_seven_characters() {
        assert_eq!(clip_qrg("430.6250"), "430.625");
        assert_eq!(clip_qrg("70cm"), "70cm");
    }
}
