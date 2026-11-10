//! WA8DED `M` monitor listing.
//!
//! This is the monitor window's filter. It does not decide whether a frame is
//! received or delivered on a connected channel. `N` lists nothing. `I`, `U`,
//! and `S` pick I, UI, and supervisory frames (RR/RNR/REJ plus SABM, UA, DISC,
//! DM, and FRMR — UI is the only unnumbered frame with its own letter). `C`
//! keeps the monitor listing while a link is up. `+` keeps up to eight
//! callsigns; `-` drops up to eight. The two lists are never both active.
//!
//! KISS and TFPCX have no `M` in the modem, so the monitor view uses this.
//! A WA8DED TNC applies the same rule itself and only withholds channel-0
//! monitor text.

/// Which monitored frame the `M` letters select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameClass {
    /// Information frames (`I`).
    Info,
    /// Unnumbered information (`U`).
    Ui,
    /// Supervisory and the other control frames (`S`).
    Supervisory,
}

/// One address heard in a monitor header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub class: Option<FrameClass>,
    pub calls: Vec<String>,
}

/// Operator line that starts with `M` or `MONITOR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorInput {
    /// `M` with no parameter — ask for the current setting.
    Query,
    /// Parameter text after the command word.
    Set(String),
}

/// Frame types and callsign list for the monitor window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorFilter {
    pub i: bool,
    pub u: bool,
    pub s: bool,
    pub c: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for MonitorFilter {
    fn default() -> Self {
        Self {
            i: false,
            u: false,
            s: false,
            c: false,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

impl MonitorFilter {
    /// Classic GP default: I, UI, supervisory, and monitor while connected.
    pub fn iusc() -> Self {
        Self {
            i: true,
            u: true,
            s: true,
            c: true,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }

    /// Config `monitor_mode`. Empty or unreadable text becomes [`Self::iusc`].
    pub fn from_config(text: &str) -> Self {
        let text = text.trim().trim_matches('\0');
        if text.is_empty() {
            return Self::iusc();
        }
        Self::apply(&Self::default(), text).unwrap_or_else(|_| Self::iusc())
    }

    /// `M` / `MONITOR` plus an optional parameter. Anything else is `None`.
    pub fn parse_command(line: &str) -> Option<MonitorInput> {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        let cmd = parts.next()?;
        if !cmd.eq_ignore_ascii_case("M") && !cmd.eq_ignore_ascii_case("MONITOR") {
            return None;
        }
        let rest = line
            .split_once(char::is_whitespace)
            .map(|(_, rest)| rest.trim())
            .unwrap_or("");
        if rest.is_empty() {
            Some(MonitorInput::Query)
        } else {
            Some(MonitorInput::Set(rest.to_string()))
        }
    }

    /// Apply one `M` parameter. Letters are taken left to right. `N` clears
    /// the frame letters. A missing `+`/`-` clears both call lists, so every
    /// callsign is a candidate again.
    pub fn apply(current: &Self, args: &str) -> Result<Self, &'static str> {
        let args = args.trim().trim_matches('\0').trim();
        if args.is_empty() {
            return Err("missing monitor parameter");
        }
        let mut tokens = args.split_whitespace().peekable();
        let head = tokens.next().ok_or("missing monitor parameter")?;

        let mut i = false;
        let mut u = false;
        let mut s = false;
        let mut c = false;
        let mut types_touched = false;
        let mut sign: Option<bool> = None;
        let mut calls = Vec::new();

        let mut chars = head.chars().peekable();
        while let Some(ch) = chars.next() {
            let ch = ch.to_ascii_uppercase();
            match ch {
                'N' => {
                    touch(&mut types_touched, &mut i, &mut u, &mut s, &mut c);
                    i = false;
                    u = false;
                    s = false;
                    c = false;
                }
                'I' => {
                    touch(&mut types_touched, &mut i, &mut u, &mut s, &mut c);
                    i = true;
                }
                'U' => {
                    touch(&mut types_touched, &mut i, &mut u, &mut s, &mut c);
                    u = true;
                }
                'S' => {
                    touch(&mut types_touched, &mut i, &mut u, &mut s, &mut c);
                    s = true;
                }
                'C' => {
                    touch(&mut types_touched, &mut i, &mut u, &mut s, &mut c);
                    c = true;
                }
                // Older TNC-1 letters. WA8DED 2 does not use them.
                'R' | 'T' => {
                    touch(&mut types_touched, &mut i, &mut u, &mut s, &mut c);
                }
                '+' | '-' => {
                    if sign.is_some() {
                        return Err("+ and - cannot be used together");
                    }
                    sign = Some(ch == '+');
                    let rest: String = chars.collect();
                    if !rest.is_empty() {
                        calls.push(normalize_call(&rest).ok_or("bad callsign")?);
                    }
                    break;
                }
                _ => return Err("bad monitor parameter"),
            }
        }

        if sign.is_none() {
            if let Some(tok) = tokens.peek().copied() {
                if tok.starts_with('+') || tok.starts_with('-') {
                    let tok = tokens.next().unwrap();
                    let (include, rest) = if let Some(rest) = tok.strip_prefix('+') {
                        (true, rest)
                    } else if let Some(rest) = tok.strip_prefix('-') {
                        (false, rest)
                    } else {
                        (false, tok)
                    };
                    sign = Some(include);
                    if !rest.is_empty() {
                        calls.push(normalize_call(rest).ok_or("bad callsign")?);
                    }
                }
            }
        }

        for tok in tokens {
            if tok.starts_with('+') || tok.starts_with('-') {
                return Err("+ and - cannot be used together");
            }
            calls.push(normalize_call(tok).ok_or("bad callsign")?);
        }
        if calls.len() > 8 {
            return Err("at most 8 callsigns");
        }
        if sign.is_none() && !calls.is_empty() {
            return Err("bad monitor parameter");
        }

        let mut out = current.clone();
        if types_touched {
            out.i = i;
            out.u = u;
            out.s = s;
            out.c = c;
        }
        match sign {
            Some(true) => {
                out.include = calls;
                out.exclude.clear();
            }
            Some(false) => {
                out.exclude = calls;
                out.include.clear();
            }
            None => {
                out.include.clear();
                out.exclude.clear();
            }
        }
        Ok(out)
    }

    /// Letters in `IUSC` order, then `+` or `-` and the call list. `N` when
    /// no frame type is selected.
    pub fn label(&self) -> String {
        let mut label = String::new();
        if self.i {
            label.push('I');
        }
        if self.u {
            label.push('U');
        }
        if self.s {
            label.push('S');
        }
        if self.c {
            label.push('C');
        }
        if label.is_empty() {
            label.push('N');
        }
        if !self.include.is_empty() {
            label.push('+');
            label.push_str(&self.include.join(" "));
        } else if !self.exclude.is_empty() {
            label.push('-');
            label.push_str(&self.exclude.join(" "));
        }
        label
    }

    /// Whether this frame is listed on the monitor.
    ///
    /// `connected` is true while any channel has a link. Without `C` the
    /// monitor stays quiet then. An unknown class is listed only when some
    /// frame type is enabled, no include-list is set, and `C` allows it.
    pub fn allows(&self, class: Option<FrameClass>, calls: &[String], connected: bool) -> bool {
        if connected && !self.c {
            return false;
        }
        let Some(class) = class else {
            if !self.i && !self.u && !self.s {
                return false;
            }
            if !self.include.is_empty() {
                return false;
            }
            return !listed(&self.exclude, calls);
        };
        let type_ok = match class {
            FrameClass::Info => self.i,
            FrameClass::Ui => self.u,
            FrameClass::Supervisory => self.s,
        };
        if !type_ok {
            return false;
        }
        if !self.include.is_empty() {
            return listed(&self.include, calls);
        }
        !listed(&self.exclude, calls)
    }
}

/// `fm SRC to DEST [via …] ctl …`, or a TNC-2 `SRC>DEST:` line (treated as UI).
pub fn parse_heard(line: &str) -> Option<Heard> {
    parse_fm(line).or_else(|| parse_tnc2(line))
}

fn touch(touched: &mut bool, i: &mut bool, u: &mut bool, s: &mut bool, c: &mut bool) {
    if *touched {
        return;
    }
    *touched = true;
    *i = false;
    *u = false;
    *s = false;
    *c = false;
}

fn listed(list: &[String], calls: &[String]) -> bool {
    calls
        .iter()
        .any(|call| list.iter().any(|entry| entry.eq_ignore_ascii_case(call)))
}

/// Uppercase call with SSID 0 omitted. `None` when it is not an AX.25 call.
pub fn normalize_call(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_matches(|c: char| c == '*' || c == ',');
    if raw.is_empty() {
        return None;
    }
    let (base, ssid) = match raw.split_once('-') {
        Some((base, ssid)) => (base, ssid),
        None => (raw, "0"),
    };
    if !(1..=6).contains(&base.len()) || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !ssid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let ssid: u8 = ssid.parse().ok()?;
    if ssid > 15 {
        return None;
    }
    let base = base.to_ascii_uppercase();
    if ssid == 0 {
        Some(base)
    } else {
        Some(format!("{base}-{ssid}"))
    }
}

fn parse_fm(line: &str) -> Option<Heard> {
    let (_, rest) = super::ax25::split_monitor_fm(line)?;
    let mut tokens = rest.split_whitespace();
    let mut src = tokens.next()?;
    if is_hhmm(src) {
        src = tokens.next()?;
    }
    if !tokens.next()?.eq_ignore_ascii_case("to") {
        return None;
    }
    let dest = tokens.next()?;
    let mut vias = Vec::new();
    let mut ctl = None;
    if let Some(word) = tokens.next() {
        if word.eq_ignore_ascii_case("via") {
            if let Some(list) = tokens.next() {
                if !list.eq_ignore_ascii_case("ctl") {
                    vias.extend(list.split(',').map(str::to_string));
                } else {
                    ctl = tokens.next().map(str::to_string);
                }
            }
            if ctl.is_none() {
                for word in tokens.by_ref() {
                    if word.eq_ignore_ascii_case("ctl") {
                        ctl = tokens.next().map(str::to_string);
                        break;
                    }
                }
            }
        } else if word.eq_ignore_ascii_case("ctl") {
            ctl = tokens.next().map(str::to_string);
        }
    }
    let mut calls = Vec::new();
    if let Some(call) = normalize_call(src) {
        calls.push(call);
    }
    if let Some(call) = normalize_call(dest) {
        calls.push(call);
    }
    for via in vias {
        if let Some(call) = normalize_call(&via) {
            calls.push(call);
        }
    }
    Some(Heard {
        class: ctl.as_deref().and_then(class_from_ctl),
        calls,
    })
}

fn parse_tnc2(line: &str) -> Option<Heard> {
    let head = line.split_once(':').map(|(head, _)| head).unwrap_or(line);
    let (src, rest) = head.split_once('>')?;
    let mut parts = rest.split(',');
    let dest = parts.next()?;
    let mut calls = Vec::new();
    calls.push(normalize_call(src.trim())?);
    calls.push(normalize_call(dest.trim())?);
    for via in parts {
        if let Some(call) = normalize_call(via.trim()) {
            calls.push(call);
        }
    }
    Some(Heard {
        class: Some(FrameClass::Ui),
        calls,
    })
}

fn class_from_ctl(ctl: &str) -> Option<FrameClass> {
    let ctl = ctl.trim().to_ascii_uppercase();
    if ctl.starts_with("UI") {
        return Some(FrameClass::Ui);
    }
    let mut chars = ctl.chars();
    if chars.next() == Some('I') && chars.next().is_some_and(|c| c.is_ascii_digit()) {
        return Some(FrameClass::Info);
    }
    if ctl.starts_with("RR")
        || ctl.starts_with("RNR")
        || ctl.starts_with("REJ")
        || ctl.starts_with("SREJ")
        || ctl.starts_with("SABM")
        || ctl.starts_with("DISC")
        || ctl.starts_with("UA")
        || ctl.starts_with("DM")
        || ctl.starts_with("FRMR")
    {
        return Some(FrameClass::Supervisory);
    }
    None
}

fn is_hhmm(token: &str) -> bool {
    let Some((h, m)) = token.split_once(':') else {
        return false;
    };
    h.len() == 2
        && m.len() == 2
        && h.bytes().all(|b| b.is_ascii_digit())
        && m.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(current: &MonitorFilter, args: &str) -> MonitorFilter {
        MonitorFilter::apply(current, args).unwrap()
    }

    #[test]
    fn letters_select_frame_classes() {
        let mode = set(&MonitorFilter::default(), "IUSC");
        assert_eq!(mode.label(), "IUSC");
        let calls = Vec::new();
        assert!(mode.allows(Some(FrameClass::Info), &calls, false));
        assert!(mode.allows(Some(FrameClass::Ui), &calls, false));
        assert!(mode.allows(Some(FrameClass::Supervisory), &calls, false));
        assert!(mode.allows(Some(FrameClass::Ui), &calls, true));
        let ui_only = set(&MonitorFilter::default(), "U");
        assert!(ui_only.allows(Some(FrameClass::Ui), &calls, false));
        assert!(!ui_only.allows(Some(FrameClass::Info), &calls, false));
        assert!(!ui_only.allows(Some(FrameClass::Supervisory), &calls, false));
    }

    #[test]
    fn none_and_missing_c_only_hide_the_listing() {
        let none = set(&MonitorFilter::iusc(), "N");
        assert_eq!(none.label(), "N");
        let calls = Vec::new();
        assert!(!none.allows(Some(FrameClass::Ui), &calls, false));
        assert!(!none.allows(None, &calls, false));
        let quiet = set(&MonitorFilter::default(), "IU");
        assert!(quiet.allows(Some(FrameClass::Ui), &calls, false));
        assert!(!quiet.allows(Some(FrameClass::Ui), &calls, true));
        assert_eq!(quiet.label(), "IU");
    }

    #[test]
    fn call_lists_are_exclusive_and_capped() {
        let include = set(&MonitorFilter::iusc(), "U+DB0ABC DL0XYZ-2");
        assert_eq!(include.label(), "U+DB0ABC DL0XYZ-2");
        let yes = vec!["DB0ABC".into()];
        let no = vec!["NOCALL".into()];
        assert!(include.allows(Some(FrameClass::Ui), &yes, false));
        assert!(!include.allows(Some(FrameClass::Ui), &no, false));
        assert!(!include.allows(Some(FrameClass::Info), &yes, false));
        let exclude = set(&include, "U-NOCALL");
        assert!(exclude.include.is_empty());
        assert!(!exclude.allows(Some(FrameClass::Ui), &no, false));
        assert!(exclude.allows(Some(FrameClass::Ui), &yes, false));
        let cleared = set(&exclude, "IUSC");
        assert!(cleared.include.is_empty() && cleared.exclude.is_empty());
        assert!(MonitorFilter::apply(&cleared, "U+NOTACALL").is_err());
        assert!(MonitorFilter::apply(&cleared, "U+C1 C2 C3 C4 C5 C6 C7 C8 C9").is_err());
        assert!(MonitorFilter::apply(&cleared, "IU+-DB0ABC").is_err());
        let empty = set(&include, "+");
        assert!(empty.include.is_empty());
        assert!(empty.u);
    }

    #[test]
    fn n_then_a_letter_keeps_that_letter() {
        let mode = set(&MonitorFilter::iusc(), "NI");
        assert!(mode.i && !mode.u && !mode.s && !mode.c);
        let cleared = set(&mode, "IN");
        assert_eq!(cleared.label(), "N");
    }

    #[test]
    fn headers_carry_class_and_calls() {
        let heard = parse_heard("fm DB0ABC-0 to CQ via DIGI-1* ctl UI^ pid F0").unwrap();
        assert_eq!(heard.class, Some(FrameClass::Ui));
        assert_eq!(heard.calls, vec!["DB0ABC", "CQ", "DIGI-1"]);
        let i = parse_heard("fm 03:22 NL0MSK to NL0MSK-2 ctl I00^ pid F0").unwrap();
        assert_eq!(i.class, Some(FrameClass::Info));
        assert_eq!(i.calls, vec!["NL0MSK", "NL0MSK-2"]);
        let sabm = parse_heard("fm NL0MSK to NL0MSK ctl SABM+").unwrap();
        assert_eq!(sabm.class, Some(FrameClass::Supervisory));
        let port = parse_heard("fm2 03:22 NL0MSK to CQ ctl UI^ pid F0").unwrap();
        assert_eq!(port.class, Some(FrameClass::Ui));
        assert_eq!(port.calls, vec!["NL0MSK", "CQ"]);
        let tnc2 = parse_heard("DB0ET>U3RTS5,WIDE1-1:hello").unwrap();
        assert_eq!(tnc2.class, Some(FrameClass::Ui));
        assert_eq!(tnc2.calls, vec!["DB0ET", "U3RTS5", "WIDE1-1"]);
    }

    #[test]
    fn bare_m_is_a_query() {
        assert_eq!(
            MonitorFilter::parse_command("  m  "),
            Some(MonitorInput::Query)
        );
        assert!(matches!(
            MonitorFilter::parse_command("MONITOR IUSC"),
            Some(MonitorInput::Set(_))
        ));
        assert!(MonitorFilter::parse_command("I NL0MSK").is_none());
    }
}
