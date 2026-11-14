//! Channel-window ANSI colour (SGR).
//!
//! A payload may contain `ESC [ … m` (`\x1b`, also written `\033`). The
//! channel keeps a pen across packet cuts and across rows:
//! `[0m` resets, `[1m` selects the bright foreground, `[30m`–`[37m` set it.
//! Background codes `[40m`–`[47m` are dropped. Every other CSI / ESC
//! sequence is removed so it does not show as text.
//!
//! The sequence may be split across packets. Bytes held in the decoder are
//! not drawn until the sequence finishes.

use eframe::egui::Color32;

/// Pen for the next visible bytes. `fg` is an SGR number 30–37, or the
/// channel's own colour when `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnsiStyle {
    pub fg: Option<u8>,
    pub bold: bool,
}

impl AnsiStyle {
    pub fn plain() -> Self {
        Self {
            fg: None,
            bold: false,
        }
    }

    /// Colour to draw. `fallback` is the channel row colour (green / white).
    pub fn paint(self, fallback: Color32) -> Color32 {
        let Some(fg) = self.fg else {
            return if self.bold {
                brighten(fallback)
            } else {
                fallback
            };
        };
        match (fg, self.bold) {
            (30, false) => Color32::BLACK,
            // Bright black is VGA dark gray, which vanishes on the channel
            // background, so bold black is drawn as light gray.
            (30, true) => Color32::from_rgb(170, 170, 170),
            (31, false) => Color32::from_rgb(170, 0, 0),
            (31, true) => Color32::from_rgb(255, 85, 85),
            (32, false) => Color32::from_rgb(0, 170, 0),
            (32, true) => Color32::from_rgb(85, 255, 85),
            (33, false) => Color32::from_rgb(170, 170, 0),
            (33, true) => Color32::from_rgb(255, 255, 85),
            (34, false) => Color32::from_rgb(0, 0, 170),
            (34, true) => Color32::from_rgb(85, 85, 255),
            (35, false) => Color32::from_rgb(170, 0, 170),
            (35, true) => Color32::from_rgb(255, 85, 255),
            (36, false) => Color32::from_rgb(0, 170, 170),
            (36, true) => Color32::from_rgb(85, 255, 255),
            (37, false) => Color32::from_rgb(170, 170, 170),
            (37, true) => Color32::WHITE,
            _ => fallback,
        }
    }
}

fn brighten(c: Color32) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_add(85),
        c.g().saturating_add(85),
        c.b().saturating_add(85),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    Ground,
    Esc,
    /// ESC followed by one intermediate (`ESC ( B` and the like).
    EscInt,
    Csi,
    Osc,
    /// ESC inside an OSC, waiting to see if the next byte is `\`.
    OscEsc,
}

/// Incremental SGR stripper / painter for one channel.
#[derive(Debug, Clone)]
pub struct AnsiDecoder {
    style: AnsiStyle,
    state: Scan,
    params: Vec<u8>,
    overflow: bool,
}

impl Default for AnsiDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiDecoder {
    pub fn new() -> Self {
        Self {
            style: AnsiStyle::plain(),
            state: Scan::Ground,
            params: Vec::new(),
            overflow: false,
        }
    }

