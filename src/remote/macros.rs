//! GPI / CTEXT macro expansion — classic Graphic Packet `%` tokens.
//!
//! Documented set: fifteen macros in `README.TXT`, plus `%DIGI%` and the
//! config-only `%TFPCX200` keyword. Formatting suffix `#NN` pads/truncates.

use std::collections::HashMap;

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
            name_unknown: false,
        }
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
        ("%Z", "Timezone"),
        ("%_", "CRLF"),
        ("%%", "Literal %"),
        ("%O", "Random ORIGIN.GPI line"),
        ("%Q", "QRG (MHz)"),
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
}
