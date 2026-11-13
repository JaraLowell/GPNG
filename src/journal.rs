//! Station journal. One `<base call>.GPL` per base call, not per SSID.
//!
//! The clock is the PC clock plus `UTC` minutes (`UTC = -120` means the
//! log is two hours behind the PC). A numeric QRG keeps up to four digits
//! after the decimal and drops a trailing zero (`149.1125`, `150.112`).

use std::fs::OpenOptions;
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

/// `149.1125` stays. `150.1120` becomes `150.112`. `150.112` is not padded.
/// More than four digits after the decimal is rejected. A non-numeric QRG
/// such as `70cm` is left for [`clip_qrg`].
pub fn normalize_qrg(raw: &str) -> Result<String, String> {
    let t = raw.trim().replace(',', ".");
    if t.is_empty() {
        return Err("invalid QRG".into());
    }
    let (whole, frac) = if let Some((whole, frac)) = t.split_once('.') {
        if frac.contains('.') {
            return Err(format!("invalid QRG '{t}'"));
        }
        (whole, frac)
    } else {
        (t.as_str(), "")
    };
    if whole.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
    {
        return Err(format!("invalid QRG '{t}'"));
    }
    if frac.len() > 4 {
        return Err("QRG has more than 4 digits after the decimal".into());
    }
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let frac = frac.trim_end_matches('0');
    let text = if frac.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{frac}")
    };
    let mhz: f64 = text.parse().map_err(|_| format!("invalid QRG '{t}'"))?;
    if !(0.0..=1000.0).contains(&mhz) {
        return Err("QRG out of range".into());
    }
    Ok(text)
}

/// Display for a QRG that is only stored as a number. Four places, then
/// the same trailing-zero rule as [`normalize_qrg`].
pub fn format_mhz(mhz: f64) -> String {
    if !mhz.is_finite() {
        return String::new();
    }
    let scaled = (mhz * 10_000.0).round();
    if !(0.0..=10_000_000.0).contains(&scaled) {
        return String::new();
    }
    let scaled = scaled as u64;
    let whole = scaled / 10_000;
    let frac = scaled % 10_000;
    if frac == 0 {
        return whole.to_string();
    }
    let mut frac_s = format!("{frac:04}");
    while frac_s.ends_with('0') {
        frac_s.pop();
    }
    format!("{whole}.{frac_s}")
}

pub fn clip_qrg(text: &str) -> String {
    let text = text.trim();
    if let Ok(norm) = normalize_qrg(text) {
        return norm;
    }
    text.chars().take(7).collect()
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
        crate::paths::ensure_dir(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(format_line(open, end, qrg).as_bytes())?;
    Ok(())
}