    /// Visible runs in `input`. Newlines are left in the text; the channel
    /// walker splits those itself. `allow_color` false still removes the
    /// sequences and reports a plain pen.
    pub fn push_text(&mut self, input: &str, allow_color: bool) -> Vec<(String, AnsiStyle)> {
        let bytes = input.as_bytes();
        let mut out: Vec<(String, AnsiStyle)> = Vec::new();
        let mut text_start: Option<usize> = None;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match self.state {
                Scan::Ground => {
                    if b == 0x1b {
                        self.flush(input, &mut text_start, i, allow_color, &mut out);
                        self.state = Scan::Esc;
                    } else if text_start.is_none() {
                        text_start = Some(i);
                    }
                    i += 1;
                }
                Scan::Esc => {
                    self.state = match b {
                        b'[' => Scan::Csi,
                        b']' => Scan::Osc,
                        0x20..=0x2f => Scan::EscInt,
                        _ => Scan::Ground,
                    };
                    if self.state == Scan::Csi {
                        self.params.clear();
                        self.overflow = false;
                    }
                    i += 1;
                }
                Scan::EscInt => {
                    self.state = Scan::Ground;
                    i += 1;
                }
                Scan::Csi => {
                    if b == 0x1b {
                        self.params.clear();
                        self.overflow = false;
                        self.state = Scan::Esc;
                        i += 1;
                    } else if (0x20..=0x3f).contains(&b) {
                        if self.params.len() < 64 {
                            self.params.push(b);
                        } else {
                            self.overflow = true;
                        }
                        i += 1;
                    } else if (0x40..=0x7e).contains(&b) {
                        if b == b'm' && !self.overflow {
                            apply_sgr(&mut self.style, &self.params);
                        }
                        self.params.clear();
                        self.overflow = false;
                        self.state = Scan::Ground;
                        i += 1;
                    } else {
                        self.params.clear();
                        self.overflow = false;
                        self.state = Scan::Ground;
                        i += 1;
                    }
                }
                Scan::Osc => {
                    if b == 0x07 {
                        self.state = Scan::Ground;
                        i += 1;
                    } else if b == 0x1b {
                        self.state = Scan::OscEsc;
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                Scan::OscEsc => {
                    if b == b'\\' {
                        self.state = Scan::Ground;
                        i += 1;
                    } else {
                        // The ESC ends the OSC and starts another sequence.
                        self.state = Scan::Esc;
                    }
                }
            }
        }
        if self.state == Scan::Ground {
            self.flush(input, &mut text_start, input.len(), allow_color, &mut out);
        }
        out
    }
}

impl AnsiDecoder {
    fn flush(
        &self,
        input: &str,
        start: &mut Option<usize>,
        end: usize,
        allow_color: bool,
        out: &mut Vec<(String, AnsiStyle)>,
    ) {
        let Some(from) = start.take() else {
            return;
        };
        if from >= end {
            return;
        }
        let style = if allow_color {
            self.style
        } else {
            AnsiStyle::plain()
        };
        let text = &input[from..end];
        if text.is_empty() {
            return;
        }
        if let Some((prev, prev_style)) = out.last_mut() {
            if *prev_style == style {
                prev.push_str(text);
                return;
            }
        }
        out.push((text.to_string(), style));
    }
}

fn apply_sgr(style: &mut AnsiStyle, params: &[u8]) {
    let Ok(text) = std::str::from_utf8(params) else {
        return;
    };
    if text.is_empty() {
        *style = AnsiStyle::plain();
        return;
    }
    for part in text.split(';') {
        let n = if part.is_empty() {
            0
        } else if let Ok(n) = part.parse::<i32>() {
            n
        } else {
            continue;
        };
        match n {
            0 => *style = AnsiStyle::plain(),
            1 => style.bold = true,
            30..=37 => style.fg = Some(n as u8),
            // Background. The channel keeps its own background.
            40..=47 => {}
            _ => {}
        }
    }
}

/// Turn the DOS-style Alt+27 arrow into ESC only for complete SGR sequences.
pub fn expand_sgr_arrows(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = 0;
    for (index, _) in text.match_indices('←') {
        let rest = &text[index + '←'.len_utf8()..];
        let Some(params) = rest.strip_prefix('[') else {
            continue;
        };
        let length = params
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b';')
            .count();
        if length <= 64 && params.as_bytes().get(length) == Some(&b'm') {
            out.push_str(&text[start..index]);
            out.push('\x1b');
            start = index + '←'.len_utf8();
        }
    }
    out.push_str(&text[start..]);
    out
}

/// Length of a complete numeric SGR sequence at the start of `bytes`.
pub fn sgr_sequence_len(bytes: &[u8]) -> Option<usize> {
    let params = bytes.strip_prefix(b"\x1b[")?;
    let len = params
        .iter()
        .take_while(|b| b.is_ascii_digit() || **b == b';')
        .count();
    (len <= 64 && params.get(len) == Some(&b'm')).then_some(len + 3)
}

/// Remove colour sequences. A trailing unfinished sequence is dropped.
pub fn strip_ansi(text: &str) -> String {
    let mut dec = AnsiDecoder::new();
    let mut out = String::new();
    for (piece, _) in dec.push_text(text, false) {
        out.push_str(&piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_arrows_only_expand_complete_color_sequences() {
        assert_eq!(
            expand_sgr_arrows("←[1;31mNot!←[0m ← arrow ←[2J ←[31"),
            "\x1b[1;31mNot!\x1b[0m ← arrow ←[2J ←[31"
        );
        assert_eq!(
            expand_sgr_arrows(&format!("←[{}m", "1".repeat(65))),
            format!("←[{}m", "1".repeat(65))
        );
    }
}
