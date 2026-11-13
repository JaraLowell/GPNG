//! GPI / CTEXT macro expansion — classic Graphic Packet `%` tokens.
//!
//! Documented set: fifteen macros in `README.TXT`, plus `%DIGI%`, `%Q`,
//! `%A` (sysop activity), and the config-only `%TFPCX200` keyword.
//! Formatting suffix `#NN` pads/truncates. Undocumented EXE extras `%P` /
//! `%*` are not implemented.

use std::collections::HashMap;
use std::time::Duration;

use chrono::Local;

/// Idle under this many seconds → `%A` reports the sysop at the keyboard.
pub const ACTIVITY_PRESENT_SECS: u64 = 5 * 60;

/// Runtime values available while expanding a `*.GPI` / `*.GPM` / prompt line.
#[derive(Debug, Clone, Default)]
pub struct MacroContext {
    pub version: String,
    pub peer_call: String,
    pub peer_name: String,
    pub my_call: String,
    pub channel: u8,
    pub time: String,
    pub date: String,
    pub zone: String,
    pub news_hint: String,
    pub origin_line: String,
    pub digi: String,
    /// QRG in MHz (session-bar / Config.toml), e.g. `27.235`.
    pub qrg: String,
    /// Preformatted `%A` text from [`activity_status`].
    pub activity: String,
    /// If true, `%?` should solicit a name (caller handles interaction).
    pub name_unknown: bool,
}

impl MacroContext {
    pub fn demo() -> Self {
        Self {
            version: crate::version::APP_VERSION.into(),
            peer_call: "N0PEER".into(),
            peer_name: "Peer".into(),
            my_call: "N0CALL".into(),
            channel: 1,
            time: "12:00:00".into(),
            date: "02.11.2026".into(),
            zone: "UTC".into(),
            news_hint: String::new(),
            origin_line: "73 de GP".into(),
            digi: String::new(),
            qrg: "27.235".into(),
            activity: activity_status(Duration::from_secs(30)),
            name_unknown: false,
        }
    }
}

/// `%A` / activity status from idle time since last sysop key/mouse input.
///
/// Under five minutes: present. Otherwise relative minutes / hours / days.
pub fn activity_status(elapsed: Duration) -> String {
    if elapsed.as_secs() < ACTIVITY_PRESENT_SECS {
        "Sysop is at Keyboard".into()
    } else {
        format!(
            "Sysop not at keyboard, last seen {} ago",
            activity_ago_phrase(elapsed)
        )
    }
}

fn activity_ago_phrase(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 3600 {
        let n = (secs / 60).max(1);
        format!("{n} minute{}", if n == 1 { "" } else { "s" })
    } else if secs < 86_400 {
        let n = secs / 3600;
        format!("{n} hour{}", if n == 1 { "" } else { "s" })
    } else {
        let n = secs / 86_400;
        format!("{n} day{}", if n == 1 { "" } else { "s" })
    }
}

/// Expand `%` macros in `input` using `ctx`.
///
/// Unknown `%X` sequences are left unchanged so operators notice gaps.
pub fn expand(input: &str, ctx: &MacroContext) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }

        // %% → literal %
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }

        // %DIGI% (multi-letter, terminated by %)
        if matches!(chars.peek(), Some('D') | Some('d')) {
            let snapshot: String = chars.clone().take(5).collect();
            if snapshot.eq_ignore_ascii_case("DIGI%") {
                for _ in 0..5 {
                    chars.next();
                }
                out.push_str(&ctx.digi);
                continue;
            }
        }

        let Some(code) = chars.next() else {
            out.push('%');
            break;
        };

        let mut value = match code {
            'V' | 'v' => ctx.version.clone(),
            'C' | 'c' => ctx.peer_call.clone(),
            'N' | 'n' => {
                let name = ctx.peer_name.trim();
                if name.is_empty() {
                    ctx.peer_call.clone()
                } else {
                    ctx.peer_name.clone()
                }
            }
            'Y' | 'y' => ctx.my_call.clone(),
            'K' | 'k' => ctx.channel.to_string(),
            'T' | 't' => ctx.time.clone(),
            'D' | 'd' => ctx.date.clone(),
            'B' | 'b' => "\u{0007}".into(),
            'I' | 'i' => ctx.news_hint.clone(),
            'Z' | 'z' => ctx.zone.clone(),
            '_' => "\r\n".into(),
            'O' | 'o' => ctx.origin_line.clone(),
            'Q' | 'q' => ctx.qrg.clone(),
            'A' | 'a' => ctx.activity.clone(),
            '?' => {
                if ctx.name_unknown {
                    // Placeholder — real UI should prompt; mark for session layer.
                    "{name?}".into()
                } else {
                    String::new()
                }
            }
            '>' => {
                // Prompt-on marker; session layer interprets. Emit empty in pure text.
                String::new()
            }
            other => {
                out.push('%');
                out.push(other);
                continue;
            }
        };

        // Optional #NN width
        if chars.peek() == Some(&'#') {
            chars.next();
            let mut width = String::new();
            while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                width.push(chars.next().unwrap());
            }
            if let Ok(w) = width.parse::<usize>() {
                if value.chars().count() < w {
                    value.extend(std::iter::repeat(' ').take(w - value.chars().count()));
                } else {
                    value = value.chars().take(w).collect();
                }
            } else {
                out.push('#');
                out.push_str(&width);
            }
        }

        out.push_str(&value);
    }

    out
}

