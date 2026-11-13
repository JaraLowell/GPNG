//! Bottom session strip above the icon toolbar (Monitor / Channel).
//!
//! Classic layout (cells are clickable toggles / dialogs):
//! ```text
//! [ QRG: 27.235 ] [ CT: CTEXT ] [ äöü ] [ /|stair ] [ Echo ] [ ♪ ] [ Clear ]
//!   Monitor extras: [1]…[10] spy
//!   Channel extras: [ 7+ ] [ Remote ] [ BIN-RX ] [ BIN-TX ]
//! [ ScrLock ] [ ▲ ] [ ▼ ] [ B ]
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::SessionChromeConfig;

use super::channel_bits::{
    ChannelBits, MASK_BELL, MASK_ECHO, MASK_REMOTE, MASK_SEVEN_PLUS, MASK_SOFTSCROLL, MASK_UMLAUT,
    SLOT_MONITOR,
};
use super::layout::ActivePane;

pub use super::channel_bits::{is_broadcast, slot_for};

/// How new lines arrive in the RX pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStep {
    /// `/` — the window jumps straight to the newest line.
    Hard,
    /// Stair — the window crawls to the newest line, one pixel at a time.
    Smooth,
}

impl LineStep {
    pub fn from_smooth(smooth: bool) -> Self {
        if smooth {
            Self::Smooth
        } else {
            Self::Hard
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hard => "/",
            // The session bar paints a stair for this. A Unicode stair is a box
            // in the UI font.
            Self::Smooth => "",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionChrome {
    pub qrg_mhz: f64,
    /// QRG as written: up to four decimal places, trailing zero removed.
    /// Empty means format `qrg_mhz`.
    pub qrg_log: String,
    pub ctext_path: PathBuf,
    /// Connect text for a channel. Empty, or no entry, uses `ctext_path`.
    /// Index is the channel number (`1..=10`). Index 0 is unused.
    pub channel_ctext: Vec<PathBuf>,
    /// Per-slot umlaut / CP437 mode ([`MASK_UMLAUT`]).
    pub umlaut: ChannelBits,
    /// Per-slot softscroll ([`MASK_SOFTSCROLL`]).
    pub softscroll: ChannelBits,
    /// Per-slot echo ([`MASK_ECHO`]).
    pub echo: ChannelBits,
    /// Per-channel ♪ — connect, disconnect, BEL, newline ([`MASK_BELL`]).
    pub bell: ChannelBits,
    /// Per-channel 7+ AutoSave ([`MASK_SEVEN_PLUS`]).
    pub seven_plus: ChannelBits,
    /// Per-channel Remote ([`MASK_REMOTE`]).
    pub remote: ChannelBits,
    pub bin_rx: bool,
    pub bin_tx: bool,
    /// On: freeze viewport (classic Scroll Lock). Off: always follow new lines.
    pub scr_lock: bool,
    /// Clock ♫ hourly chime (ALT-G).
    pub hour_gong: bool,
    /// Show the RX vertical scrollbar.
    pub scrollbar: bool,
    /// Active spy slot on Monitor (`1..=10`), if any.
    pub spy_slot: Option<u8>,
    pub show_qrg_dialog: bool,
    pub qrg_edit: String,
    /// MYCALL popup (same style as QRG).
    pub show_mycall_dialog: bool,
    pub mycall_edit: String,
    /// `0` = monitor / global; `1..=N` = channel.
    pub mycall_slot: u8,
    /// Set spy callsign popup.
    pub show_spy_dialog: bool,
    pub spy_edit: String,
    /// Connect-to popup (`CALL` or `CALL,VIA[,VIA…]`).
    pub show_connect_dialog: bool,
    pub connect_edit: String,
    /// Channel that will run the connect (`1..=N`).
    pub connect_channel: u8,
    /// One-shot scroll nudge in pixels (negative = up).
    pub scroll_nudge: f32,
}

impl Default for SessionChrome {
    fn default() -> Self {
        Self {
            qrg_mhz: 27.235,
            qrg_log: String::new(),
            ctext_path: PathBuf::from("CTEXT.GPI"),
            channel_ctext: Vec::new(),
            umlaut: ChannelBits(0),
            softscroll: ChannelBits(0),
            // Classic default — show local TX on every channel + Monitor.
            echo: ChannelBits(MASK_ECHO),
            bell: ChannelBits(MASK_BELL),
            seven_plus: ChannelBits(0),
            remote: ChannelBits(0),
            bin_rx: false,
            bin_tx: false,
            scr_lock: true,
            hour_gong: true,
            scrollbar: true,
            spy_slot: None,
            show_qrg_dialog: false,
            qrg_edit: String::new(),
            show_mycall_dialog: false,
            mycall_edit: String::new(),
            mycall_slot: 0,
            show_spy_dialog: false,
            spy_edit: String::new(),
            show_connect_dialog: false,
            connect_edit: String::new(),
            connect_channel: 1,
            scroll_nudge: 0.0,
        }
    }
}

impl SessionChrome {
    /// Apply persisted `[session.chrome]` values (empty `ctext` → default CTEXT).
    pub fn from_config(chrome: &SessionChromeConfig) -> Self {
        let mut s = Self::default();
        s.qrg_mhz = chrome.qrg_mhz;
        s.qrg_log = crate::journal::format_mhz(chrome.qrg_mhz);
        if !chrome.ctext.trim().is_empty() {
            s.ctext_path = PathBuf::from(&chrome.ctext);
        } else {
            s.ctext_path = default_ctext_path();
        }
        s.channel_ctext = chrome.channel_ctext.iter().map(PathBuf::from).collect();
        s.umlaut = ChannelBits(chrome.umlaut).clamp(MASK_UMLAUT);
        s.softscroll = ChannelBits(chrome.softscroll).clamp(MASK_SOFTSCROLL);
        s.echo = ChannelBits(chrome.echo).clamp(MASK_ECHO);
        // Legacy `channel_echo` overrides (index = channel). `-1` / missing = leave bit.
        for (idx, code) in chrome.channel_echo.iter().enumerate() {
            if idx == 0 || idx > 10 {
                continue;
            }
            match code {
                0 => s.echo.set(idx as u8, false),
                1 => s.echo.set(idx as u8, true),
                _ => {}
            }
        }
        s.bell = ChannelBits(chrome.bell).clamp(MASK_BELL);
        s.seven_plus = ChannelBits(chrome.seven_plus).clamp(MASK_SEVEN_PLUS);
        s.remote = ChannelBits(chrome.remote).clamp(MASK_REMOTE);
        s.scr_lock = chrome.scr_lock;
        s.hour_gong = chrome.hour_gong;
        s
    }

    /// Snapshot for writing back to Config.toml.
    pub fn to_config(&self) -> SessionChromeConfig {
        SessionChromeConfig {
            qrg_mhz: self.qrg_mhz,
            ctext: self.ctext_path.to_string_lossy().into_owned(),
            channel_ctext: self
                .channel_ctext
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            umlaut: self.umlaut.clamp(MASK_UMLAUT).raw(),
            softscroll: self.softscroll.clamp(MASK_SOFTSCROLL).raw(),
            echo: self.echo.clamp(MASK_ECHO).raw(),
            channel_echo: Vec::new(),
            bell: self.bell.clamp(MASK_BELL).raw(),
            seven_plus: self.seven_plus.clamp(MASK_SEVEN_PLUS).raw(),
            remote: self.remote.clamp(MASK_REMOTE).raw(),
            scr_lock: self.scr_lock,
            hour_gong: self.hour_gong,
        }
    }

    pub fn umlaut_for(&self, slot: u8) -> bool {
        self.umlaut.get(slot)
    }

    pub fn softscroll_for(&self, slot: u8) -> bool {
        self.softscroll.get(slot)
    }

    pub fn line_step_for(&self, slot: u8) -> LineStep {
        LineStep::from_smooth(self.softscroll_for(slot))
    }

    pub fn echo_for(&self, slot: u8) -> bool {
        self.echo.get(slot)
    }

    pub fn bell_for(&self, slot: u8) -> bool {
        self.bell.get(slot)
    }

    pub fn seven_plus_for(&self, slot: u8) -> bool {
        self.seven_plus.get(slot)
    }

    pub fn remote_for(&self, slot: u8) -> bool {
        self.remote.get(slot)
    }

    /// Session-bar / setup lit state (Monitor = all apply bits on).
    pub fn umlaut_lit(&self, pane: ActivePane) -> bool {
        self.umlaut.display(pane, MASK_UMLAUT)
    }

    pub fn softscroll_lit(&self, pane: ActivePane) -> bool {
        self.softscroll.display(pane, MASK_SOFTSCROLL)
    }

    pub fn echo_lit(&self, pane: ActivePane) -> bool {
        self.echo.display(pane, MASK_ECHO)
    }

    pub fn bell_lit(&self, pane: ActivePane) -> bool {
        self.bell.display(pane, MASK_BELL)
    }

    pub fn seven_plus_lit(&self, pane: ActivePane) -> bool {
        self.seven_plus.display(pane, MASK_SEVEN_PLUS)
    }

    pub fn remote_lit(&self, pane: ActivePane) -> bool {
        self.remote.display(pane, MASK_REMOTE)
    }

    /// Toggle a bitfield setting for `pane`. Monitor broadcasts across `mask`.
    pub fn toggle_bits(bits: &mut ChannelBits, pane: ActivePane, mask: u32) -> bool {
        let slot = slot_for(pane);
        if is_broadcast(pane) {
            bits.toggle(SLOT_MONITOR, mask)
        } else {
            bits.toggle(slot, mask)
        }
    }

    pub fn toggle_umlaut(&mut self, pane: ActivePane) -> bool {
        Self::toggle_bits(&mut self.umlaut, pane, MASK_UMLAUT)
    }

    pub fn toggle_softscroll(&mut self, pane: ActivePane) -> bool {
        Self::toggle_bits(&mut self.softscroll, pane, MASK_SOFTSCROLL)
    }

    pub fn toggle_echo(&mut self, pane: ActivePane) -> bool {
        Self::toggle_bits(&mut self.echo, pane, MASK_ECHO)
    }

    pub fn toggle_bell(&mut self, pane: ActivePane) -> bool {
        Self::toggle_bits(&mut self.bell, pane, MASK_BELL)
    }

    pub fn toggle_seven_plus(&mut self, pane: ActivePane) -> bool {
        Self::toggle_bits(&mut self.seven_plus, pane, MASK_SEVEN_PLUS)
    }

    pub fn toggle_remote(&mut self, pane: ActivePane) -> bool {
        Self::toggle_bits(&mut self.remote, pane, MASK_REMOTE)
    }

    pub fn set_seven_plus_for(&mut self, channel: u8, on: bool) {
        if channel == SLOT_MONITOR {
            self.seven_plus.set_masked(MASK_SEVEN_PLUS, on);
        } else {
            self.seven_plus.set(channel, on);
        }
    }

    pub fn qrg_text(&self) -> String {
        if self.qrg_log.trim().is_empty() {
            crate::journal::format_mhz(self.qrg_mhz)
        } else {
            self.qrg_log.clone()
        }
    }

    pub fn qrg_label(&self) -> String {
        format!("QRG: {}", self.qrg_text())
    }

    pub fn ctext_label(&self) -> String {
        self.ctext_label_for(0)
    }

    /// File this screen sends. A channel with its own path uses that.
    /// Monitor, and a channel with no path, use the global file.
    pub fn ctext_for(&self, channel: u8) -> &Path {
        if let Some(path) = self.channel_ctext_path(channel) {
            path
        } else {
            &self.ctext_path
        }
    }

    /// Path stored on this channel, when the channel does not inherit global.
    pub fn channel_ctext_path(&self, channel: u8) -> Option<&Path> {
        if channel == 0 {
            return None;
        }
        self.channel_ctext.get(channel as usize).and_then(|path| {
            if path.as_os_str().is_empty() {
                None
            } else {
                Some(path.as_path())
            }
        })
    }

    pub fn set_channel_ctext(&mut self, channel: u8, path: PathBuf) {
        if channel == 0 {
            self.ctext_path = path;
            return;
        }
        // The same file as the Monitor choice is global mode again.
        let path = if same_ctext_file(&path, &self.ctext_path) {
            PathBuf::new()
        } else {
            path
        };
        let idx = channel as usize;
        if self.channel_ctext.len() <= idx {
            self.channel_ctext.resize(idx + 1, PathBuf::new());
        }
        self.channel_ctext[idx] = path;
    }

    pub fn ctext_label_for(&self, channel: u8) -> String {
        let name = self
            .ctext_for(channel)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("CTEXT");
        format!("CT: {name}")
    }

    pub fn ctext_file_name_for(&self, channel: u8) -> String {
        self.ctext_for(channel)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("CTEXT.GPI")
            .to_string()
    }

    pub fn set_qrg_from_edit(&mut self) -> Result<(), String> {
        let text = crate::journal::normalize_qrg(&self.qrg_edit)?;
        self.qrg_mhz = text.parse().unwrap_or(0.0);
        self.qrg_log = text.clone();
        self.qrg_edit = text;
        self.show_qrg_dialog = false;
        Ok(())
    }

    pub fn open_qrg_dialog(&mut self) {
        self.qrg_edit = self.qrg_text();
        self.show_qrg_dialog = true;
    }

    pub fn open_mycall_dialog(&mut self, slot: u8, current: &str) {
        self.mycall_slot = slot;
        self.mycall_edit = current.to_string();
        self.show_mycall_dialog = true;
    }

    pub fn open_spy_dialog(&mut self, slot: u8, current: &str) {
        self.spy_slot = Some(slot);
        self.spy_edit = current.to_string();
        self.show_spy_dialog = true;
    }

    pub fn open_connect_dialog(&mut self, channel: u8, prefill: &str) {
        self.connect_channel = channel;
        self.connect_edit = prefill.to_string();
        self.show_connect_dialog = true;
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
        // One monospace row in the RX pane (`padlog::PAD_FONT_SIZE`).
        self.scroll_nudge += lines as f32 * -14.0;
    }

    pub fn take_scroll_nudge(&mut self) -> f32 {
        let n = self.scroll_nudge;
        self.scroll_nudge = 0.0;
        n
    }
}

fn same_ctext_file(picked: &Path, global: &Path) -> bool {
    match (picked.file_name(), global.file_name()) {
        (Some(picked), Some(global)) => picked.eq_ignore_ascii_case(global),
        _ => false,
    }
}

/// Resolve connect-text GPI (`data/CTEXT.GPI`).
pub fn default_ctext_path() -> PathBuf {
    crate::remote::gpi::default_ctext_path()
}

pub fn default_bell_wav() -> PathBuf {
    asset_wav("BELL.WAV")
}

/// Hourly clock chime (`data/CHIME.WAV`). Not the `//BELL` sound.
pub fn default_chime_wav() -> PathBuf {
    asset_wav("CHIME.WAV")
}

pub fn default_connect_wav() -> PathBuf {
    asset_wav("CONNECT.WAV")
}

/// Filename on disk is `DISCONECT.WAV` (classic spelling).
pub fn default_disconnect_wav() -> PathBuf {
    asset_wav("DISCONECT.WAV")
}

/// Channel chat tick (RX or local echo). Not used on the Monitor.
pub fn default_newline_wav() -> PathBuf {
    asset_wav("NEWLINE.WAV")
}

fn asset_wav(name: &str) -> PathBuf {
    let in_data = crate::paths::data_file(name);
    if in_data.is_file() {
        return in_data;
    }
    for dir in [
        PathBuf::from("assets"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
    ] {
        if let Some(path) = crate::paths::find_named_file(&dir, name) {
            return path;
        }
    }
    in_data
}

/// Most copies one play may ask for. A 12-hour chime is the longest.
const WAV_REPEAT_MAX: u32 = 12;

/// Strikes for a 12-hour clock. Midnight and noon are 12. 15:00 is 3.
pub fn chime_strikes(hour24: u32) -> u32 {
    match hour24 % 12 {
        0 => 12,
        n => n,
    }
}
/// `RXCLICK` plays this many times. `0` is off. Graphic Packet's default is 2.
pub const RX_CLICK_MAX: u32 = 2;
/// `CONNECTBELL` plays this many times. `0` is off.
pub const CONNECT_BELL_MAX: u32 = 4;

pub fn rx_click_plays(n: u32) -> u32 {
    n.min(RX_CLICK_MAX)
}

pub fn connect_bell_plays(n: u32) -> u32 {
    n.min(CONNECT_BELL_MAX)
}

/// Disconnect follows the connect bell. Off means no `DISCONECT.WAV` either.
pub fn disconnect_plays(connect_bell: u32) -> bool {
    connect_bell_plays(connect_bell) > 0
}

/// Best-effort WAV play on a background thread.
pub fn play_bell_wav(path: &Path) {
    play_wav(path);
}

pub fn play_wav(path: &Path) {
    play_wav_at(path, 1.0);
}

/// Play `times` copies on one background thread. `gap` is the pause after
/// each copy except the last. `0` plays nothing.
pub fn play_wav_times(path: &Path, times: u32, gap: Duration, volume: f32) {
    let times = times.min(WAV_REPEAT_MAX);
    if times == 0 {
        return;
    }
    let path = path.to_path_buf();
    let volume = volume.clamp(0.0, 1.0);
    std::thread::spawn(move || {
        for n in 0..times {
            if n > 0 {
                std::thread::sleep(gap);
            }
            if let Err(e) = play_wav_inner(&path, volume) {
                tracing::debug!("wav {}: {e}", path.display());
                break;
            }
        }
    });
}

/// `volume` is 0.0..=1.0 (rodio sink gain).
pub fn play_wav_at(path: &Path, volume: f32) {
    let path = path.to_path_buf();
    let volume = volume.clamp(0.0, 1.0);
    std::thread::spawn(move || {
        if let Err(e) = play_wav_inner(&path, volume) {
            tracing::debug!("wav {}: {e}", path.display());
        }
    });
}

fn play_wav_inner(path: &Path, volume: f32) -> Result<(), String> {
    use rodio::{Decoder, OutputStream, Sink};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let source = Decoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let (_stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
    let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
    sink.set_volume(volume);
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
    raw.iter()
        .map(|&b| char::from_u32(b as u32).unwrap_or('?'))
        .collect()
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
