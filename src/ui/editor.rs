//! Simple GP text editor for `*.GPI` — IBM code page 437 load/save.

use std::fs;
use std::path::{Path, PathBuf};

/// Unicode codepoints for CP437 bytes `0x80..0xFF` (DOS / VGA text).
const CP437_HI: [char; 128] = [
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}', '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}',
    '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}', '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00A2}', '\u{00A3}', '\u{00A5}', '\u{20A7}', '\u{0192}',
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}', '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}',
    '\u{00BF}', '\u{2310}', '\u{00AC}', '\u{00BD}', '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255D}', '\u{255C}', '\u{255B}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}', '\u{2500}', '\u{253C}', '\u{255E}', '\u{255F}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256C}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256B}',
    '\u{256A}', '\u{2518}', '\u{250C}', '\u{2588}', '\u{2584}', '\u{258C}', '\u{2590}', '\u{2580}',
    '\u{03B1}', '\u{00DF}', '\u{0393}', '\u{03C0}', '\u{03A3}', '\u{03C3}', '\u{00B5}', '\u{03C4}',
    '\u{03A6}', '\u{0398}', '\u{03A9}', '\u{03B4}', '\u{221E}', '\u{03C6}', '\u{03B5}', '\u{2229}',
    '\u{2261}', '\u{00B1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00F7}', '\u{2248}',
    '\u{00B0}', '\u{2219}', '\u{00B7}', '\u{221A}', '\u{207F}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

fn byte_to_char(b: u8) -> char {
    if b < 0x80 {
        b as char
    } else {
        CP437_HI[(b - 0x80) as usize]
    }
}

fn char_to_byte(c: char) -> u8 {
    if c.is_ascii() {
        return c as u8;
    }
    for (i, &mapped) in CP437_HI.iter().enumerate() {
        if mapped == c {
            return 0x80 + i as u8;
        }
    }
    // Unmapped → DOS '?'
    b'?'
}

/// Decode CP437 file bytes to Unicode (egui TextEdit).
pub fn decode_cp437(raw: &[u8]) -> String {
    raw.iter().map(|&b| byte_to_char(b)).collect()
}

/// Encode Unicode editor text to CP437 bytes for disk.
pub fn encode_cp437(text: &str) -> Vec<u8> {
    text.chars().map(char_to_byte).collect()
}

#[derive(Debug, Clone)]
pub struct GpEditor {
    pub path: PathBuf,
    pub text: String,
    pub dirty: bool,
    /// Cursor line (1-based) for status bar.
    pub cursor_line: usize,
    /// Cursor column (1-based).
    pub cursor_col: usize,
}

impl Default for GpEditor {
    fn default() -> Self {
        Self {
            path: crate::paths::data_file("GTEXT.GPI"),
            text: String::new(),
            dirty: false,
            cursor_line: 1,
            cursor_col: 1,
        }
    }
}

impl GpEditor {
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("NONAME.TXT")
            .to_string()
    }

    pub fn line_count(&self) -> usize {
        if self.text.is_empty() {
            return 0;
        }
        self.text.lines().count().max(1)
    }

    pub fn load_path(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let raw = fs::read(path).map_err(|e| format!("load {}: {e}", path.display()))?;
        self.text = decode_cp437(&raw);
        // Normalize to `\n` in the buffer; save restores file newlines as CRLF if wanted.
        self.text = self.text.replace("\r\n", "\n").replace('\r', "\n");
        self.path = path.to_path_buf();
        self.dirty = false;
        self.cursor_line = 1;
        self.cursor_col = 1;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), String> {
        self.save_as(self.path.clone())
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = fs::create_dir_all(parent);
            }
        }
        // Classic GPI on DOS used CRLF.
        let normalized = self.text.replace('\r', "");
        let dos = normalized.replace('\n', "\r\n");
        let bytes = encode_cp437(&dos);
        fs::write(path, bytes).map_err(|e| format!("save {}: {e}", path.display()))?;
        self.path = path.to_path_buf();
        self.dirty = false;
        Ok(())
    }

    /// The cursor line, 0-based, clamped to the buffer.
    fn cursor_row(&self) -> usize {
        let n = self.text.split('\n').count().max(1);
        self.cursor_line.saturating_sub(1).min(n - 1)
    }

    pub fn copy_cursor_line(&self) -> String {
        self.text
            .split('\n')
            .nth(self.cursor_row())
            .unwrap_or("")
            .to_string()
    }

    /// Ctrl-Y: lift the cursor line out of the buffer.
    pub fn cut_cursor_line(&mut self) -> String {
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let i = self.cursor_row().min(lines.len() - 1);
        let line = lines.remove(i);
        if lines.is_empty() {
            lines.push(String::new());
        }
        self.text = lines.join("\n");
        self.dirty = true;
        self.cursor_line = (i + 1).min(lines.len());
        self.cursor_col = 1;
        line
    }

    /// Ctrl-P: put a cut or copied line back at the cursor.
    pub fn paste_line(&mut self, line: &str) {
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let i = self.cursor_row().min(lines.len() - 1);
        if lines.len() == 1 && lines[0].is_empty() {
            lines[0] = line.to_string();
        } else {
            lines.insert(i, line.to_string());
        }
        self.text = lines.join("\n");
        self.dirty = true;
    }

    /// Ctrl-D / Ctrl-T / Ctrl-O: append to the cursor line.
    pub fn append_on_cursor_line(&mut self, text: &str) {
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let i = self.cursor_row().min(lines.len() - 1);
        let row = &mut lines[i];
        if !row.is_empty() && !row.ends_with(' ') {
            row.push(' ');
        }
        row.push_str(text);
        self.text = lines.join("\n");
        self.dirty = true;
    }

    pub fn update_cursor_from_galley(&mut self, text: &str, cursor_cc: usize) {
        let cursor_cc = cursor_cc.min(text.len());
        let before = &text[..cursor_cc];
        self.cursor_line = before.chars().filter(|&c| c == '\n').count() + 1;
        self.cursor_col = before
            .rsplit('\n')
            .next()
            .map(|s| s.chars().count() + 1)
            .unwrap_or(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_box_drawing() {
        let s = "┌─┐\näöü";
        let bytes = encode_cp437(s);
        let back = decode_cp437(&bytes);
        assert!(back.contains('ä') || back.contains('?'));
        assert_eq!(decode_cp437(&encode_cp437("ABC\n")), "ABC\n");
    }

    #[test]
    fn cut_and_paste_the_cursor_line() {
        let mut ed = GpEditor::default();
        ed.text = "one\ntwo\nthree".into();
        ed.cursor_line = 2;
        assert_eq!(ed.cut_cursor_line(), "two");
        assert_eq!(ed.text, "one\nthree");
        ed.paste_line("two");
        assert_eq!(ed.text, "one\ntwo\nthree");
    }
}
