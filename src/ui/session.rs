//! Bottom session strip above the icon toolbar (Monitor / Channel).
//!
//! Classic layout (cells are clickable toggles / dialogs):
//! ```text
//! [ QRG: 27.235 ] [ CT: CTEXT ] [ äöü ] [ /|stairs ] [ Echo ] [ ♪ ]
//!   Monitor extras: [1]…[10] spy
//!   Channel extras: [ 7+ ] [ Remote ] [ BIN-RX ] [ BIN-TX ]
//! [ ScrLock ] [ ▲ ] [ ▼ ] [ B ]
//! ```

use std::path::{Path, PathBuf};

/// How new lines arrive in the RX pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStep {
    /// Hard jump — one whole text row per line.
    Hard,
    /// Smooth / soft scroll feel (stairs glyph in classic GP).
    Smooth,
}

impl LineStep {
    pub fn cycle(self) -> Self {
        match self {
            Self::Hard => Self::Smooth,
            Self::Smooth => Self::Hard,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hard => "/",
            Self::Smooth => "≣", // stairs-like
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionChrome {
    pub qrg_mhz: f64,
    pub ctext_path: PathBuf,
    /// `false` = plain ASCII monitor; `true` = extended DOS (CP437 / äöü).
    pub extended_ascii: bool,
    pub line_step: LineStep,
    pub echo: bool,
    pub bell: bool,
    pub seven_plus: bool,
    pub remote: bool,
    pub bin_rx: bool,
    pub bin_tx: bool,
    /// On: stick RX to end on new traffic. Off: keep viewport; use ▲▼.
    pub scr_lock: bool,
    /// Show the RX vertical scrollbar.
    pub scrollbar: bool,
    /// Active spy slot on Monitor (`1..=10`), if any.
    pub spy_slot: Option<u8>,
    pub show_qrg_dialog: bool,
    pub qrg_edit: String,
    /// One-shot scroll nudge in pixels (negative = up).
    pub scroll_nudge: f32,
}

impl Default for SessionChrome {
    fn default() -> Self {
        Self {
            qrg_mhz: 27.235,
            ctext_path: PathBuf::from("CTEXT.GPI"),
            extended_ascii: false,
            line_step: LineStep::Hard,
            echo: false,
            bell: true,
            seven_plus: false,
            remote: false,
            bin_rx: false,
            bin_tx: false,
            scr_lock: true,
            scrollbar: true,
            spy_slot: None,
            show_qrg_dialog: false,
            qrg_edit: String::new(),
            scroll_nudge: 0.0,
        }
    }
}

impl SessionChrome {
    pub fn qrg_label(&self) -> String {
        format!("QRG: {:.3}", self.qrg_mhz)
    }

    pub fn ctext_label(&self) -> String {
        let name = self
            .ctext_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("CTEXT");
        format!("CT: {name}")
    }

    pub fn set_qrg_from_edit(&mut self) -> Result<(), String> {
        let t = self.qrg_edit.trim().replace(',', ".");
        let v: f64 = t
            .parse()
            .map_err(|_| format!("invalid QRG '{t}'"))?;
        if !(0.0..=1000.0).contains(&v) {
            return Err("QRG out of range".into());
        }
        self.qrg_mhz = (v * 1000.0).round() / 1000.0;
        self.show_qrg_dialog = false;
        Ok(())
    }

    pub fn open_qrg_dialog(&mut self) {
        self.qrg_edit = format!("{:.3}", self.qrg_mhz);
        self.show_qrg_dialog = true;
    }

    pub fn set_ctext_path(&mut self, path: PathBuf) {
        self.ctext_path = path;
    }

    pub fn ctext_display_name(&self) -> String {
        self.ctext_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("CTEXT.GPI")
            .to_string()
    }

    pub fn nudge_lines(&mut self, lines: i32) {
        // ~14 px monospace row in the RX pane.
        self.scroll_nudge += lines as f32 * -14.0;
    }

    pub fn take_scroll_nudge(&mut self) -> f32 {
        let n = self.scroll_nudge;
        self.scroll_nudge = 0.0;
        n
    }
}

/// Resolve connect-text GPI (`tools/GTEXT.GPI` preferred, else `CTEXT.GPI`).
pub fn default_ctext_path() -> PathBuf {
    crate::remote::gpi::default_gtext_path()
}

pub fn default_bell_wav() -> PathBuf {
    let candidates = [
        PathBuf::from("assets/bell.wav"),
        PathBuf::from("gp-rs/assets/bell.wav"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/bell.wav"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/bell.wav"))
}

/// Best-effort bell: play `assets/bell.wav` on a background thread.
pub fn play_bell_wav(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = play_bell_inner(&path) {
            tracing::debug!("bell: {e}");
        }
    });
}

fn play_bell_inner(path: &Path) -> Result<(), String> {
    use rodio::{Decoder, OutputStream, Sink};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let source = Decoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let (_stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
    let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

/// Map a byte string for the RX pane according to ASCII mode.
pub fn decode_rx_text(raw: &[u8], extended: bool) -> String {
    if !extended {
        return String::from_utf8_lossy(raw)
            .chars()
            .map(|c| if c == '\u{FFFD}' { '?' } else { c })
            .collect();
    }
    // DOS CP437-ish: keep high bytes as Latin-1 so äöü/box-draw show up.
    raw.iter().map(|&b| char::from_u32(b as u32).unwrap_or('?')).collect()
}

/// Compact hex for debug RX lines (max `max` bytes).
pub fn format_hex_preview(raw: &[u8], max: usize) -> String {
    let take = raw.len().min(max);
    let mut s = raw[..take]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    if raw.len() > max {
        s.push('…');
    }
    s
}

/// Parsed AX.25 UI frame (APRS / unproto) — dest, source, digis, info.
#[derive(Debug, Clone)]
pub struct Ax25UiFrame {
    pub dest: String,
    pub src: String,
    pub via: Vec<String>,
    pub info: String,
}

impl Ax25UiFrame {
    pub fn display_line(&self) -> String {
        let via = if self.via.is_empty() {
            String::new()
        } else {
            format!(",{}", self.via.join(","))
        };
        format!("{}>{}{}: {}", self.src, self.dest, via, self.info)
    }
}

/// Parse a KISS/AX.25 UI payload into addresses + info (APRS-friendly).
pub fn parse_ax25_ui(payload: &[u8]) -> Option<Ax25UiFrame> {
    if payload.len() < 16 {
        return None;
    }
    let mut i = 0usize;
    let mut addrs = Vec::new();
    // Destination, source, then digis until address-extension bit clear.
    loop {
        if i + 7 > payload.len() {
            return None;
        }
        let chunk = &payload[i..i + 7];
        let mut call = String::new();
        for b in &chunk[..6] {
            let c = (*b >> 1) as char;
            if c != ' ' && c.is_ascii_graphic() {
                call.push(c);
            }
        }
        if call.is_empty() {
            return None;
        }
        let ssid = (chunk[6] >> 1) & 0x0f;
        let last = chunk[6] & 0x01 != 0;
        if ssid > 0 {
            call.push_str(&format!("-{ssid}"));
        }
        addrs.push(call);
        i += 7;
        if last || addrs.len() >= 10 {
            break;
        }
    }
    if addrs.len() < 2 {
        return None;
    }
    // Control + PID (UI + no-layer3 is common for APRS: 0x03 0xF0).
    if i + 2 > payload.len() {
        return None;
    }
    let control = payload[i];
    let _pid = payload[i + 1];
    i += 2;
    // UI frame: control & 0xef == 0x03 (with/without poll bit).
    if control & 0xef != 0x03 {
        return None;
    }
    let info = decode_rx_text(&payload[i..], false)
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\r' {
                '.'
            } else {
                c
            }
        })
        .collect::<String>();
    Some(Ax25UiFrame {
        dest: addrs[0].clone(),
        src: addrs[1].clone(),
        via: addrs[2..].to_vec(),
        info,
    })
}

/// Best-effort AX.25 UI-frame summary for KISS/APRS monitor lines.
pub fn format_ax25_ui(payload: &[u8]) -> Option<String> {
    parse_ax25_ui(payload).map(|f| f.display_line())
}