/// Ordered macro list for the editor help panel — matches `expand()` above.
pub fn help_entries() -> &'static [(&'static str, &'static str)] {
    &[
        ("%V", "Software version"),
        ("%C", "Peer callsign"),
        ("%N", "Peer name, else callsign"),
        ("%Y", "Own callsign"),
        ("%K", "Channel number"),
        ("%T", "Time HH:MM:SS"),
        ("%D", "Date"),
        ("%B", "Bell character"),
        ("%I", "News available hint"),
        ("%Z", "PC timezone"),
        ("%_", "CRLF"),
        ("%%", "Literal %"),
        ("%O", "Random ORIGIN.GPI line"),
        ("%Q", "QRG (MHz)"),
        ("%A", "Sysop at keyboard / last seen"),
        ("%?", "Ask for name if unknown"),
        ("%>", "Enable remote prompt"),
        ("%DIGI%", "Digipeater path"),
        ("%x#nn", "Pad/truncate any macro to nn"),
    ]
}

/// Quick reference map for docs / UI help.
pub fn catalogue() -> HashMap<&'static str, &'static str> {
    help_entries().iter().copied().collect()
}

/// Short name for a Windows timezone id. `daylight` picks the summer name.
///
/// Holland is `W. Europe Standard Time`: CET in winter, CEST in summer.
pub fn zone_abbrev(windows_id: &str, daylight: bool) -> Option<&'static str> {
    let (standard, summer) = match windows_id {
        "W. Europe Standard Time"
        | "Central Europe Standard Time"
        | "Central European Standard Time"
        | "Romance Standard Time" => ("CET", "CEST"),
        "GMT Standard Time" => ("GMT", "BST"),
        "Greenwich Standard Time" => ("GMT", "GMT"),
        "UTC" => ("UTC", "UTC"),
        "GTB Standard Time" | "E. Europe Standard Time" | "FLE Standard Time" => ("EET", "EEST"),
        "Russian Standard Time" => ("MSK", "MSK"),
        "Turkey Standard Time" => ("TRT", "TRT"),
        "Israel Standard Time" => ("IST", "IDT"),
        "South Africa Standard Time" => ("SAST", "SAST"),
        "W. Central Africa Standard Time" => ("WAT", "WAT"),
        "Eastern Standard Time" => ("EST", "EDT"),
        "Central Standard Time" | "Canada Central Standard Time" => ("CST", "CDT"),
        "Mountain Standard Time" => ("MST", "MDT"),
        "US Mountain Standard Time" => ("MST", "MST"),
        "Pacific Standard Time" => ("PST", "PDT"),
        "Alaskan Standard Time" => ("AKST", "AKDT"),
        "Hawaiian Standard Time" => ("HST", "HST"),
        "AUS Eastern Standard Time" => ("AEST", "AEDT"),
        "Cen. Australia Standard Time" => ("ACST", "ACDT"),
        "W. Australia Standard Time" => ("AWST", "AWST"),
        "New Zealand Standard Time" => ("NZST", "NZDT"),
        "Tokyo Standard Time" => ("JST", "JST"),
        "China Standard Time" => ("CST", "CST"),
        "India Standard Time" => ("IST", "IST"),
        _ => return None,
    };
    Some(if daylight { summer } else { standard })
}

/// Timezone of the PC clock. Known Windows zones use the short name
/// (`CET` / `CEST`). Anything else is `UTC` or `UTC+1`.
pub fn local_zone() -> String {
    if let Some((id, daylight)) = pc_zone() {
        if let Some(name) = zone_abbrev(&id, daylight) {
            return name.to_string();
        }
    }
    utc_offset_label(Local::now().offset().local_minus_utc())
}

