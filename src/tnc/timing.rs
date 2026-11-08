//! Shared TNC timing values from `[tnc] init` / WA8DED.INI `[open]`.
//!
//! Hostmode (WA8DED / TFPCX) sends these as text commands (`P 63`, `T 45`, …).
//! KISS sends the subset the modem understands as TAPR command frames after
//! `FEND`: TXDELAY / PERSIST / SLOTTIME. Frack / T2 / T3 / MaxUnack stay on
//! the host (our AX.25 stack) when the wire is KISS.

/// Parsed from lines like `P 63`, `W 9`, `T 45`, `O 3`, `F 5`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimingParams {
    /// WA8DED `P` / KISS PERSIST (0–255).
    pub persist: Option<u8>,
    /// WA8DED `W` / KISS SLOTTIME (10 ms units).
    pub slottime: Option<u8>,
    /// WA8DED `T` / KISS TXDELAY (10 ms units).
    pub txdelay: Option<u8>,
    /// WA8DED `O` — max outstanding I-frames (host-side on KISS).
    pub maxunack: Option<u8>,
    /// WA8DED `F` — frack / T1 (seconds or TNC units; host-side on KISS).
    pub frack: Option<u32>,
}

impl TimingParams {
    pub fn from_lines<'a, I>(lines: I) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut out = Self::default();
        for raw in lines {
            let line = raw.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(key) = parts.next() else {
                continue;
            };
            let Some(val) = parts.next() else {
                continue;
            };
            match key.to_ascii_uppercase().as_str() {
                "P" => {
                    if let Ok(n) = val.parse::<u8>() {
                        out.persist = Some(n);
                    }
                }
                "W" => {
                    if let Ok(n) = val.parse::<u8>() {
                        out.slottime = Some(n);
                    }
                }
                "T" => {
                    if let Ok(n) = val.parse::<u8>() {
                        out.txdelay = Some(n);
                    }
                }
                "O" => {
                    if let Ok(n) = val.parse::<u8>() {
                        out.maxunack = Some(n);
                    }
                }
                "F" => {
                    if let Ok(n) = val.parse::<u32>() {
                        out.frack = Some(n);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// TAPR KISS host→TNC commands: `(cmd_nibble, arg)`.
    ///
    /// 1 = TXDELAY, 2 = PERSIST, 3 = SLOTTIME (each one data byte).
    pub fn kiss_set_commands(&self) -> Vec<(u8, u8)> {
        let mut cmds = Vec::new();
        if let Some(t) = self.txdelay {
            cmds.push((1, t));
        }
        if let Some(p) = self.persist {
            cmds.push((2, p));
        }
        if let Some(w) = self.slottime {
            cmds.push((3, w));
        }
        cmds
    }

    /// Open-style lines so [`super::wa8ded_ini::format_timing_banner`] can fill
    /// P/W/T/O; live F/@T2/@T3 still come from `replies` when present.
    pub fn as_open_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(p) = self.persist {
            lines.push(format!("P {p}"));
        }
        if let Some(w) = self.slottime {
            lines.push(format!("W {w}"));
        }
        if let Some(t) = self.txdelay {
            lines.push(format!("T {t}"));
        }
        if let Some(o) = self.maxunack {
            lines.push(format!("O {o}"));
        }
        lines
    }

    /// Optional `(cmd, reply)` pairs when Frack is known from config (no T2/T3).
    pub fn as_status_replies(&self) -> Vec<(String, String)> {
        let mut replies = Vec::new();
        if let Some(f) = self.frack {
            replies.push(("F".into(), f.to_string()));
        }
        replies
    }
}

/// Prefer `WA8DED.INI` `[open]`, else `[tnc] init`.
pub fn timing_from_tnc_config(tnc: &crate::config::TncConfig) -> TimingParams {
    let path = super::wa8ded_ini::resolve_wa8ded_ini(&tnc.wa8ded_ini);
    let lines = if path.is_file() {
        super::wa8ded_ini::section_lines(&path, "open")
    } else {
        tnc.init.clone()
    };
    TimingParams::from_lines(lines.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_style_lines() {
        let t = TimingParams::from_lines(["P 63", "W 9", "T 45", "O 3", "F 5", "M IUSC"]);
        assert_eq!(t.persist, Some(63));
        assert_eq!(t.slottime, Some(9));
        assert_eq!(t.txdelay, Some(45));
        assert_eq!(t.maxunack, Some(3));
        assert_eq!(t.frack, Some(5));
        assert_eq!(t.kiss_set_commands(), vec![(1, 45), (2, 63), (3, 9)]);
    }
}