fn utc_offset_label(seconds: i32) -> String {
    if seconds == 0 {
        return "UTC".into();
    }
    let sign = if seconds >= 0 { '+' } else { '-' };
    let seconds = seconds.unsigned_abs();
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    if mins == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours}:{mins:02}")
    }
}

/// `(Windows timezone id, daylight in effect)`.
fn pc_zone() -> Option<(String, bool)> {
    #[cfg(windows)]
    {
        windows_zone()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_zone() -> Option<(String, bool)> {
    #[repr(C)]
    struct SystemTime {
        _year: u16,
        _month: u16,
        _dow: u16,
        _day: u16,
        _hour: u16,
        _minute: u16,
        _second: u16,
        _millis: u16,
    }
    #[repr(C)]
    struct DynTz {
        _bias: i32,
        _standard_name: [u16; 32],
        _standard_date: SystemTime,
        _standard_bias: i32,
        _daylight_name: [u16; 32],
        _daylight_date: SystemTime,
        _daylight_bias: i32,
        key_name: [u16; 128],
        dynamic_disabled: u8,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDynamicTimeZoneInformation(info: *mut DynTz) -> u32;
    }
    const DAYLIGHT: u32 = 2;
    const INVALID: u32 = u32::MAX;
    unsafe {
        let mut info = std::mem::zeroed::<DynTz>();
        let rc = GetDynamicTimeZoneInformation(&mut info);
        if rc == INVALID {
            return None;
        }
        let key = wide_z(&info.key_name);
        if key.is_empty() {
            return None;
        }
        let daylight = rc == DAYLIGHT && info.dynamic_disabled == 0;
        Some((key, daylight))
    }
}

#[cfg(windows)]
fn wide_z(words: &[u16]) -> String {
    let end = words.iter().position(|&c| c == 0).unwrap_or(words.len());
    String::from_utf16_lossy(&words[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_version_and_pad() {
        let ctx = MacroContext {
            peer_name: "Ulf".into(),
            ..MacroContext::demo()
        };
        assert_eq!(
            expand("GP %V", &ctx),
            format!("GP {}", crate::version::APP_VERSION)
        );
        assert_eq!(expand("%N#6", &ctx), "Ulf   ");
        assert_eq!(expand("100%%", &ctx), "100%");
    }

    #[test]
    fn name_macro_falls_back_to_callsign() {
        let ctx = MacroContext {
            peer_call: "NL0MSK".into(),
            peer_name: String::new(),
            ..MacroContext::demo()
        };
        assert_eq!(expand("Hallo %N,", &ctx), "Hallo NL0MSK,");
        let named = MacroContext {
            peer_name: "Jara".into(),
            ..ctx
        };
        assert_eq!(expand("Hallo %N,", &named), "Hallo Jara,");
    }

    #[test]
    fn activity_macro_present_under_five_minutes() {
        assert_eq!(
            activity_status(Duration::from_secs(4 * 60 + 59)),
            "Sysop is at Keyboard"
        );
        let ctx = MacroContext {
            activity: activity_status(Duration::from_secs(10)),
            ..MacroContext::demo()
        };
        assert_eq!(expand("%A", &ctx), "Sysop is at Keyboard");
    }

    #[test]
    fn activity_macro_last_seen_minutes_hours_days() {
        assert_eq!(
            activity_status(Duration::from_secs(5 * 60)),
            "Sysop not at keyboard, last seen 5 minutes ago"
        );
        assert_eq!(
            activity_status(Duration::from_secs(60 * 60)),
            "Sysop not at keyboard, last seen 1 hour ago"
        );
        assert_eq!(
            activity_status(Duration::from_secs(2 * 86_400)),
            "Sysop not at keyboard, last seen 2 days ago"
        );
    }

    #[test]
    fn holland_is_cet_in_winter_and_cest_in_summer() {
        assert_eq!(zone_abbrev("W. Europe Standard Time", false), Some("CET"));
        assert_eq!(zone_abbrev("W. Europe Standard Time", true), Some("CEST"));
        assert_eq!(zone_abbrev("Romance Standard Time", false), Some("CET"));
        assert_eq!(zone_abbrev("GMT Standard Time", true), Some("BST"));
    }

    #[test]
    fn the_pc_clock_zone_is_not_a_hardcoded_utc() {
        let label = local_zone();
        assert!(!label.is_empty());
        #[cfg(windows)]
        {
            let (id, daylight) = super::pc_zone().expect("Windows timezone");
            if let Some(expect) = zone_abbrev(&id, daylight) {
                assert_eq!(label, expect, "id={id} daylight={daylight}");
            } else {
                assert!(label.starts_with("UTC"), "id={id} label={label}");
            }
        }
    }
}
