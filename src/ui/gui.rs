//! Pixel UI (egui) — `ICONS.GP` bitmaps + analog clock, classic GP chrome.
//!
//! Layout matched to V1.63 screenshots + operator notes:
//! ```text
//! ┌── purple pre-write (history + 3-line compose) ──┬── analog clock ──┐
//! ├── channel grid (2×6) ───────────────────────────┤   (tall: spans   │
//! ├── magenta status grid (2 rows, non-clickable) ──┤    all three)    │
//! ├─────────────────────────────────────────────────┴──────────────────┤
//! │ full-width content                                                  │
//! │ session strip: QRG CT äöü / Echo ♪ Clear [spy|7+/Remote/BIN] ScrLock ▲▼ B │
//! │ icons evenly spaced (set depends on pane)                           │
//! ```
//!
//! Magenta is **not** one text line: left title cell is 2 rows high; right
//! side has Mode/Buffers on row 1 and Packets/Mycall on row 2 (Monitor).
//! Buffers, Not disp, Not sent, Tries, and Unack come from the modem driver.

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, Timelike};
use eframe::egui::{
    self, Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId, Frame, Pos2,
    Rect, RichText, Sense, Stroke, TextWrapMode, TextureHandle, TextureOptions, Ui, Vec2,
};
use eframe::egui::text::LayoutJob;
use tokio::sync::mpsc;

use crate::config::{Config, HostProtocol};
use crate::remote::commands::{parse_line, RemoteAction};
use crate::remote::gpi;
use crate::remote::macros::MacroContext;
use crate::tnc::ax25::{self, Ax25Frame, Ax25FrameType};
use crate::tnc::{
    pack_text_payloads, payload_lines, BufferReport, ChannelBuffers, HostCode, TncCommand,
    TncEvent, TNC_RAM,
};
use crate::version;

use super::ansi::{strip_ansi, AnsiDecoder, AnsiStyle};
use super::editor::GpEditor;
use super::icons::{
    toolbar_for, toolbar_for_channel, IconLibrary, ICON_HEIGHT, ICON_SLOT_LABELS, ICON_WIDTH,
};
use super::layout::ActivePane;
use super::mheard::MheardList;
use super::padlog::{self, PadKind, TextPad};
use super::session::{
    decode_rx_text, default_bell_wav, default_connect_wav, default_disconnect_wav,
    format_hex_preview, parse_ax25_ui, play_bell_wav, play_wav, Ax25UiFrame, SessionChrome,
};
use super::theme::vga;

const TYPED_HISTORY: usize = 10;
/// KISS T3: idle link check (RR poll) after this much silence.
const AX25_T3_IDLE: Duration = Duration::from_secs(5 * 60);
/// KISS T1: wait for RR/I after a poll before retry / drop.
const AX25_T1_POLL: Duration = Duration::from_secs(30);
/// Retries of RR poll before link timeout disconnect.
const AX25_POLL_RETRIES: u8 = 3;
/// KISS SABM: wait for UA before resend.
const AX25_SABM_RETRY: Duration = Duration::from_secs(5);
/// Extra SABM sends after the first (then give up — no DISC on air).
const AX25_SABM_RETRIES: u8 = 3;
const COMPOSE_ROWS: usize = 3;

/// Classic VGA chrome (from GP V1.63 / DOSBox reference).
const VGA_BLUE: Color32 = Color32::from_rgb(0, 0, 170);
const VGA_DARK_GREEN: Color32 = Color32::from_rgb(0, 85, 0);
const VGA_GREEN: Color32 = Color32::from_rgb(0, 170, 0);
const VGA_DARK_RED: Color32 = Color32::from_rgb(170, 0, 0);
const VGA_GRAY: Color32 = Color32::from_rgb(170, 170, 170);
const VGA_DARK_GRAY: Color32 = Color32::from_rgb(85, 85, 85);
/// Classic GP text-window colours (Monitor / Channel / Spy pads).

/// Classic DOS / BGI 3D bevel: raised = light-grey top+left, black bottom+right;
/// pressed (sunken) = reversed.
fn paint_bevel(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let hi_face = Color32::from_rgb(200, 200, 200); // soft highlight (not hard white)
    let (hi, lo) = if pressed {
        (Color32::BLACK, hi_face)
    } else {
        (hi_face, Color32::BLACK)
    };
    // egui Rect max edge is exclusive — keep the stroke inside the filled area.
    let x0 = rect.left();
    let y0 = rect.top();
    let x1 = rect.right() - 1.0;
    let y1 = rect.bottom() - 1.0;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let tl = Pos2::new(x0, y0);
    let tr = Pos2::new(x1, y0);
    let bl = Pos2::new(x0, y1);
    let br = Pos2::new(x1, y1);
    painter.line_segment([tl, tr], Stroke::new(1.0, hi));
    painter.line_segment([tl, bl], Stroke::new(1.0, hi));
    painter.line_segment([bl, br], Stroke::new(1.0, lo));
    painter.line_segment([tr, br], Stroke::new(1.0, lo));
}

fn egui_color(name: &str) -> Color32 {
    match vga(name) {
        ratatui::style::Color::Black => Color32::BLACK,
        ratatui::style::Color::Blue => Color32::from_rgb(0, 0, 170),
        ratatui::style::Color::Green => Color32::from_rgb(0, 170, 0),
        ratatui::style::Color::Cyan => Color32::from_rgb(0, 170, 170),
        ratatui::style::Color::Red => Color32::from_rgb(170, 0, 0),
        ratatui::style::Color::Magenta => Color32::from_rgb(170, 0, 170),
        ratatui::style::Color::Yellow => Color32::from_rgb(170, 170, 0),
        ratatui::style::Color::Gray => Color32::from_rgb(170, 170, 170),
        ratatui::style::Color::DarkGray => Color32::from_rgb(85, 85, 85),
        ratatui::style::Color::LightBlue => Color32::from_rgb(85, 85, 255),
        ratatui::style::Color::LightGreen => Color32::from_rgb(85, 255, 85),
        ratatui::style::Color::LightCyan => Color32::from_rgb(85, 255, 255),
        ratatui::style::Color::LightRed => Color32::from_rgb(255, 85, 85),
        ratatui::style::Color::LightMagenta => Color32::from_rgb(255, 85, 255),
        ratatui::style::Color::White => Color32::WHITE,
        ratatui::style::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        _ => Color32::WHITE,
    }
}


pub struct GpGuiApp {
    cfg: Config,
    /// Path used to persist MYCALL (and later settings) into Config.toml.
    config_path: PathBuf,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
    active: ActivePane,
    status: String,
    /// Current compose buffer (shown in a 3-line editor).
    input: String,
    /// Last typed lines (pre-write history), newest at the end.
    typed_history: VecDeque<String>,
    pads: Vec<TextPad>,
    /// Spy windows `1..=10` (index 0 = spy 1).
    spy_pads: Vec<TextPad>,
    /// BBS-Menu pad (`ActivePane::Check`).
    bbs_pad: TextPad,
    /// DieBox-List pad (`ActivePane::List`).
    box_pad: TextPad,
    /// Soft event / command log (mode changes, etc.).
    event_log: VecDeque<String>,
    channels: u8,
    /// Peer callsign per channel index (`1..=channels`); `None` = disconnected.
    channel_peer: Vec<Option<String>>,
    /// In-progress connect target per channel (not linked yet).
    channel_connecting: Vec<Option<String>>,
    /// Self-connect in flight: `(caller_ch, answer_ch)`.
    self_connect_pair: Option<(u8, u8)>,
    /// Self-connect still linked (until either side disconnects).
    self_connect_live: Option<(u8, u8)>,
    /// Ephemeral on-air SSID for self-connect / busy redirect (not Config.toml).
    /// Channel numbers / `-n` are internal unless the user sets a slot MYCALL.
    temp_slot_call: Vec<Option<String>>,
    /// KISS AX.25 send sequence N(S) per channel (`0` unused).
    ax25_vs: Vec<u8>,
    /// KISS AX.25 receive sequence N(R) per channel (`0` unused).
    ax25_vr: Vec<u8>,
    /// Last I/S/U activity on a linked KISS channel (T3 idle clock).
    ax25_alive_at: Vec<Option<Instant>>,
    /// Outstanding RR poll (T1) — `None` = not waiting.
    ax25_poll_at: Vec<Option<Instant>>,
    /// RR poll attempts since last peer reply.
    ax25_poll_tries: Vec<u8>,
    /// Last SABM send time while connecting (awaiting UA).
    ax25_sabm_at: Vec<Option<Instant>>,
    /// SABMs sent for this connect attempt (1 = first).
    ax25_sabm_tries: Vec<u8>,
    /// Focus the connect dialog call field once when opened.
    connect_dialog_focus: bool,
    icon_tex: Vec<TextureHandle>,
    /// WA8DED monitor flags, e.g. `IUSC` (from TNC / local set).
    monitor_mode: String,
    /// Free modem RAM, from the driver (`@B`, or 600 minus bytes still queued).
    tnc_free: u32,
    /// Receive frames the modem has not handed up yet (monitor row).
    mon_not_disp: u32,
    ch_not_disp: Vec<u32>,
    ch_not_sent: Vec<u32>,
    ch_tries: Vec<u32>,
    ch_unack: Vec<u32>,
    /// Open terminal row on a channel. `None` means the next byte starts a row.
    rx_row: Vec<Option<u64>>,
    /// Previous packet ended on CR; the next LF is the same newline.
    rx_cr: Vec<bool>,
    /// Colour pen for the channel. A packet cut does not reset it.
    rx_ansi: Vec<AnsiDecoder>,
    /// Serial device for the session-bar badge (`COM6`).
    serial_port: String,
    /// Serial baud for the session-bar badge.
    serial_baud: u32,
    /// UART link up (green COM badge when open).
    serial_open: bool,
    /// Raw UART bytes seen since start (proves RX even without frames).
    serial_rx_bytes: u64,
    /// Decoded hostmode/KISS frames since start.
    serial_rx_frames: u64,
    monitor_bg: Color32,
    #[allow(dead_code)] // per-line PadKind colours replace flat pane fg
    monitor_fg: Color32,
    state_bg: Color32,
    state_fg: Color32,
    input_bg: Color32,
    #[allow(dead_code)]
    input_fg: Color32,
    connect_bg: Color32,
    #[allow(dead_code)]
    connect_fg: Color32,
    session: SessionChrome,
    /// Internal GPI / text editor (EDIT icon → default GTEXT.GPI).
    editor: GpEditor,
    /// Stations heard (APRS/AX.25 UI) — JSON-backed.
    mheard: MheardList,
    /// Selected row in the MHeard list (`None` = none).
    mheard_selected: Option<usize>,
    /// After SPY on MHeard: next list pick opens the confirm dialog.
    mheard_spy_pick: bool,
    /// Confirm dialog: Start Spy for {call}?
    show_mheard_spy_confirm: bool,
    mheard_spy_pending: String,
    bell_wav: PathBuf,
    connect_wav: PathBuf,
    disconnect_wav: PathBuf,
    /// Hourly gong (classic ALT-G / click ♫ on the clock).
    hour_gong: bool,
    /// Prevents double-fire within the same hour.
    last_gong: Option<(chrono::NaiveDate, u32)>,
}

impl GpGuiApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        config_path: PathBuf,
        events: mpsc::Receiver<TncEvent>,
        cmd_tx: mpsc::Sender<TncCommand>,
    ) -> Self {
        let channels = cfg.session.channels.clamp(1, 10) as u8;
        let mon_max = cfg.buffers.text_buffer.first().copied().unwrap_or(600) as usize;
        let ch_max = cfg
            .buffers
            .text_buffer
            .get(1)
            .copied()
            .unwrap_or(300) as usize;
        let spy_max = cfg.buffers.spy_buffer.max(1) as usize;
        let bbs_max = cfg.buffers.check_buffer.max(1) as usize;
        let box_max = cfg.buffers.list_buffer.max(1) as usize;
        let log_dir = cfg.paths.log_dir.clone();

        let mut pads = Vec::with_capacity(channels as usize + 1);
        let mut channel_peer = Vec::with_capacity(channels as usize + 1);
        let mut channel_connecting = Vec::with_capacity(channels as usize + 1);
        let mut temp_slot_call = Vec::with_capacity(channels as usize + 1);
        let mut ax25_vs = Vec::with_capacity(channels as usize + 1);
        let mut ax25_vr = Vec::with_capacity(channels as usize + 1);
        let mut ax25_alive_at = Vec::with_capacity(channels as usize + 1);
        let mut ax25_poll_at = Vec::with_capacity(channels as usize + 1);
        let mut ax25_poll_tries = Vec::with_capacity(channels as usize + 1);
        let mut ax25_sabm_at = Vec::with_capacity(channels as usize + 1);
        let mut ax25_sabm_tries = Vec::with_capacity(channels as usize + 1);
        for n in 0..=channels {
            let max = if n == 0 { mon_max } else { ch_max };
            let mut pad = TextPad::new(max);
            if n == 0 {
                pad.load_monitor_log(&padlog::monitor_path(&log_dir));
            } else {
                pad.load_log(&padlog::channel_path(&log_dir, n));
            }
            pads.push(pad);
            channel_peer.push(None);
            channel_connecting.push(None);
            temp_slot_call.push(None);
            ax25_vs.push(0);
            ax25_vr.push(0);
            ax25_alive_at.push(None);
            ax25_poll_at.push(None);
            ax25_poll_tries.push(0);
            ax25_sabm_at.push(None);
            ax25_sabm_tries.push(0);
        }
        // Session banner (orange) — not persisted.
        pads[0].push(PadKind::System, version::startup_credits());
        pads[0].push(
            PadKind::System,
            version::startup_local_call(cfg.primary_mycall()),
        );

        let spy_pads: Vec<TextPad> = (1..=10)
            .map(|slot| {
                let mut pad = TextPad::new(spy_max);
                pad.load_log(&padlog::spy_path(&log_dir, slot));
                pad
            })
            .collect();

        let mut bbs_pad = TextPad::new(bbs_max);
        bbs_pad.load_log(&padlog::bbs_path(&log_dir));
        let mut box_pad = TextPad::new(box_max);
        box_pad.load_log(&padlog::box_path(&log_dir));

        let slots = channels as usize + 1;
        install_console_font(&cc.egui_ctx);
        let icon_tex = load_icon_textures(&cc.egui_ctx);
        let status = if icon_tex.is_empty() {
            "data/ICONS.GP not found".into()
        } else {
            format!("{} icons", icon_tex.len())
        };

        let colors = &cfg.colors;
        let (serial_port, serial_baud) = if cfg.tnc.kiss_use_tcp() {
            (cfg.tnc.kiss_tcp_addr(), 0)
        } else {
            (cfg.serial.port.clone(), cfg.serial.baud)
        };
        let monitor_mode = if cfg.tnc.monitor_mode.is_empty() {
            "IUSC".into()
        } else {
            cfg.tnc.monitor_mode.clone()
        };
        let session = SessionChrome::from_config(&cfg.session.chrome);
        Self {
            monitor_bg: egui_color(&colors.monitor_background),
            monitor_fg: egui_color(&colors.monitor_foreground),
            state_bg: egui_color(&colors.state_background),
            state_fg: egui_color(&colors.state_foreground),
            input_bg: egui_color(&colors.input_background),
            input_fg: egui_color(&colors.input_foreground),
            connect_bg: egui_color(&colors.connect_background),
            connect_fg: egui_color(&colors.connect_foreground),
            cfg,
            config_path,
            events,
            cmd_tx,
            active: ActivePane::Monitor,
            status,
            input: String::new(),
            typed_history: VecDeque::new(),
            pads,
            spy_pads,
            bbs_pad,
            box_pad,
            event_log: VecDeque::new(),
            channels,
            channel_peer,
            channel_connecting,
            self_connect_pair: None,
            self_connect_live: None,
            temp_slot_call,
            ax25_vs,
            ax25_vr,
            ax25_alive_at,
            ax25_poll_at,
            ax25_poll_tries,
            ax25_sabm_at,
            ax25_sabm_tries,
            connect_dialog_focus: false,
            icon_tex,
            monitor_mode,
            tnc_free: TNC_RAM,
            mon_not_disp: 0,
            ch_not_disp: vec![0; slots],
            ch_not_sent: vec![0; slots],
            ch_tries: vec![0; slots],
            ch_unack: vec![0; slots],
            rx_row: vec![None; slots],
            rx_cr: vec![false; slots],
            rx_ansi: vec![AnsiDecoder::new(); slots],
            serial_port,
            serial_baud,
            serial_open: false,
            serial_rx_bytes: 0,
            serial_rx_frames: 0,
            session,
            editor: GpEditor::default(),
            mheard: MheardList::load_default(),
            mheard_selected: None,
            mheard_spy_pick: false,
            show_mheard_spy_confirm: false,
            mheard_spy_pending: String::new(),
            bell_wav: default_bell_wav(),
            connect_wav: default_connect_wav(),
            disconnect_wav: default_disconnect_wav(),
            hour_gong: true,
            last_gong: None,
        }
    }

    /// Open Editor on `GTEXT.GPI` (classic default connect-text file).
    fn open_editor_default(&mut self) {
        self.select(ActivePane::Editor);
        let path = gpi::default_gtext_path();
        match self.editor.load_path(&path) {
            Ok(()) => {
                self.status = format!("EDIT {}", self.editor.display_name());
                self.log_event(format!("editor ← {}", self.editor.display_name()));
            }
            Err(e) => {
                self.editor.path = path;
                self.editor.text.clear();
                self.editor.dirty = false;
                self.status = e.clone();
                self.log_event(format!("editor: {e}"));
            }
        }
    }

    fn editor_save(&mut self) {
        match self.editor.save() {
            Ok(()) => {
                self.status = format!("saved {}", self.editor.display_name());
                self.log_event(format!("editor saved {}", self.editor.display_name()));
                // Keep session CT in sync when editing GTEXT.
                if self
                    .editor
                    .display_name()
                    .eq_ignore_ascii_case("GTEXT.GPI")
                {
                    self.session.set_ctext_path(self.editor.path.clone());
                }
            }
            Err(e) => {
                self.status = e.clone();
                self.log_event(format!("editor save: {e}"));
            }
        }
    }

    fn editor_load_dialog(&mut self) {
        let start = crate::paths::data_dir();
        let dialog = rfd::FileDialog::new()
            .add_filter("GPI / text", &["gpi", "GPI", "txt", "TXT"])
            .set_title("Load GPI / text")
            .set_directory(&start);
        if let Some(path) = dialog.pick_file() {
            match self.editor.load_path(&path) {
                Ok(()) => {
                    self.status = format!("loaded {}", self.editor.display_name());
                    self.log_event(format!("editor ← {}", self.editor.display_name()));
                    self.select(ActivePane::Editor);
                }
                Err(e) => {
                    self.status = e.clone();
                    self.log_event(format!("editor load: {e}"));
                }
            }
        }
    }

    fn maybe_hour_gong(&mut self) {
        if !self.hour_gong {
            return;
        }
        let now = Local::now();
        if now.minute() != 0 || now.second() > 1 {
            return;
        }
        let key = (now.date_naive(), now.hour());
        if self.last_gong == Some(key) {
            return;
        }
        self.last_gong = Some(key);
        play_bell_wav(&self.bell_wav);
    }

    fn log_event(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.event_log.push_back(msg.clone());
        while self.event_log.len() > 200 {
            self.event_log.pop_front();
        }
        self.pads[0].push(PadKind::System, msg);
    }

    fn drain_tnc(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                TncEvent::Status(m) => {
                    self.status = m.clone();
                    let low = m.to_ascii_lowercase();
                    // Serial Kenwood / TCP Soundmodem / hostmode bring-up → green COM badge.
                    let link_up = low.starts_with("serial open")
                        || low.starts_with("tcp open")
                        || low.contains("wa8ded driver")
                        || low.contains("tfpcx driver");
                    if link_up {
                        let was_open = self.serial_open;
                        self.serial_open = true;
                        if low.starts_with("serial open") || low.starts_with("tcp open") {
                            self.log_event(m);
                        }
                        if !was_open {
                            self.sync_channel_identities();
                        }
                    }
                }
                TncEvent::Warning(m) => {
                    self.status = m.clone();
                    // Keep real failures; skip routine serial `KISS.INI:` bring-up noise.
                    let low = m.to_ascii_lowercase();
                    if !low.starts_with("kiss.ini") {
                        self.log_event(format!("! {m}"));
                    }
                }
                TncEvent::LinkDown(m) => {
                    self.status = format!("link down: {m}");
                    self.serial_open = false;
                    self.log_event(format!("link down: {m}"));
                    for peer in &mut self.channel_peer {
                        *peer = None;
                    }
                    for c in &mut self.channel_connecting {
                        *c = None;
                    }
                }
                // UART counters only — decoded frames land on the monitor.
                TncEvent::RawRx(bytes) => {
                    self.serial_rx_bytes += bytes.len() as u64;
                }
                TncEvent::Buffers(report) => self.apply_buffers(report),
                TncEvent::HostFrame {
                    channel,
                    code,
                    payload,
                } => {
                    self.serial_rx_frames += 1;
                    if self.session.bell && payload.contains(&0x07) {
                        play_bell_wav(&self.bell_wav);
                    }
                    let text = decode_rx_text(&payload, self.session.extended_ascii);
                    // Heuristic: TNC reply to `M` query often looks like `IUSC` / `UISC`.
                    if channel == 0
                        && matches!(code, HostCode::Data | HostCode::Other(_))
                        && looks_like_monitor_mode(&text)
                    {
                        self.monitor_mode = text.trim().to_ascii_uppercase();
                        self.log_event(format!("monitor mode ← TNC: {}", self.monitor_mode));
                    }
                    // Link status may arrive as LinkStatus *or* plain Monitor/Data text.
                    let linkish = matches!(code, HostCode::LinkStatus)
                        || (matches!(
                            code,
                            HostCode::Monitor | HostCode::Data | HostCode::Other(_)
                        ) && looks_like_link_status(&text));
                    let mut skip_monitor_dup = false;
                    if linkish {
                        let status_ch = if channel == 0 {
                            self.infer_link_status_channel(&text).unwrap_or(0)
                        } else {
                            channel
                        };
                        if status_ch >= 1 {
                            self.apply_link_status(status_ch, &text);
                        }
                        // Modem link chatter on the monitor (not an `fm` header).
                        self.pads[0].push_monitor(PadKind::Own, text.trim().to_string());
                        if channel == 0 {
                            skip_monitor_dup = true;
                        }
                    }
                    let mut spy_src: Option<String> = None;
                    let (kind, line) = match code {
                        HostCode::Ok => continue,
                        HostCode::Monitor => {
                            spy_src = extract_heard_call(&text);
                            (PadKind::Incoming, text)
                        }
                        HostCode::LinkStatus => (PadKind::System, text),
                        HostCode::Data => {
                            // KISS: full AX.25 L2 (UI / SABM / I / …) — not hostmode text.
                            if self.kiss_mode() {
                                if let Some(frame) = ax25::parse_frame(&payload) {
                                    self.handle_kiss_ax25(frame);
                                    continue;
                                }
                            }
                            if let Some(frame) = parse_ax25_ui(&payload) {
                                spy_src = Some(frame.src.clone());
                                self.mheard.hear(&frame);
                                (PadKind::Incoming, frame.display_line())
                            } else if text.chars().any(|c| c.is_ascii_graphic()) {
                                spy_src = extract_heard_call(&text);
                                (PadKind::Incoming, text)
                            } else {
                                (
                                    PadKind::System,
                                    format!(
                                        "DATA ch{channel} {}B  {}",
                                        payload.len(),
                                        format_hex_preview(&payload, 32)
                                    ),
                                )
                            }
                        }
                        HostCode::Other(c) => {
                            (PadKind::System, format!("{c:#04x}  {text}"))
                        }
                    };
                    if let Some(src) = spy_src.as_deref() {
                        self.feed_spy_pads(src, kind, &line);
                    }
                    // KISS port 0 and hostmode ch0 → Monitor; else channel pad.
                    let idx = if channel == 0 {
                        0
                    } else if channel <= self.channels {
                        channel as usize
                    } else {
                        0
                    };
                    // First I-frame / data while still "connecting" → treat as linked
                    // (some firmwares are light on LinkStatus text).
                    if channel >= 1 && matches!(code, HostCode::Data) {
                        self.promote_connecting_on_data(channel);
                    }
                    // Channel text is rebuilt from newlines across host frames.
                    // A 128-byte cut is not a row break. `//` commands wait for
                    // the newline that finishes the line.
                    let reassemble = channel >= 1 && matches!(code, HostCode::Data);
                    if reassemble {
                        self.accept_rx_text(channel, &line, kind);
                    } else if !(skip_monitor_dup && idx == 0) {
                        if let Some(pad) = self.pads.get_mut(idx) {
                            for part in payload_lines(&line) {
                                let shown = if idx == 0 {
                                    stamp_monitor_fm(&strip_ansi(&part))
                                } else {
                                    part
                                };
                                if idx == 0 {
                                    pad.push_monitor(kind, shown);
                                } else {
                                    pad.push(kind, shown);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn select(&mut self, pane: ActivePane) {
        // Leaving Spy → un-light the spy slot button.
        if !matches!(pane, ActivePane::Spy(_)) {
            self.session.spy_slot = None;
        }
        // Leaving MHeard → cancel spy-from-list pick mode.
        if !matches!(pane, ActivePane::Mh) {
            self.mheard_spy_pick = false;
            self.show_mheard_spy_confirm = false;
        }
        self.active = pane;
        self.status = format!("active: {}", pane.label());
    }

    /// Update channel button peer from a WA8DED link-status string.
    fn apply_link_status(&mut self, channel: u8, text: &str) {
        if !(1..=self.channels).contains(&channel) {
            return;
        }
        let idx = channel as usize;
        let upper = text.to_ascii_uppercase();
        let was_linked = self.channel_peer.get(idx).and_then(|p| p.as_ref()).is_some();

        if upper.contains("DISCONNECTED")
            || upper.contains("LINK FAILURE")
            || upper.contains("LINK FAIL")
            || upper.contains("BUSY FROM")
            || upper.contains("FAILURE WITH")
        {
            if was_linked {
                play_wav(&self.disconnect_wav);
            }
            self.channel_peer[idx] = None;
            if let Some(c) = self.channel_connecting.get_mut(idx) {
                *c = None;
            }
            if let Some((c, a)) = self.self_connect_pair {
                if channel == c || channel == a {
                    self.self_connect_pair = None;
                    self.clear_temp_identity(a);
                }
            }
            if let Some((c, a)) = self.self_connect_live {
                if channel == c || channel == a {
                    self.self_connect_live = None;
                    let other = if channel == c { a } else { c };
                    self.clear_temp_identity(a);
                    // Drop the other end of a self-connect too.
                    if self.channel_peer.get(other as usize).and_then(|p| p.as_ref()).is_some() {
                        self.tx_modem_line(other, "D");
                        if let Some(p) = self.channel_peer.get_mut(other as usize) {
                            *p = None;
                        }
                    }
                }
            }
            // All free again → one global `I` (+ any user-dedicated slots).
            if !self.any_channel_busy() {
                self.restore_global_identities();
            }
            return;
        }
        if let Some(pos) = upper.find("CONNECTED TO ") {
            let after = &text[pos + "CONNECTED TO ".len()..];
            let call = after
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .to_ascii_uppercase();
            if let Some(c) = self.channel_connecting.get_mut(idx) {
                *c = None;
            }
            self.channel_peer[idx] = if call.is_empty() {
                Some("****".into())
            } else {
                Some(call)
            };
        } else if upper.contains("CONNECTED") {
            if let Some(c) = self.channel_connecting.get_mut(idx) {
                *c = None;
            }
            if self.channel_peer[idx].is_none() {
                self.channel_peer[idx] = Some("****".into());
            }
        }
        // Classic: on connect, send GTEXT / CTEXT to the peer.
        if upper.contains("CONNECTED TO ") || upper.contains("CONNECTED") {
            if !was_linked {
                play_wav(&self.connect_wav);
            }
            self.prepare_busy_redirect_identities();
            // Self-connect: answer channel sends CTEXT (white there, green on caller).
            if let Some((caller, answer)) = self.self_connect_pair {
                if channel == caller || channel == answer {
                    self.complete_self_connect_ctext(caller, answer);
                    return;
                }
            }
            if !was_linked {
                self.send_connect_text(channel);
            }
        }
    }

    /// Effective MYCALL for a slot (ephemeral self-connect SSID overrides config).
    fn effective_mycall(&self, channel: u8) -> String {
        if let Some(Some(call)) = self.temp_slot_call.get(channel as usize) {
            return call.clone();
        }
        if channel == 0 {
            self.cfg.primary_mycall().to_string()
        } else {
            self.cfg.mycall_for_slot(channel).to_string()
        }
    }

    /// Push this slot's effective MYCALL to the TNC (`I call`).
    fn push_channel_identity(&mut self, channel: u8) {
        let call = self.effective_mycall(channel);
        self.tx_modem_line(channel, &format!("I {call}"));
    }

    /// Give `channel` a user/config MYCALL and put it on the air.
    /// One callsign → one channel (duplicates fall back to global, not `-n` spam).
    fn claim_identity(&mut self, channel: u8, call: &str) -> Result<String, String> {
        let call = crate::config::normalize_mycall(call)?;
        let primary = self.cfg.primary_mycall().to_string();
        if let Some(t) = self.temp_slot_call.get_mut(channel as usize) {
            *t = None;
        }
        for ch in 1..=self.channels {
            if ch == channel {
                continue;
            }
            if self.effective_mycall(ch).eq_ignore_ascii_case(&call) {
                if let Some(t) = self.temp_slot_call.get_mut(ch as usize) {
                    *t = None;
                }
                let _ = self.cfg.set_mycall_for_slot(ch, &primary);
                // Only re-push if that slot had a non-global call on the air.
                if !call.eq_ignore_ascii_case(&primary) {
                    self.push_channel_identity(ch);
                }
            }
        }
        self.cfg.set_mycall_for_slot(channel, &call)?;
        // Radio: global lives on ch0; per-channel `I` only for a real override.
        if channel == 0 || !call.eq_ignore_ascii_case(&primary) {
            self.push_channel_identity(channel);
        } else if channel == 1 {
            // Keep ch1 acceptor in sync with global (single on-air call).
            self.push_channel_identity(0);
        }
        Ok(call)
    }

    /// Temporary `{base}-{channel}` on air for self-connect / busy redirect only.
    /// Not written into Config.toml and not applied to every free slot.
    fn assign_channel_ssid(&mut self, channel: u8) -> Result<String, String> {
        if channel == 0 {
            return Ok(self.cfg.primary_mycall().to_string());
        }
        let call = Config::channel_ssid_mycall(self.cfg.primary_mycall(), channel);
        let call = crate::config::normalize_mycall(&call)?;
        if let Some(t) = self.temp_slot_call.get_mut(channel as usize) {
            *t = Some(call.clone());
        }
        self.push_channel_identity(channel);
        Ok(call)
    }

    fn clear_temp_identity(&mut self, channel: u8) {
        let had = self
            .temp_slot_call
            .get_mut(channel as usize)
            .and_then(|t| t.take())
            .is_some();
        if had {
            // Put that TNC channel back to config/global — one `I`, not a flood.
            self.push_channel_identity(channel);
        }
    }

    fn any_channel_busy(&self) -> bool {
        (1..=self.channels).any(|n| self.channel_connected(n))
    }

    /// User-set call in Config that is not the global MYCALL (e.g. DB0ABC).
    fn channel_has_dedicated_mycall(&self, channel: u8) -> bool {
        let primary = self.cfg.primary_mycall();
        let call = self.cfg.mycall_for_slot(channel);
        !call.is_empty() && !call.eq_ignore_ascii_case(primary)
    }

    /// Which channel currently owns this callsign, if any (includes temp SSID).
    fn channel_owning_call(&self, call: &str) -> Option<u8> {
        (1..=self.channels).find(|&ch| self.effective_mycall(ch).eq_ignore_ascii_case(call))
    }

    fn sync_channel_identities(&mut self) {
        self.restore_global_identities();
    }

    /// Idle / link-up: **one** global `I` on the radio, plus any user-dedicated
    /// channel overrides. Auto `-n` SSIDs stay internal / ephemeral only.
    fn restore_global_identities(&mut self) {
        let primary = self.cfg.primary_mycall().to_string();
        for ch in 1..=self.channels {
            if let Some(t) = self.temp_slot_call.get_mut(ch as usize) {
                *t = None;
            }
            // Scrub leftover auto `{base}-{n}` entries from older builds.
            let slot = self.cfg.mycall_for_slot(ch).to_string();
            let auto = Config::channel_ssid_mycall(&primary, ch);
            if slot.eq_ignore_ascii_case(&auto) {
                let _ = self.cfg.set_mycall_for_slot(ch, &primary);
            }
        }
        let _ = self.cfg.set_mycall_for_slot(1, &primary);
        // Single global identity on the modem.
        self.push_channel_identity(0);
        // Manual per-channel MYCALL only.
        for ch in 1..=self.channels {
            if self.channel_has_dedicated_mycall(ch) {
                self.push_channel_identity(ch);
            }
        }
        let _ = self.cfg.persist_mycall(&self.config_path);
    }

    /// After a link is up: do **not** invent `-n` identities for free channels.
    /// Global MYCALL stays on the radio; temps for self-connect stay as-is.
    fn prepare_busy_redirect_identities(&mut self) {
        // Intentionally empty — channel numbers are internal unless user-set.
    }

    /// Dedicated name in use → park redirect on a free channel with a temp SSID.
    fn redirect_busy_dedicated(&mut self, from_channel: u8) -> Option<(u8, String)> {
        let free = (1..=self.channels).find(|&n| n != from_channel && !self.channel_connected(n))?;
        let call = self.assign_channel_ssid(free).ok()?;
        Some((free, call))
    }

    /// Expand and TX `GTEXT.GPI` (or picked CT file) on the given channel.
    fn send_connect_text(&mut self, channel: u8) {
        let Some(text) = self.load_connect_text(channel) else {
            return;
        };
        let prev = self.active;
        if channel >= 1 {
            self.active = ActivePane::Channel(channel);
        }
        self.send_gpi_text(&text);
        self.active = prev;
    }

    fn load_connect_text(&self, channel: u8) -> Option<String> {
        let raw = [
            self.session.ctext_path.clone(),
            gpi::default_gtext_path(),
            crate::paths::data_file("GTEXT.GPI"),
        ]
        .into_iter()
        .find_map(|p| gpi::read_file(&p))?;
        let mut ctx = self.macro_ctx();
        self.apply_channel_peer(&mut ctx, channel);
        Some(gpi::expand_gpi_text(&raw, &ctx))
    }

    /// Fill `%C` / `%N` / `%Y` / `%K` for one channel.
    /// `%N` stays empty here when no name is stored; the expander uses the call.
    fn apply_channel_peer(&self, ctx: &mut MacroContext, channel: u8) {
        ctx.channel = channel;
        ctx.my_call = self.effective_mycall(channel);
        let peer = self
            .channel_peer
            .get(channel as usize)
            .and_then(|p| p.as_ref())
            .filter(|c| !c.is_empty() && *c != "****");
        if let Some(call) = peer {
            ctx.peer_call = call.clone();
            ctx.peer_name = self.mheard.name_for(call).unwrap_or_default();
            ctx.name_unknown = ctx.peer_name.is_empty();
        } else {
            ctx.peer_call.clear();
            ctx.peer_name.clear();
            ctx.name_unknown = false;
        }
    }

    /// KISS self-connect: no RF loopback — TX UA as answer, link both ends, CTEXT.
    fn kiss_finish_self_connect(&mut self, caller: u8, answer: u8) {
        let answer_call = self.effective_mycall(answer);
        let caller_call = self.effective_mycall(caller);
        // Answer SABM with UA (visible in Soundmodem + Monitor).
        self.tx_kiss_payload(0, &ax25::build_ua(&caller_call, &answer_call, true));
        self.push_kiss_mon(
            PadKind::Own,
            &answer_call,
            &caller_call,
            &[],
            Ax25FrameType::Ua,
            0,
            0,
            true,
            false,
            None,
        );
        if let Some(pad) = self.pads.get_mut(answer as usize) {
            pad.push(
                PadKind::System,
                format!("*** CONNECTED to {caller_call}"),
            );
        }
        if let Some(pad) = self.pads.get_mut(caller as usize) {
            pad.push(
                PadKind::System,
                format!("*** CONNECTED to {answer_call}"),
            );
        }
        for ch in [caller, answer] {
            if let Some(vs) = self.ax25_vs.get_mut(ch as usize) {
                *vs = 0;
            }
            if let Some(vr) = self.ax25_vr.get_mut(ch as usize) {
                *vr = 0;
            }
            self.kiss_link_activity(ch, true);
        }
        play_wav(&self.connect_wav);
        self.kiss_clear_sabm_timer(caller);
        self.kiss_clear_sabm_timer(answer);
        self.complete_self_connect_ctext(caller, answer);
        self.status = format!("KISS self-connect ch{caller} ↔ ch{answer}");
    }

    /// Self-connect finished: answer side TX CTEXT — white on answer, green on caller.
    fn complete_self_connect_ctext(&mut self, caller: u8, answer: u8) {
        // Mark both ends linked (TNC may only report CONNECTED on the caller).
        let answer_call = self.effective_mycall(answer);
        // Channel list shows the peer callsign — answer side sees global MYCALL.
        let caller_call = self.cfg.primary_mycall().to_string();
        if let Some(p) = self.channel_peer.get_mut(caller as usize) {
            *p = Some(answer_call);
        }
        if let Some(p) = self.channel_peer.get_mut(answer as usize) {
            *p = Some(caller_call);
        }
        if let Some(c) = self.channel_connecting.get_mut(caller as usize) {
            *c = None;
        }
        if let Some(c) = self.channel_connecting.get_mut(answer as usize) {
            *c = None;
        }
        self.self_connect_live = Some((caller, answer));
        self.self_connect_pair = None;

        let Some(text) = self.load_connect_text(answer) else {
            return;
        };
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        let peer = self.cfg.primary_mycall().to_string();
        let my = self.effective_mycall(answer);
        let kiss = self.kiss_mode();
        // Sending window is the same terminal: a newline in the file is a
        // row, and a colour sequence changes the pen. The 128-byte cut does not.
        self.seal_term(answer);
        self.accept_rx_text(answer, &text, PadKind::Own);
        self.seal_term(answer);
        if !kiss {
            for line in payload_lines(&text) {
                self.pads[0].push_monitor(PadKind::Own, strip_ansi(&line));
            }
        }
        self.seal_term(caller);
        for chunk in pack_text_payloads(&text, max) {
            // Caller rebuilds rows from newlines, not from the 128-byte cut.
            self.accept_rx_text(caller, &chunk, PadKind::Incoming);
            if kiss {
                let ns = *self.ax25_vs.get(answer as usize).unwrap_or(&0);
                let nr = *self.ax25_vr.get(answer as usize).unwrap_or(&0);
                let frame = ax25::build_i(&peer, &my, ns, nr, false, chunk.as_bytes());
                if let Some(vs) = self.ax25_vs.get_mut(answer as usize) {
                    *vs = (ns + 1) & 7;
                }
                self.tx_kiss_payload(0, &frame);
                self.push_kiss_mon(
                    PadKind::Own,
                    &my,
                    &peer,
                    &[],
                    Ax25FrameType::Info,
                    ns,
                    nr,
                    false,
                    true,
                    Some(&chunk),
                );
                // Caller answers this I-frame. No RF loopback, so the RR is
                // generated here and applied to the send queue.
                self.tx_local_rr(&my, &peer, ns);
            } else {
                let _ = self.cmd_tx.try_send(TncCommand::HostFrame {
                    channel: answer,
                    data: chunk.into_bytes(),
                });
            }
        }
        self.status = format!("CTEXT ch{answer} → ch{caller}");
    }

    /// Hostmode `D` or KISS DISC; tears down self-connect pair too.
    /// Clears local link state immediately so the DISCONNECT icon / `n: ......`
    /// update even if the modem is slow to report DISCONNECTED.
    fn disconnect_channel(&mut self, channel: u8) {
        if !(1..=self.channels).contains(&channel) {
            return;
        }
        let was_linked = self
            .channel_peer
            .get(channel as usize)
            .and_then(|p| p.as_ref())
            .is_some();
        if self.kiss_mode() {
            self.kiss_send_disc(channel);
        } else {
            self.tx_modem_line(channel, "D");
        }

        let mut also = Vec::new();
        if let Some((c, a)) = self.self_connect_live.take() {
            if channel == c || channel == a {
                also.push(if channel == c { a } else { c });
            } else {
                self.self_connect_live = Some((c, a));
            }
        }
        if let Some((c, a)) = self.self_connect_pair.take() {
            if channel == c || channel == a {
                also.push(a);
                also.push(c);
                self.clear_temp_identity(a);
            } else {
                self.self_connect_pair = Some((c, a));
            }
        }
        for other in also {
            if other != channel && (1..=self.channels).contains(&other) {
                if self.kiss_mode() {
                    self.kiss_send_disc(other);
                } else {
                    self.tx_modem_line(other, "D");
                }
                self.clear_channel_link_state(other);
            }
        }
        self.clear_channel_link_state(channel);
        if was_linked {
            play_wav(&self.disconnect_wav);
        }
        if !self.any_channel_busy() {
            self.restore_global_identities();
        }
        self.status = format!("Disconnect ch{channel}");
    }

    fn kiss_send_disc(&mut self, channel: u8) {
        let peer = self
            .channel_peer
            .get(channel as usize)
            .and_then(|p| p.clone())
            .or_else(|| {
                self.channel_connecting
                    .get(channel as usize)
                    .and_then(|p| p.clone())
            });
        let Some(peer) = peer else {
            return;
        };
        let src = self.effective_mycall(channel);
        self.tx_kiss_payload(0, &ax25::build_disc(&peer, &src));
        self.push_kiss_mon(
            PadKind::Own,
            &src,
            &peer,
            &[],
            Ax25FrameType::Disc,
            0,
            0,
            true,
            true,
            None,
        );
    }

    fn clear_channel_link_state(&mut self, channel: u8) {
        let idx = channel as usize;
        if let Some(p) = self.channel_peer.get_mut(idx) {
            *p = None;
        }
        if let Some(c) = self.channel_connecting.get_mut(idx) {
            *c = None;
        }
        if let Some(t) = self.ax25_alive_at.get_mut(idx) {
            *t = None;
        }
        if let Some(t) = self.ax25_poll_at.get_mut(idx) {
            *t = None;
        }
        if let Some(n) = self.ax25_poll_tries.get_mut(idx) {
            *n = 0;
        }
        if let Some(t) = self.ax25_sabm_at.get_mut(idx) {
            *t = None;
        }
        if let Some(n) = self.ax25_sabm_tries.get_mut(idx) {
            *n = 0;
        }
        if let Some(vs) = self.ax25_vs.get_mut(idx) {
            *vs = 0;
        }
        if let Some(vr) = self.ax25_vr.get_mut(idx) {
            *vr = 0;
        }
        self.seal_term(channel);
        if let Some(dec) = self.rx_ansi.get_mut(idx) {
            *dec = AnsiDecoder::new();
        }
    }

    fn kiss_arm_sabm_timer(&mut self, channel: u8) {
        let idx = channel as usize;
        if let Some(t) = self.ax25_sabm_at.get_mut(idx) {
            *t = Some(Instant::now());
        }
        if let Some(n) = self.ax25_sabm_tries.get_mut(idx) {
            *n = 1;
        }
    }

    fn kiss_clear_sabm_timer(&mut self, channel: u8) {
        let idx = channel as usize;
        if let Some(t) = self.ax25_sabm_at.get_mut(idx) {
            *t = None;
        }
        if let Some(n) = self.ax25_sabm_tries.get_mut(idx) {
            *n = 0;
        }
    }

    /// Connecting → linked once we see traffic (or explicit CONNECTED elsewhere).
    fn promote_connecting_on_data(&mut self, channel: u8) {
        let idx = channel as usize;
        if self.channel_peer.get(idx).and_then(|p| p.as_ref()).is_some() {
            return;
        }
        // Pending self-connect: either side's first data finishes the pair.
        if let Some((caller, answer)) = self.self_connect_pair {
            if channel == caller || channel == answer {
                self.complete_self_connect_ctext(caller, answer);
                return;
            }
        }
        let Some(dest) = self
            .channel_connecting
            .get(idx)
            .and_then(|c| c.as_ref())
            .cloned()
        else {
            return;
        };
        if let Some(c) = self.channel_connecting.get_mut(idx) {
            *c = None;
        }
        if let Some(p) = self.channel_peer.get_mut(idx) {
            *p = Some(dest);
        }
        play_wav(&self.connect_wav);
        self.prepare_busy_redirect_identities();
        self.send_connect_text(channel);
    }

    fn channel_connected(&self, n: u8) -> bool {
        let idx = n as usize;
        self.channel_peer
            .get(idx)
            .and_then(|p| p.as_ref())
            .is_some()
            || self
                .channel_connecting
                .get(idx)
                .and_then(|p| p.as_ref())
                .is_some()
    }

    /// True when the channel strip should show DISCONNECT (fully linked).
    fn channel_linked(&self, n: u8) -> bool {
        self.channel_peer
            .get(n as usize)
            .and_then(|p| p.as_ref())
            .is_some()
    }

    /// Channel to drop for `//Q` / `//DISC`: prefer the given slot, else active
    /// linked channel, else any linked channel.
    fn resolve_disconnect_channel(&self, channel: u8) -> u8 {
        if channel >= 1 && self.channel_connected(channel) {
            return channel;
        }
        if let ActivePane::Channel(n) = self.active {
            if n >= 1 && self.channel_connected(n) {
                return n;
            }
        }
        (1..=self.channels)
            .find(|&n| self.channel_linked(n))
            .or_else(|| (1..=self.channels).find(|&n| self.channel_connected(n)))
            .unwrap_or(0)
    }

    fn channel_label(&self, n: u8) -> String {
        let idx = n as usize;
        if let Some(call) = self.channel_peer.get(idx).and_then(|p| p.as_ref()) {
            format!("{n}: {call}")
        } else if let Some(call) = self.channel_connecting.get(idx).and_then(|p| p.as_ref()) {
            format!("{n}: {call}")
        } else {
            format!("{n}: ......")
        }
    }

    /// Which channel a ch0/monitor link-status line belongs to.
    fn infer_link_status_channel(&self, text: &str) -> Option<u8> {
        // "(2) CONNECTED to …" style — trust the modem's channel tag.
        if let Some(ch) = parse_hostmode_channel_tag(text) {
            if (1..=self.channels).contains(&ch) {
                return Some(ch);
            }
        }
        if let Some((c, _)) = self.self_connect_pair {
            return Some(c); // pending self-connect → caller
        }
        // Prefer a channel that is mid-connect.
        (1..=self.channels)
            .find(|&n| {
                self.channel_connecting
                    .get(n as usize)
                    .and_then(|p| p.as_ref())
                    .is_some()
            })
            .or_else(|| {
                let upper = text.to_ascii_uppercase();
                if upper.contains("CONNECTED") {
                    match self.active {
                        ActivePane::Channel(n) => Some(n),
                        _ => None,
                    }
                } else {
                    None
                }
            })
    }

    /// Local TX echo into the RX pad(s). Channel pads always keep a transcript;
    /// Monitor (and the monitor copy of channel TX) follows the Echo toggle.
    /// KISS: channel body only here — Monitor gets `fm … ctl …` in `send_kiss_text`.
    fn echo_own_line(&mut self, line: &str) {
        let idx = match self.active {
            ActivePane::Channel(n) => n as usize,
            _ => 0,
        };
        // Enter finishes a row. It does not glue onto an open received line.
        // Colour bytes in the typed line paint that row; they are not shown.
        if idx != 0 {
            let ch = idx as u8;
            self.seal_term(ch);
            self.accept_rx_text(ch, line, PadKind::Own);
            let wrote = self.rx_row.get(idx).copied().flatten().is_some();
            self.seal_term(ch);
            if !wrote {
                if let Some(pad) = self.pads.get_mut(idx) {
                    pad.push(PadKind::Own, "");
                }
            }
        }
        if self.kiss_mode() {
            return;
        }
        if self.session.echo {
            self.pads[0].push_monitor(PadKind::Own, strip_ansi(line));
        }
    }

    fn kiss_mode(&self) -> bool {
        self.cfg.tnc.protocol_kind() == HostProtocol::Kiss
    }

    /// GP DOS / axlisten monitor header on pad 0 (`fm … ctl …`).
    fn push_kiss_mon(
        &mut self,
        kind: PadKind,
        src: &str,
        dest: &str,
        via: &[String],
        frame_kind: Ax25FrameType,
        ns: u8,
        nr: u8,
        poll: bool,
        command: bool,
        info_text: Option<&str>,
    ) {
        let hdr = stamp_monitor_fm(&ax25::monitor_ctl_line(
            src, dest, via, frame_kind, ns, nr, poll, command,
        ));
        self.pads[0].push_monitor(kind, hdr);
        if let Some(text) = info_text {
            for line in ax25::monitor_payload_lines(text) {
                self.pads[0].push_monitor(kind, strip_ansi(&line));
            }
        }
    }

    fn push_kiss_mon_frame(&mut self, kind: PadKind, frame: &Ax25Frame, info_text: Option<&str>) {
        self.push_kiss_mon(
            kind,
            &frame.src,
            &frame.dest,
            &frame.via,
            frame.kind,
            frame.ns,
            frame.nr,
            frame.poll,
            frame.command,
            info_text,
        );
    }

    /// Raw KISS port payload (already AX.25-framed). Port 0 = Soundmodem radio 1.
    fn tx_kiss_payload(&mut self, port: u8, payload: &[u8]) {
        let _ = self.cmd_tx.try_send(TncCommand::HostFrame {
            channel: port,
            data: payload.to_vec(),
        });
    }

    /// RR response from `i_dest` back to `i_src` for I-frame `ns`.
    /// Written on the air and fed to the send queue (`LocalAck`), because a
    /// self-connect does not hear its own frames come back. The driver holds
    /// it until the I-frame could have left the radio.
    fn tx_local_rr(&mut self, i_src: &str, i_dest: &str, ns: u8) {
        let nr = ns.wrapping_add(1) & 7;
        if let Some(ch) = self.kiss_channel_owning_dest(i_dest) {
            if let Some(vr) = self.ax25_vr.get_mut(ch as usize) {
                *vr = nr;
            }
            self.kiss_link_activity(ch, true);
        }
        let frame = ax25::build_rr(i_src, i_dest, nr, false, false);
        let _ = self.cmd_tx.try_send(TncCommand::LocalAck { port: 0, frame });
        self.push_kiss_mon(
            PadKind::Incoming,
            i_dest,
            i_src,
            &[],
            Ax25FrameType::Rr,
            0,
            nr,
            false,
            false,
            None,
        );
    }

    /// Self-connect: other channel sees this text as Incoming (no RF loopback).
    fn deliver_self_connect_rx(&mut self, from_ch: u8, line: &str) {
        let Some((c, a)) = self.self_connect_live else {
            return;
        };
        let other = if from_ch == c {
            a
        } else if from_ch == a {
            c
        } else {
            return;
        };
        self.accept_rx_text(other, line, PadKind::Incoming);
        self.kiss_link_activity(other, true);
    }

    /// Terminal write. Bytes append to the open row. `\n` / `\r` end that row.
    /// The end of a packet does not.
    fn accept_rx_text(&mut self, channel: u8, chunk: &str, kind: PadKind) {
        let bytes = chunk.as_bytes();
        let mut i = 0;
        if self.rx_cr.get(channel as usize).copied().unwrap_or(false) {
            if let Some(flag) = self.rx_cr.get_mut(channel as usize) {
                *flag = false;
            }
            if bytes.first() == Some(&b'\n') {
                i = 1;
            }
            self.term_break(channel, kind);
        }
        while i < bytes.len() {
            if bytes[i] == b'\n' {
                self.term_break(channel, kind);
                i += 1;
            } else if bytes[i] == b'\r' {
                if bytes.get(i + 1) == Some(&b'\n') {
                    self.term_break(channel, kind);
                    i += 2;
                } else if i + 1 == bytes.len() {
                    if let Some(flag) = self.rx_cr.get_mut(channel as usize) {
                        *flag = true;
                    }
                    i += 1;
                } else {
                    self.term_break(channel, kind);
                    i += 1;
                }
            } else {
                let start = i;
                while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                    i += 1;
                }
                let allow = self.cfg.session.allow_color_ansi;
                let runs = self
                    .rx_ansi
                    .get_mut(channel as usize)
                    .map(|dec| dec.push_text(&chunk[start..i], allow))
                    .unwrap_or_default();
                for (text, style) in runs {
                    self.term_append(channel, kind, &text, style);
                }
            }
        }
    }

    /// Next byte starts a new row. The current row stays as it is.
    fn seal_term(&mut self, channel: u8) {
        let idx = channel as usize;
        if let Some(slot) = self.rx_row.get_mut(idx) {
            *slot = None;
        }
        if let Some(flag) = self.rx_cr.get_mut(idx) {
            *flag = false;
        }
    }

    fn term_append(&mut self, channel: u8, kind: PadKind, text: &str, style: AnsiStyle) {
        if text.is_empty() {
            return;
        }
        let idx = channel as usize;
        let color = style.paint(kind.color());
        if let Some(seq) = self.rx_row.get(idx).copied().flatten() {
            if self
                .pads
                .get_mut(idx)
                .is_some_and(|pad| pad.append_styled(seq, text, Some(color)))
            {
                return;
            }
        }
        let Some(pad) = self.pads.get_mut(idx) else {
            return;
        };
        let seq = pad.push_styled(kind, text, color);
        if let Some(slot) = self.rx_row.get_mut(idx) {
            *slot = Some(seq);
        }
    }

    fn term_break(&mut self, channel: u8, kind: PadKind) {
        let idx = channel as usize;
        let open = self.rx_row.get(idx).copied().flatten();
        if let Some(seq) = open {
            let text = self
                .pads
                .get(idx)
                .and_then(|pad| pad.text_if_seq(seq))
                .unwrap_or_default();
            if let Some(slot) = self.rx_row.get_mut(idx) {
                *slot = None;
            }
            self.maybe_remote_line(channel, &text);
        } else if let Some(pad) = self.pads.get_mut(idx) {
            pad.push(kind, "");
        }
    }

    fn maybe_remote_line(&mut self, channel: u8, line: &str) {
        if self.session.remote && channel >= 1 {
            if let Some((action, args)) = parse_line(line.trim()) {
                self.handle_remote_on_channel(channel, action, &args);
            }
        }
    }

    /// One already-packed chunk → AX.25 UI or I-frame. Bytes are sent as-is.
    fn send_kiss_text(&mut self, line: &str) {
        let my = self.mycall_for_active();
        let info = line.as_bytes();
        match self.active {
            ActivePane::Channel(n) if self.channel_linked(n) => {
                let Some(peer) = self
                    .channel_peer
                    .get(n as usize)
                    .and_then(|p| p.clone())
                else {
                    return;
                };
                let ns = *self.ax25_vs.get(n as usize).unwrap_or(&0);
                let nr = *self.ax25_vr.get(n as usize).unwrap_or(&0);
                let frame = ax25::build_i(&peer, &my, ns, nr, false, info);
                if let Some(vs) = self.ax25_vs.get_mut(n as usize) {
                    *vs = (ns + 1) & 7;
                }
                self.tx_kiss_payload(0, &frame);
                self.kiss_link_activity(n, false);
                self.push_kiss_mon(
                    PadKind::Own,
                    &my,
                    &peer,
                    &[],
                    Ax25FrameType::Info,
                    ns,
                    nr,
                    false,
                    true,
                    Some(line),
                );
                // Self-connect peer channel sees the chat and answers with RR.
                self.deliver_self_connect_rx(n, line);
                if self
                    .self_connect_live
                    .is_some_and(|(c, a)| n == c || n == a)
                {
                    self.tx_local_rr(&my, &peer, ns);
                }
            }
            ActivePane::Channel(_) => {
                self.status = "KISS: not linked — Connect first, or type on Monitor (CQ)".into();
            }
            _ => {
                // Unproto: MYCALL → CQ
                let frame = ax25::build_ui("CQ", &my, &[], info);
                self.tx_kiss_payload(0, &frame);
                self.push_kiss_mon(
                    PadKind::Own,
                    &my,
                    "CQ",
                    &[],
                    Ax25FrameType::Ui,
                    0,
                    0,
                    false,
                    true,
                    Some(line),
                );
                self.status = format!("KISS UI {my} > CQ");
            }
        }
    }

    /// KISS RX: UI → Monitor; SABM/UA/I/DISC → channel link state.
    fn handle_kiss_ax25(&mut self, frame: Ax25Frame) {
        let info_text = decode_rx_text(&frame.info, self.session.extended_ascii);
        match frame.kind {
            Ax25FrameType::Ui => {
                let ui = Ax25UiFrame {
                    dest: frame.dest.clone(),
                    src: frame.src.clone(),
                    via: frame.via.clone(),
                    info: info_text.clone(),
                };
                self.mheard.hear(&ui);
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, Some(&info_text));
                let mon = frame.monitor_line();
                self.feed_spy_pads(&frame.src, PadKind::Incoming, &mon);
                for line in ax25::monitor_payload_lines(&info_text) {
                    self.feed_spy_pads(&frame.src, PadKind::Incoming, &line);
                }
            }
            Ax25FrameType::Sabm => {
                if !self.cfg.is_own_call(&frame.dest)
                    && self.channel_owning_call(&frame.dest).is_none()
                {
                    return;
                }
                // Pending KISS self-connect: this SABM is for our answer SSID —
                // finish locally (also covers rare TX→RX loopback).
                if let Some((caller, answer)) = self.self_connect_pair {
                    let ans = self.effective_mycall(answer);
                    if frame.dest.eq_ignore_ascii_case(&ans) {
                        self.kiss_finish_self_connect(caller, answer);
                        return;
                    }
                }
                // Already linked to this peer on a live self-connect — ignore echo.
                if let Some((c, a)) = self.self_connect_live {
                    let ans = self.effective_mycall(a);
                    let call = self.effective_mycall(c);
                    if frame.src.eq_ignore_ascii_case(&call)
                        && frame.dest.eq_ignore_ascii_case(&ans)
                    {
                        return;
                    }
                }
                // Answer on the slot that owns this call, else first free.
                let owned = self.channel_owning_call(&frame.dest);
                let ch = match owned {
                    Some(c) if !self.channel_linked(c) => Some(c),
                    Some(_) => self.first_free_channel(),
                    None => self.first_free_channel(),
                };
                // Heard SABM (even if we refuse).
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, None);
                let Some(ch) = ch else {
                    // All busy → DM (refuse).
                    let my = self.cfg.primary_mycall().to_string();
                    self.tx_kiss_payload(0, &ax25::build_dm(&frame.src, &my, true));
                    self.push_kiss_mon(
                        PadKind::Own,
                        &my,
                        &frame.src,
                        &[],
                        Ax25FrameType::Dm,
                        0,
                        0,
                        true,
                        false,
                        None,
                    );
                    return;
                };
                let my = if owned == Some(ch) {
                    frame.dest.clone()
                } else {
                    if let Some(t) = self.temp_slot_call.get_mut(ch as usize) {
                        *t = Some(frame.dest.clone());
                    }
                    frame.dest.clone()
                };
                // SABM → UA (we answer connect requests).
                self.tx_kiss_payload(0, &ax25::build_ua(&frame.src, &my, true));
                if let Some(p) = self.channel_peer.get_mut(ch as usize) {
                    *p = Some(frame.src.clone());
                }
                if let Some(c) = self.channel_connecting.get_mut(ch as usize) {
                    *c = None;
                }
                if let Some(vs) = self.ax25_vs.get_mut(ch as usize) {
                    *vs = 0;
                }
                if let Some(vr) = self.ax25_vr.get_mut(ch as usize) {
                    *vr = 0;
                }
                self.kiss_clear_sabm_timer(ch);
                self.kiss_link_activity(ch, true);
                play_wav(&self.connect_wav);
                if let Some(pad) = self.pads.get_mut(ch as usize) {
                    pad.push(
                        PadKind::System,
                        format!("*** CONNECTED to {}", frame.src),
                    );
                }
                self.push_kiss_mon(
                    PadKind::Own,
                    &my,
                    &frame.src,
                    &[],
                    Ax25FrameType::Ua,
                    0,
                    0,
                    true,
                    false,
                    None,
                );
                self.send_connect_text(ch);
                self.status = format!("KISS linked ch{ch} ← {}", frame.src);
            }
            Ax25FrameType::Ua => {
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, None);
                let ch = (1..=self.channels).find(|&n| {
                    self.channel_connecting
                        .get(n as usize)
                        .and_then(|p| p.as_ref())
                        .is_some_and(|d| d.eq_ignore_ascii_case(&frame.src))
                });
                let Some(ch) = ch else {
                    return;
                };
                if let Some((caller, answer)) = self.self_connect_pair {
                    if ch == caller || ch == answer {
                        self.complete_self_connect_ctext(caller, answer);
                        self.kiss_link_activity(caller, true);
                        self.kiss_link_activity(answer, true);
                        return;
                    }
                }
                if let Some(c) = self.channel_connecting.get_mut(ch as usize) {
                    *c = None;
                }
                if let Some(p) = self.channel_peer.get_mut(ch as usize) {
                    *p = Some(frame.src.clone());
                }
                self.kiss_clear_sabm_timer(ch);
                self.kiss_link_activity(ch, true);
                play_wav(&self.connect_wav);
                if let Some(pad) = self.pads.get_mut(ch as usize) {
                    pad.push(
                        PadKind::System,
                        format!("*** CONNECTED to {}", frame.src),
                    );
                }
                self.send_connect_text(ch);
                self.status = format!("KISS linked ch{ch} → {}", frame.src);
            }
            Ax25FrameType::Info => {
                // Route to channel whose peer is the sender (or dest is our call).
                let ch = (1..=self.channels)
                    .find(|&n| {
                        self.channel_peer
                            .get(n as usize)
                            .and_then(|p| p.as_ref())
                            .is_some_and(|p| p.eq_ignore_ascii_case(&frame.src))
                    })
                    .or_else(|| {
                        (1..=self.channels).find(|&n| {
                            self.channel_linked(n)
                                && self
                                    .effective_mycall(n)
                                    .eq_ignore_ascii_case(&frame.dest)
                        })
                    })
                    .unwrap_or(0);
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, Some(&info_text));
                if ch >= 1 {
                    if let Some(vr) = self.ax25_vr.get_mut(ch as usize) {
                        *vr = (frame.ns.wrapping_add(1)) & 7;
                    }
                    let my = self.effective_mycall(ch);
                    let nr = *self.ax25_vr.get(ch as usize).unwrap_or(&0);
                    // Ack I with RR (Final if peer set P).
                    self.tx_kiss_payload(
                        0,
                        &ax25::build_rr(&frame.src, &my, nr, frame.poll, false),
                    );
                    self.push_kiss_mon(
                        PadKind::Own,
                        &my,
                        &frame.src,
                        &[],
                        Ax25FrameType::Rr,
                        0,
                        nr,
                        frame.poll,
                        false,
                        None,
                    );
                    self.kiss_link_activity(ch, true);
                    self.accept_rx_text(ch, &info_text, PadKind::Incoming);
                }
            }
            Ax25FrameType::Disc => {
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, None);
                let ch = (1..=self.channels).find(|&n| {
                    self.channel_peer
                        .get(n as usize)
                        .and_then(|p| p.as_ref())
                        .is_some_and(|p| p.eq_ignore_ascii_case(&frame.src))
                        || self
                            .channel_connecting
                            .get(n as usize)
                            .and_then(|p| p.as_ref())
                            .is_some_and(|p| p.eq_ignore_ascii_case(&frame.src))
                });
                if let Some(ch) = ch {
                    let my = self.effective_mycall(ch);
                    self.tx_kiss_payload(0, &ax25::build_ua(&frame.src, &my, true));
                    self.push_kiss_mon(
                        PadKind::Own,
                        &my,
                        &frame.src,
                        &[],
                        Ax25FrameType::Ua,
                        0,
                        0,
                        true,
                        false,
                        None,
                    );
                    let was = self.channel_linked(ch);
                    self.clear_channel_link_state(ch);
                    if was {
                        play_wav(&self.disconnect_wav);
                    }
                    if let Some(pad) = self.pads.get_mut(ch as usize) {
                        pad.push(PadKind::System, "*** DISCONNECTED");
                    }
                }
            }
            Ax25FrameType::Rr => {
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, None);
                let ch = self.kiss_channel_for_frame(&frame.src, &frame.dest);
                let Some(ch) = ch else {
                    // Still answer if the poll is clearly for one of our calls.
                    if frame.poll {
                        if let Some(ch) = self.kiss_channel_owning_dest(&frame.dest) {
                            self.kiss_reply_rr_final(ch, &frame.src);
                        }
                    }
                    return;
                };
                self.kiss_link_activity(ch, true);
                if frame.poll {
                    self.kiss_reply_rr_final(ch, &frame.src);
                }
            }
            Ax25FrameType::Dm => {
                self.push_kiss_mon_frame(PadKind::Incoming, &frame, None);
                let ch = (1..=self.channels).find(|&n| {
                    self.channel_connecting
                        .get(n as usize)
                        .and_then(|p| p.as_ref())
                        .is_some_and(|d| d.eq_ignore_ascii_case(&frame.src))
                        || self
                            .channel_peer
                            .get(n as usize)
                            .and_then(|p| p.as_ref())
                            .is_some_and(|p| p.eq_ignore_ascii_case(&frame.src))
                });
                if let Some(ch) = ch {
                    if let Some(pad) = self.pads.get_mut(ch as usize) {
                        pad.push(
                            PadKind::System,
                            format!("*** DM from {} — link refused/busy", frame.src),
                        );
                    }
                    self.kiss_clear_sabm_timer(ch);
                    self.clear_channel_link_state(ch);
                }
            }
            Ax25FrameType::Other => {}
        }
    }

    /// Mark KISS link alive; peer traffic also clears an outstanding T1 poll.
    fn kiss_link_activity(&mut self, channel: u8, from_peer: bool) {
        let idx = channel as usize;
        if let Some(t) = self.ax25_alive_at.get_mut(idx) {
            *t = Some(Instant::now());
        }
        if from_peer {
            if let Some(p) = self.ax25_poll_at.get_mut(idx) {
                *p = None;
            }
            if let Some(n) = self.ax25_poll_tries.get_mut(idx) {
                *n = 0;
            }
        }
    }

    /// SABM retries while connecting; T3 idle RR poll when linked.
    fn kiss_link_timers(&mut self) {
        if !self.kiss_mode() {
            return;
        }
        self.kiss_sabm_timers();
        let now = Instant::now();
        let mut poll: Vec<u8> = Vec::new();
        let mut drop: Vec<u8> = Vec::new();
        for ch in 1..=self.channels {
            if !self.channel_linked(ch) {
                continue;
            }
            let idx = ch as usize;
            let alive = self.ax25_alive_at.get(idx).and_then(|t| *t).unwrap_or(now);
            let pending = self.ax25_poll_at.get(idx).and_then(|t| *t);
            let tries = *self.ax25_poll_tries.get(idx).unwrap_or(&0);
            if let Some(sent) = pending {
                if now.duration_since(sent) >= AX25_T1_POLL {
                    if tries >= AX25_POLL_RETRIES {
                        drop.push(ch);
                    } else {
                        poll.push(ch);
                    }
                }
            } else if now.duration_since(alive) >= AX25_T3_IDLE {
                poll.push(ch);
            }
        }
        for ch in poll {
            self.kiss_send_rr_poll(ch);
        }
        for ch in drop {
            if let Some(pad) = self.pads.get_mut(ch as usize) {
                pad.push(
                    PadKind::System,
                    "*** link timeout (no RR) — disconnecting",
                );
            }
            self.disconnect_channel(ch);
        }
    }

    /// No UA after SABM → resend every 5 s (up to 3 retries), then local give-up (no DISC).
    fn kiss_sabm_timers(&mut self) {
        let now = Instant::now();
        let mut retry: Vec<u8> = Vec::new();
        let mut give_up: Vec<u8> = Vec::new();
        for ch in 1..=self.channels {
            if self.channel_linked(ch) {
                continue;
            }
            let idx = ch as usize;
            let connecting = self
                .channel_connecting
                .get(idx)
                .and_then(|p| p.as_ref())
                .is_some();
            if !connecting {
                continue;
            }
            // Self-connect finishes immediately — no SABM wait.
            if self
                .self_connect_pair
                .is_some_and(|(c, a)| c == ch || a == ch)
            {
                continue;
            }
            let Some(sent) = self.ax25_sabm_at.get(idx).and_then(|t| *t) else {
                continue;
            };
            if now.duration_since(sent) < AX25_SABM_RETRY {
                continue;
            }
            let tries = *self.ax25_sabm_tries.get(idx).unwrap_or(&0);
            // tries = SABMs already sent; allow 1 initial + AX25_SABM_RETRIES.
            if tries >= 1 + AX25_SABM_RETRIES {
                give_up.push(ch);
            } else {
                retry.push(ch);
            }
        }
        for ch in retry {
            self.kiss_resend_sabm(ch);
        }
        for ch in give_up {
            self.kiss_sabm_give_up(ch);
        }
    }

    fn kiss_resend_sabm(&mut self, channel: u8) {
        let Some(dest) = self
            .channel_connecting
            .get(channel as usize)
            .and_then(|p| p.clone())
        else {
            return;
        };
        let src = self.effective_mycall(channel);
        self.tx_kiss_payload(0, &ax25::build_sabm(&dest, &src));
        let idx = channel as usize;
        if let Some(t) = self.ax25_sabm_at.get_mut(idx) {
            *t = Some(Instant::now());
        }
        if let Some(n) = self.ax25_sabm_tries.get_mut(idx) {
            *n = n.saturating_add(1);
        }
        self.push_kiss_mon(
            PadKind::Own,
            &src,
            &dest,
            &[],
            Ax25FrameType::Sabm,
            0,
            0,
            true,
            true,
            None,
        );
        if let Some(pad) = self.pads.get_mut(idx) {
            pad.push(PadKind::System, format!("*** retry connect to {dest}"));
        }
        self.status = format!("KISS SABM retry ch{channel} → {dest}");
    }

    /// Connect timed out — clear UI state only (never linked, so no DISC on air).
    fn kiss_sabm_give_up(&mut self, channel: u8) {
        let dest = self
            .channel_connecting
            .get(channel as usize)
            .and_then(|p| p.clone())
            .unwrap_or_else(|| "????".into());
        self.kiss_clear_sabm_timer(channel);
        self.clear_channel_link_state(channel);
        let msg = format!("*** no answer from {dest} — giving up");
        if let Some(pad) = self.pads.get_mut(channel as usize) {
            pad.push(PadKind::System, msg.clone());
        }
        self.pads[0].push(PadKind::System, msg);
        self.status = format!("KISS connect failed ch{channel} → {dest}");
        if !self.any_channel_busy() {
            self.restore_global_identities();
        }
    }

    /// Linked channel whose peer is `src`, or whose identity is `dest`.
    fn kiss_channel_for_frame(&self, src: &str, dest: &str) -> Option<u8> {
        (1..=self.channels)
            .find(|&n| {
                self.channel_peer
                    .get(n as usize)
                    .and_then(|p| p.as_ref())
                    .is_some_and(|p| p.eq_ignore_ascii_case(src))
            })
            .or_else(|| self.kiss_channel_owning_dest(dest))
    }

    fn kiss_channel_owning_dest(&self, dest: &str) -> Option<u8> {
        (1..=self.channels).find(|&n| {
            self.channel_linked(n) && self.effective_mycall(n).eq_ignore_ascii_case(dest)
        })
    }

    /// RR Final on air + Monitor (answer to peer poll / I+P).
    fn kiss_reply_rr_final(&mut self, channel: u8, peer: &str) {
        let my = self.effective_mycall(channel);
        let nr = *self.ax25_vr.get(channel as usize).unwrap_or(&0);
        self.tx_kiss_payload(0, &ax25::build_rr(peer, &my, nr, true, false));
        self.push_kiss_mon(
            PadKind::Own,
            &my,
            peer,
            &[],
            Ax25FrameType::Rr,
            0,
            nr,
            true,
            false,
            None,
        );
        self.kiss_link_activity(channel, false);
    }

    fn kiss_send_rr_poll(&mut self, channel: u8) {
        let Some(peer) = self
            .channel_peer
            .get(channel as usize)
            .and_then(|p| p.clone())
        else {
            return;
        };
        let my = self.effective_mycall(channel);
        let nr = *self.ax25_vr.get(channel as usize).unwrap_or(&0);
        // Command RR with P=1 — peer should answer RR F.
        self.tx_kiss_payload(0, &ax25::build_rr(&peer, &my, nr, true, true));
        let idx = channel as usize;
        if let Some(p) = self.ax25_poll_at.get_mut(idx) {
            *p = Some(Instant::now());
        }
        if let Some(n) = self.ax25_poll_tries.get_mut(idx) {
            *n = n.saturating_add(1);
        }
        self.push_kiss_mon(
            PadKind::Own,
            &my,
            &peer,
            &[],
            Ax25FrameType::Rr,
            0,
            nr,
            true,
            true,
            None,
        );
        self.status = format!("KISS T3 poll ch{channel}");

        // Self-connect: no RF loopback — answer locally from the other channel.
        if let Some((c, a)) = self.self_connect_live {
            let other = if channel == c {
                a
            } else if channel == a {
                c
            } else {
                return;
            };
            if self.channel_linked(other) {
                let other_peer = self.effective_mycall(channel); // who polled
                self.kiss_reply_rr_final(other, &other_peer);
                // Poll was answered — clear T1 wait on the polling side.
                self.kiss_link_activity(channel, true);
            }
        }
    }

    /// Hostmode/modem command TX — skipped on KISS (identity lives in AX.25 headers).
    fn tx_modem_line(&mut self, channel: u8, line: &str) {
        if self.kiss_mode() {
            return;
        }
        let line = line.trim_end_matches('\r').trim_end_matches('\n');
        if line.is_empty() {
            return;
        }
        if channel >= 1 {
            if let Some(pad) = self.pads.get_mut(channel as usize) {
                pad.push(PadKind::Own, line.to_string());
            }
        }
        self.pads[0].push_monitor(PadKind::Own, line.to_string());
        let mut data = line.as_bytes().to_vec();
        if !data.ends_with(&[b'\r']) {
            data.push(b'\r');
        }
        let _ = self.cmd_tx.try_send(TncCommand::HostFrame { channel, data });
    }

    fn submit(&mut self) {
        let line = std::mem::take(&mut self.input);
        let line = line.trim_end().to_string();
        if line.is_empty() {
            return;
        }

        self.typed_history.push_back(line.clone());
        while self.typed_history.len() > TYPED_HISTORY {
            self.typed_history.pop_front();
        }

        // Always show what we typed when Echo is on (before any early return).
        self.echo_own_line(&line);

        // Local / TNC monitor-mode helpers: `M IUSC`, `M`, `MONITOR IUSC`
        if let Some(mode) = parse_monitor_set(&line) {
            self.monitor_mode = mode.clone();
            self.log_event(format!("monitor mode → {mode}"));
            let _ = self.cmd_tx.try_send(TncCommand::HostFrame {
                channel: 0,
                data: format!("M {mode}").into_bytes(),
            });
            return;
        }

        if let Some((action, args)) = parse_line(&line) {
            self.handle_remote(action, &args);
            return;
        }
        // Unregistered `//verb` → serve matching data/*.GPI (not ORIGIN).
        if let Some(verb) = line.strip_prefix("//").or_else(|| line.strip_prefix("\\\\")) {
            let verb = verb.split_whitespace().next().unwrap_or("");
            if !verb.is_empty() {
                if self.serve_gpi_verb(verb) {
                    return;
                }
            }
        }

        self.send_operator_text(&line);
    }

    /// Typed text and text files: normalize breaks to `\n`, then one packet
    /// per `packet_payload` bytes. Same chunks for KISS and hostmode.
    fn send_operator_text(&mut self, text: &str) {
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        for chunk in pack_text_payloads(text, max) {
            if self.kiss_mode() {
                self.send_kiss_text(&chunk);
            } else {
                let channel = match self.active {
                    ActivePane::Channel(n) => n,
                    _ => 0,
                };
                let _ = self.cmd_tx.try_send(TncCommand::HostFrame {
                    channel,
                    data: chunk.into_bytes(),
                });
            }
        }
    }

    fn macro_ctx(&self) -> MacroContext {
        let now = Local::now();
        let channel = match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        };
        let mut ctx = MacroContext {
            version: version::APP_VERSION.into(),
            peer_call: String::new(),
            peer_name: String::new(),
            my_call: self.mycall_for_active().to_string(),
            channel,
            time: now.format("%H:%M:%S").to_string(),
            date: now.format("%d-%m-%Y").to_string(),
            zone: "UTC".into(),
            news_hint: if gpi::news_available() {
                "News available — //NEws".into()
            } else {
                String::new()
            },
            origin_line: gpi::random_origin_line(),
            digi: String::new(),
            qrg: format!("{:.3}", self.session.qrg_mhz),
            name_unknown: false,
        };
        self.apply_channel_peer(&mut ctx, channel);
        ctx
    }

    /// Serve a GPI by remote action or verb; returns true if handled.
    fn serve_gpi_verb(&mut self, verb: &str) -> bool {
        let ctx = self.macro_ctx();
        let Some((stem, text)) = gpi::serve_verb(verb, &ctx) else {
            return false;
        };
        self.status = format!("GPI {stem}");
        self.send_gpi_text(&text);
        true
    }

    fn send_gpi_text(&mut self, text: &str) {
        for line in payload_lines(text) {
            self.echo_own_line(&line);
        }
        self.send_operator_text(text);
    }

    fn handle_remote(&mut self, action: RemoteAction, args: &str) {
        self.handle_remote_on_channel(
            match self.active {
                ActivePane::Channel(n) => n,
                _ => 0,
            },
            action,
            args,
        );
    }

    fn handle_remote_on_channel(&mut self, channel: u8, action: RemoteAction, args: &str) {
        self.status = format!("remote {action:?}");
        let prev = self.active;
        if channel >= 1 {
            self.active = ActivePane::Channel(channel);
        }
        let ctx = self.macro_ctx();

        match action {
            RemoteAction::Quit => {
                // QRT text first, then hostmode disconnect.
                let ch = self.resolve_disconnect_channel(channel);
                if let Some(text) = gpi::load_expanded("qrt", &ctx) {
                    if ch >= 1 {
                        self.active = ActivePane::Channel(ch);
                    }
                    self.send_gpi_text(&text);
                } else if let Some(stem) = gpi::stem_for_action(action) {
                    self.status = format!("GPI {stem}.GPI not found");
                }
                if ch >= 1 {
                    self.disconnect_channel(ch);
                } else {
                    self.status = "//Q — select a connected channel".into();
                }
                self.active = prev;
                return;
            }
            RemoteAction::Disconnect => {
                let ch = self.resolve_disconnect_channel(channel);
                if ch >= 1 {
                    self.disconnect_channel(ch);
                } else {
                    self.status = "//DISC — select a connected channel".into();
                }
                self.active = prev;
                return;
            }
            _ => {}
        }

        if let Some(stem) = gpi::stem_for_action(action) {
            if let Some(text) = gpi::load_expanded(stem, &ctx) {
                self.send_gpi_text(&text);
                self.active = prev;
                return;
            }
            self.status = format!("GPI {stem}.GPI not found");
            self.active = prev;
            return;
        }

        match action {
            RemoteAction::Version => {
                let line = version::startup_credits();
                self.send_gpi_text(&line);
            }
            RemoteAction::Echo => {
                self.send_gpi_text(args);
            }
            RemoteAction::Bell => {
                play_bell_wav(&self.bell_wav);
            }
            RemoteAction::MHeard => {
                let lines = self.mheard.lines();
                let body = if lines.is_empty() {
                    "MHeard: (empty)".into()
                } else {
                    lines.join("\r")
                };
                self.send_gpi_text(&body);
            }
            RemoteAction::Name => {
                self.store_remote_name(channel, args);
            }
            _ => {
                self.status = format!("remote {action:?} (not implemented)");
            }
        }
        self.active = prev;
    }

    /// `//Name Jara` from the station linked on `channel` (call NL0MSK → name Jara).
    fn store_remote_name(&mut self, channel: u8, args: &str) {
        let call = self
            .channel_peer
            .get(channel as usize)
            .and_then(|p| p.clone())
            .filter(|c| !c.is_empty() && c != "****");
        let Some(call) = call else {
            self.status = "//Name — connect a station first".into();
            return;
        };
        let name = args.trim();
        if name.is_empty() {
            let shown = self.mheard.name_for(&call).unwrap_or_else(|| call.clone());
            self.status = format!("Name {call}: {shown}");
            self.send_gpi_text(&format!("Name: {shown}"));
            return;
        }
        if !self.mheard.set_name(&call, name) {
            self.status = "//Name — give a name".into();
            return;
        }
        let stored = self.mheard.name_for(&call).unwrap_or_else(|| name.to_string());
        self.status = format!("Name {call} = {stored}");
        self.send_gpi_text(&format!("Name stored: {stored}"));
    }

    fn active_layout(&self) -> LayoutJob {
        match self.active {
            ActivePane::Spy(n) => {
                let idx = n.saturating_sub(1) as usize;
                self.spy_pads
                    .get(idx)
                    .map(TextPad::layout_job)
                    .unwrap_or_default()
            }
            ActivePane::Check => self.bbs_pad.layout_job(),
            ActivePane::List => self.box_pad.layout_job(),
            ActivePane::Channel(n) => self
                .pads
                .get(n as usize)
                .map(TextPad::layout_job)
                .unwrap_or_default(),
            _ => self
                .pads
                .first()
                .map(TextPad::layout_job)
                .unwrap_or_default(),
        }
    }

    /// Empty the visible Monitor, channel, or spy pad and rewrite its log.
    fn clear_active_pad(&mut self) {
        let dir = self.cfg.paths.log_dir.clone();
        match self.active {
            ActivePane::Monitor => {
                if let Some(pad) = self.pads.first_mut() {
                    pad.clear();
                    pad.save_log(&padlog::monitor_path(&dir));
                }
                self.status = "Monitor cleared".into();
            }
            ActivePane::Channel(n) => {
                if let Some(pad) = self.pads.get_mut(n as usize) {
                    pad.clear();
                    pad.save_log(&padlog::channel_path(&dir, n));
                }
                self.status = format!("Channel {n} cleared");
            }
            ActivePane::Spy(n) => {
                let idx = n.saturating_sub(1) as usize;
                if let Some(pad) = self.spy_pads.get_mut(idx) {
                    pad.clear();
                    pad.save_log(&padlog::spy_path(&dir, n));
                }
                self.status = format!("Spy {n} cleared");
            }
            _ => {}
        }
    }

    /// Persist Own/Incoming lines for Monitor, channels, spy, BBS, Box.
    fn save_pad_logs(&self) {
        let dir = &self.cfg.paths.log_dir;
        if let Some(mon) = self.pads.first() {
            mon.save_log(&padlog::monitor_path(dir));
        }
        for ch in 1..=self.channels {
            if let Some(pad) = self.pads.get(ch as usize) {
                pad.save_log(&padlog::channel_path(dir, ch));
            }
        }
        for (i, pad) in self.spy_pads.iter().enumerate() {
            pad.save_log(&padlog::spy_path(dir, (i + 1) as u8));
        }
        self.bbs_pad.save_log(&padlog::bbs_path(dir));
        self.box_pad.save_log(&padlog::box_path(dir));
    }

    fn show_session_bar(&self) -> bool {
        matches!(
            self.active,
            ActivePane::Monitor | ActivePane::Channel(_) | ActivePane::Spy(_)
        )
    }

    fn pane_title(&self) -> String {
        match self.active {
            ActivePane::Monitor => "Monitor".into(),
            ActivePane::Channel(n) => format!("Channel {n}"),
            ActivePane::Mh => "MHeard".into(),
            ActivePane::Editor => "GP-Editor".into(),
            ActivePane::Check => "BBS-Menu".into(),
            ActivePane::List => "DieBox-List".into(),
            ActivePane::Spy(n) => format!("Spy {n}"),
        }
    }

    fn spy_call(&self, slot: u8) -> &str {
        self.cfg
            .session
            .spy_calls
            .get(slot.saturating_sub(1) as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    fn feed_spy_pads(&mut self, src: &str, kind: PadKind, line: &str) {
        let src_up = src.to_ascii_uppercase();
        for (i, call) in self.cfg.session.spy_calls.iter().enumerate() {
            if call.is_empty() {
                continue;
            }
            if call.eq_ignore_ascii_case(&src_up) {
                if let Some(pad) = self.spy_pads.get_mut(i) {
                    pad.push(kind, line.to_string());
                }
            }
        }
    }
}

impl eframe::App for GpGuiApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_pad_logs();
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_tnc();
        self.kiss_link_timers();
        self.maybe_hour_gong();
        ctx.request_repaint_after(std::time::Duration::from_millis(200));

        // Panels own reserved bands so CentralPanel (RX) cannot paint over them.
        // Strip height stays fixed; icons scale to fill it (1 px top/bottom).
        let icons_h = 72.0;
        let session_h = if self.show_session_bar() { 26.0 } else { 0.0 };
        let prewrite_h = 110.0;
        let channels_h = 72.0;
        let status_h = 44.0;
        // Slim column — dial stays centered with little side air.
        let clock_w = 156.0;
        let top_h = prewrite_h + channels_h + status_h;

        egui::TopBottomPanel::top("gp_top")
            .exact_height(top_h)
            .show_separator_line(false)
            .frame(Frame::none().fill(VGA_DARK_RED))
            .show(ctx, |ui| {
                self.top_stack_with_clock(
                    ui,
                    ui.available_width(),
                    prewrite_h,
                    channels_h,
                    status_h,
                    clock_w,
                );
            });

        egui::TopBottomPanel::bottom("gp_icons")
            .exact_height(icons_h)
            .show_separator_line(false)
            .frame(Frame::none().fill(VGA_DARK_RED))
            .show(ctx, |ui| {
                let h = ui.available_height();
                let w = ui.available_width();
                self.icon_strip(ui, w, h);
            });

        if self.show_session_bar() {
            egui::TopBottomPanel::bottom("gp_session")
                .exact_height(session_h)
                .show_separator_line(false)
                .frame(Frame::none().fill(VGA_GRAY))
                .show(ctx, |ui| {
                    self.session_bar(ui, ui.available_width(), ui.available_height());
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::none().fill(VGA_BLUE))
            .show(ctx, |ui| {
                let full_w = ui.available_width();
                let full_h = ui.available_height();
                self.rx_pane(ui, full_w, full_h);
                self.qrg_dialog(ctx);
                self.mycall_dialog(ctx);
                self.spy_dialog(ctx);
                self.mheard_spy_confirm_dialog(ctx);
                self.connect_dialog(ctx);
            });
    }
}

impl GpGuiApp {
    /// Left: purple pre-write → channels → 2-row magenta status.
    /// Right: tall clock spanning the whole stack (fixed width — never clipped).
    fn top_stack_with_clock(
        &mut self,
        ui: &mut Ui,
        width: f32,
        prewrite_h: f32,
        channels_h: f32,
        status_h: f32,
        clock_w: f32,
    ) {
        let height = prewrite_h + channels_h + status_h;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

        // Absolute split so the clock column cannot be pushed off-screen by
        // a greedy left pane (that was clipping the dial on the right).
        let clock_w = clock_w.min(rect.width() * 0.35).max(120.0);
        let left_r = rect.right() - clock_w;
        let left = Rect::from_min_max(rect.left_top(), Pos2::new(left_r, rect.bottom()));
        let pre_r =
            Rect::from_min_max(rect.left_top(), Pos2::new(left_r, rect.top() + prewrite_h));
        let ch_r = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + prewrite_h),
            Pos2::new(left_r, rect.top() + prewrite_h + channels_h),
        );
        let st_r = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + prewrite_h + channels_h),
            Pos2::new(left_r, rect.bottom()),
        );
        let clock_rect = Rect::from_min_max(Pos2::new(left_r, rect.top()), rect.right_bottom());

        // Channel chrome = dark red (same as icon strip). Clock column = dark green.
        ui.painter_at(left).rect_filled(left, 0.0, VGA_DARK_RED);
        ui.painter_at(clock_rect)
            .rect_filled(clock_rect, 0.0, VGA_DARK_GREEN);

        self.draw_prewrite(ui, pre_r);
        self.draw_channel_grid(ui, ch_r);
        self.draw_status_grid(ui, st_r);
        if draw_analog_clock(ui, clock_rect, self.hour_gong) {
            self.hour_gong = !self.hour_gong;
            self.status = format!(
                "Hourly gong {}",
                if self.hour_gong { "ON" } else { "OFF" }
            );
            if self.hour_gong {
                play_bell_wav(&self.bell_wav);
            }
        }
    }

    fn draw_prewrite(&mut self, ui: &mut Ui, rect: Rect) {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            // Purple / magenta input chrome (classic input colours, or state magenta).
            let fill = if self.input_bg == Color32::BLACK {
                Color32::from_rgb(85, 0, 85)
            } else {
                self.input_bg
            };
            Frame::none()
                .fill(fill)
                .stroke(Stroke::new(1.0, Color32::from_rgb(170, 0, 170)))
                .inner_margin(4.0)
                .show(ui, |ui| {
                    let inner = Vec2::new(
                        (rect.width() - 2.0).max(1.0),
                        (rect.height() - 2.0).max(1.0),
                    );
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    let compose_h = 52.0_f32.min(inner.y * 0.55).max(24.0);
                    let hist_h = (ui.available_height() - compose_h - 2.0).max(8.0);
                    egui::ScrollArea::vertical()
                        .max_height(hist_h)
                        .stick_to_bottom(true)
                        .id_salt("typed_history")
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            for line in &self.typed_history {
                                ui.label(
                                    RichText::new(line)
                                        .color(Color32::WHITE)
                                        .font(FontId::monospace(12.0)),
                                );
                            }
                        });
                    // Frameless compose — sits on the purple field, bottom of the pane.
                    let edit = ui.add_sized(
                        Vec2::new(ui.available_width(), compose_h),
                        egui::TextEdit::multiline(&mut self.input)
                            .desired_rows(COMPOSE_ROWS)
                            .desired_width(f32::INFINITY)
                            .frame(false)
                            .margin(egui::Margin::ZERO)
                            .text_color(Color32::WHITE)
                            .font(FontId::monospace(14.0)),
                    );
                    if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if !ui.input(|i| i.modifiers.shift) {
                            if self.input.ends_with('\n') {
                                self.input.pop();
                            }
                            self.submit();
                        }
                    }
                });
        });
    }

    /// Magenta status: same visual weight as the channel grid, but not clickable.
    /// Left title cell spans 2 rows; right side is a 2×N field grid.
    ///
    /// Outlines are 1px white grid lines drawn once (no double borders where
    /// cells touch). The right outer edge is omitted — the clock column owns
    /// that shared seam.
    fn draw_status_grid(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, self.state_bg);

        let title_w = (rect.width() * 0.20).clamp(100.0, 160.0);
        let title_x = rect.left() + title_w;
        let mid_y = rect.top() + rect.height() * 0.5;

        let my = self.mycall_for_active();
        let owned = self.status_field_rows(&my);
        let rows = [owned.0.as_slice(), owned.1.as_slice()];
        let row_h = (rect.height() / 2.0).max(1.0);
        let fields_w = (rect.width() - title_w).max(1.0);

        // Title (full height) — galley-centered in the title cell.
        {
            let title = self.pane_title();
            let galley = painter.layout_no_wrap(
                title,
                FontId::proportional(24.0),
                self.state_fg,
            );
            let title_cell =
                Rect::from_min_max(rect.left_top(), Pos2::new(title_x, rect.bottom()));
            let pos = Pos2::new(
                title_cell.center().x - galley.size().x * 0.5,
                title_cell.center().y - galley.size().y * 0.5,
            );
            painter.galley(pos, galley, self.state_fg);
        }

        // Field labels — vertically centered in each grid cell.
        for (ri, fields) in rows.iter().enumerate() {
            if fields.is_empty() {
                continue;
            }
            let n = fields.len().max(1) as f32;
            let cell_w = (fields_w / n).max(1.0);
            let y0 = rect.top() + ri as f32 * row_h;
            for (ci, text) in fields.iter().enumerate() {
                let cell = Rect::from_min_size(
                    Pos2::new(title_x + ci as f32 * cell_w, y0),
                    Vec2::new(cell_w, row_h),
                );
                let galley = painter.layout_no_wrap(
                    (*text).clone(),
                    FontId::monospace(14.0),
                    self.state_fg,
                );
                // +2 px optical nudge — Consolas metrics sit high in the cell.
                let pos = Pos2::new(
                    cell.left() + 8.0,
                    cell.center().y - galley.size().y * 0.5 + 2.0,
                );
                painter.galley(pos, galley, self.state_fg);
            }
        }

        // 1px white hairlines — each shared edge drawn once.
        let hair = Stroke::new(1.0, Color32::WHITE);
        // Outer: left / top / bottom (skip right — clock draws that seam).
        painter.line_segment([rect.left_top(), rect.right_top()], hair);
        painter.line_segment([rect.left_top(), rect.left_bottom()], hair);
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], hair);
        // Title | fields
        painter.line_segment(
            [Pos2::new(title_x, rect.top()), Pos2::new(title_x, rect.bottom())],
            hair,
        );
        // Horizontal split between field rows
        painter.line_segment(
            [Pos2::new(title_x, mid_y), Pos2::new(rect.right(), mid_y)],
            hair,
        );
        // Vertical field dividers — per row, so uneven column counts stay aligned.
        for (ri, fields) in rows.iter().enumerate() {
            let cols = fields.len();
            if cols <= 1 {
                continue;
            }
            let cell_w = fields_w / cols as f32;
            let y0 = rect.top() + ri as f32 * row_h;
            let y1 = y0 + row_h;
            for c in 1..cols {
                let x = title_x + c as f32 * cell_w;
                painter.line_segment([Pos2::new(x, y0), Pos2::new(x, y1)], hair);
            }
        }
    }

    fn apply_buffers(&mut self, report: BufferReport) {
        if let Some(free) = report.free {
            self.tnc_free = free;
        }
        if let Some(n) = report.not_displayed {
            self.mon_not_disp = n;
        }
        if report.replace_rows {
            self.ch_not_disp.fill(0);
            self.ch_not_sent.fill(0);
            self.ch_tries.fill(0);
            self.ch_unack.fill(0);
        }
        for row in report.rows {
            let Some(idx) = self.buffer_row_index(&row) else {
                continue;
            };
            self.ch_not_disp[idx] = row.not_disp;
            self.ch_not_sent[idx] = row.not_sent;
            self.ch_tries[idx] = row.tries;
            self.ch_unack[idx] = row.unack;
        }
    }

    fn buffer_row_index(&self, row: &ChannelBuffers) -> Option<usize> {
        if row.channel >= 1 {
            let idx = row.channel as usize;
            if idx < self.ch_not_disp.len() {
                return Some(idx);
            }
        }
        if !row.peer.is_empty() {
            return self.channel_peer.iter().position(|peer| {
                peer.as_ref()
                    .is_some_and(|call| call.eq_ignore_ascii_case(&row.peer))
            });
        }
        None
    }

    fn status_field_rows(&self, my: &str) -> (Vec<String>, Vec<String>) {
        match self.active {
            // Classic GP Monitor magenta grid (COM lives on the session strip).
            ActivePane::Monitor => (
                vec![
                    format!("Monitor Mode: {}", self.monitor_mode),
                    format!("Buffers: {}", self.tnc_free),
                ],
                vec![
                    format!("Packets not yet displayed: {}", self.mon_not_disp),
                    format!("Global Mycall: {my}"),
                ],
            ),
            ActivePane::Channel(n) => {
                let idx = n as usize;
                let link = if let Some(call) =
                    self.channel_peer.get(idx).and_then(|p| p.as_ref())
                {
                    format!("Connected to {call}")
                } else if let Some(call) =
                    self.channel_connecting.get(idx).and_then(|p| p.as_ref())
                {
                    format!("Connecting to {call}")
                } else {
                    "Ready for Connect".into()
                };
                let at = |v: &[u32]| v.get(idx).copied().unwrap_or(0);
                (
                    vec![
                        link,
                        format!("My: {}", self.effective_mycall(n)),
                        format!("Buffers: {}", self.tnc_free),
                    ],
                    vec![
                        format!("Ch {n}  Not disp: {}", at(&self.ch_not_disp)),
                        format!("Not sent: {}", at(&self.ch_not_sent)),
                        format!("Tries: {}", at(&self.ch_tries)),
                        format!("Unack: {}", at(&self.ch_unack)),
                    ],
                )
            }
            ActivePane::Editor => {
                let name = if self.editor.dirty {
                    format!("{}*", self.editor.display_name())
                } else {
                    self.editor.display_name()
                };
                (
                    vec![
                        name,
                        format!("Line: {}", self.editor.cursor_line),
                        format!("Col: {}", self.editor.cursor_col),
                    ],
                    vec![
                        "Insert".into(),
                        format!("Filled Lines: {}/780", self.editor.line_count()),
                    ],
                )
            }
            ActivePane::Check => (
                vec!["Selected file:".into()],
                vec!["Reference: not computed".into()],
            ),
            ActivePane::List => (
                vec![
                    "Selected file:".into(),
                    "Reference: not computed".into(),
                ],
                vec!["File drawer:".into()],
            ),
            ActivePane::Mh => {
                let sel = self
                    .mheard_selected
                    .and_then(|i| self.mheard.entries.get(i))
                    .map(|e| e.call.as_str())
                    .unwrap_or("(none)");
                let hint = if self.mheard_spy_pick {
                    "SPY: pick a station".into()
                } else {
                    format!("{}", self.mheard.path().display())
                };
                (
                    vec![
                        format!("Stations: {}", self.mheard.listed_len()),
                        format!("Selected: {sel}"),
                    ],
                    vec![hint],
                )
            }
            ActivePane::Spy(n) => {
                let call = self.spy_call(n);
                let spy_line = if call.is_empty() {
                    format!("Spying Call (none) — press SPY to set")
                } else {
                    format!("Spying Call {call}")
                };
                (
                    vec![format!("Spy window {n}"), spy_line],
                    vec![format!("Lines: {}", self.spy_pads.get(n.saturating_sub(1) as usize).map(|p| p.lines.len()).unwrap_or(0))],
                )
            }
        }
    }

    fn draw_channel_grid(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, VGA_DARK_RED);

        let cols = 6usize;
        let rows = 2usize;
        let cell_w = rect.width() / cols as f32;
        let cell_h = rect.height() / rows as f32;

        for row in 0..rows {
            for col in 0..cols {
                let n = (row * cols + col + 1) as u8;
                let cell = Rect::from_min_size(
                    Pos2::new(
                        rect.left() + col as f32 * cell_w + 1.0,
                        rect.top() + row as f32 * cell_h + 1.0,
                    ),
                    Vec2::new(cell_w - 2.0, cell_h - 2.0),
                );
                let (label, action) = if n <= self.channels {
                    (self.channel_label(n), Some(ActivePane::Channel(n)))
                } else if n == 11 {
                    ("BBS-Menu".into(), Some(ActivePane::Check))
                } else if n == 12 {
                    ("Box-List".into(), Some(ActivePane::List))
                } else {
                    continue;
                };

                let selected = match (action, self.active) {
                    (Some(ActivePane::Channel(a)), ActivePane::Channel(b)) => a == b,
                    (Some(ActivePane::Check), ActivePane::Check) => true,
                    (Some(ActivePane::List), ActivePane::List) => true,
                    _ => false,
                };
                // Idle dark grey · connected light grey · active (any) dark red.
                let connected = matches!(action, Some(ActivePane::Channel(ch)) if self.channel_connected(ch));
                let (fill, text_color) = if selected {
                    (VGA_DARK_RED, Color32::WHITE)
                } else if connected {
                    (VGA_GRAY, Color32::BLACK)
                } else {
                    (VGA_DARK_GRAY, Color32::from_rgb(220, 220, 220))
                };
                let id = ui.id().with(("ch", n));
                let resp = ui.interact(cell, id, Sense::click());
                painter.rect_filled(cell, 0.0, fill);
                // Selected / enabled → sunken; also while the pointer is held.
                paint_bevel(
                    &painter,
                    cell,
                    selected || resp.is_pointer_button_down_on(),
                );
                painter.text(
                    cell.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(13.0),
                    text_color,
                );
                if resp.clicked() {
                    if let Some(pane) = action {
                        self.select(pane);
                    }
                }
            }
        }
    }

    fn rx_pane(&mut self, ui: &mut Ui, width: f32, height: f32) {
        if matches!(self.active, ActivePane::Editor) {
            self.editor_pane(ui, width, height);
            return;
        }
        if matches!(self.active, ActivePane::Mh) {
            self.mheard_pane(ui, width, height);
            return;
        }
        let bg = match self.active {
            ActivePane::Channel(_) => self.connect_bg,
            ActivePane::Spy(_) => Color32::from_rgb(0, 0, 128), // classic spy blue
            ActivePane::Check | ActivePane::List => Color32::from_rgb(170, 170, 170),
            _ => self.monitor_bg,
        };
        let nudge = self.session.take_scroll_nudge();
        // ScrLock off → follow last line; on → freeze (all RX panes share this).
        let stick = !self.session.scr_lock;
        let show_bar = self.session.scrollbar;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            Frame::none()
                .fill(bg)
                .stroke(Stroke::new(1.0, Color32::from_rgb(170, 170, 170)))
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_min_size(rect.size() - Vec2::splat(2.0));
                    let bar_vis = if show_bar {
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                    } else {
                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                    };
                    // Both axes: tall buffers + wide DOS ASCII-art lines.
                    egui::ScrollArea::both()
                        .id_salt("rx_full")
                        .stick_to_bottom(stick)
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(bar_vis)
                        .show(ui, |ui| {
                            if nudge != 0.0 {
                                ui.scroll_with_delta(Vec2::new(0.0, nudge));
                            }
                            ui.add(
                                egui::Label::new(self.active_layout())
                                    // No soft-wrap — column art (BBS menus, spy, etc.) must stay fixed.
                                    .wrap_mode(TextWrapMode::Extend),
                            );
                        });
                });
        });
    }

    /// Clickable MHeard list (row select → SPY confirm).
    fn mheard_pane(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let bg = Color32::from_rgb(170, 170, 170);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let show_bar = self.session.scrollbar;
        let stick = !self.session.scr_lock;
        let nudge = self.session.take_scroll_nudge();
        let pick = self.mheard_spy_pick;
        let selected = self.mheard_selected;
        let lines: Vec<(usize, String)> = self
            .mheard
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.listed())
            .map(|(i, e)| (i, e.display_line()))
            .collect();
        let mut clicked: Option<usize> = None;

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            Frame::none()
                .fill(bg)
                .stroke(Stroke::new(1.0, Color32::from_rgb(85, 85, 85)))
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_min_size(rect.size() - Vec2::splat(2.0));
                    if pick {
                        ui.label(
                            RichText::new("SPY — click a station, then confirm")
                                .color(Color32::from_rgb(0, 0, 128))
                                .strong(),
                        );
                    }
                    let bar_vis = if show_bar {
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                    } else {
                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                    };
                    egui::ScrollArea::both()
                        .id_salt("mheard_list")
                        .stick_to_bottom(stick)
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(bar_vis)
                        .show(ui, |ui| {
                            if nudge != 0.0 {
                                ui.scroll_with_delta(Vec2::new(0.0, nudge));
                            }
                            if lines.is_empty() {
                                ui.label(
                                    RichText::new(
                                        "No stations heard yet — APRS/AX.25 UI frames appear here.",
                                    )
                                    .color(Color32::BLACK)
                                    .font(FontId::monospace(14.0)),
                                );
                                return;
                            }
                            for (i, line) in &lines {
                                let is_sel = selected == Some(*i);
                                let fill = if is_sel {
                                    Color32::from_rgb(0, 0, 170)
                                } else {
                                    Color32::TRANSPARENT
                                };
                                let fg = if is_sel {
                                    Color32::WHITE
                                } else {
                                    Color32::BLACK
                                };
                                let resp = egui::Frame::none()
                                    .fill(fill)
                                    .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(line)
                                                    .color(fg)
                                                    .font(FontId::monospace(14.0)),
                                            )
                                            .wrap_mode(TextWrapMode::Extend)
                                            .sense(Sense::click()),
                                        )
                                    })
                                    .response;
                                // Click on frame or label.
                                let hit = ui.interact(
                                    resp.rect,
                                    ui.id().with(("mh_row", *i)),
                                    Sense::click(),
                                );
                                if hit.clicked() || resp.clicked() {
                                    clicked = Some(*i);
                                }
                            }
                        });
                });
        });

        if let Some(i) = clicked {
            self.mheard_selected = Some(i);
            if let Some(e) = self.mheard.entries.get(i) {
                self.status = format!("MHeard: {}", e.call);
                if self.mheard_spy_pick {
                    self.open_mheard_spy_confirm(e.call.clone());
                }
            }
        }
    }

    /// Full-pane monospace editor (CP437 GPI files).
    fn editor_pane(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let bg = Color32::from_rgb(0, 0, 0);
        let fg = Color32::from_rgb(170, 170, 170);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            Frame::none()
                .fill(bg)
                .stroke(Stroke::new(1.0, Color32::from_rgb(170, 170, 170)))
                .inner_margin(4.0)
                .show(ui, |ui| {
                    ui.set_min_size(rect.size() - Vec2::splat(2.0));
                    let avail = ui.available_size();
                    let rows = ((avail.y / 16.0).floor() as usize).clamp(8, 60);
                    let before = self.editor.text.clone();
                    let output = egui::TextEdit::multiline(&mut self.editor.text)
                        .id_salt("gp_editor")
                        .font(FontId::monospace(14.0))
                        .text_color(fg)
                        .frame(false)
                        .desired_width(avail.x)
                        .desired_rows(rows)
                        .lock_focus(true)
                        .show(ui);
                    if output.response.changed() && self.editor.text != before {
                        self.editor.dirty = true;
                    }
                    if let Some(cursor) = output.cursor_range {
                        let cc = cursor.as_sorted_char_range().start;
                        let snap = self.editor.text.clone();
                        self.editor.update_cursor_from_galley(&snap, cc);
                    }
                });
        });
    }

    fn session_bar(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.painter_at(rect).rect_filled(rect, 0.0, VGA_GRAY);

        let on = VGA_GREEN; // same as clock ♫ when on (not bright LightGreen)
        let off = VGA_GRAY;
        let text_c = Color32::BLACK;
        let y = rect.top() + 2.0;
        let h = (rect.height() - 4.0).max(16.0);
        let gap = 2.0;

        let mut clicks: Vec<SessionClick> = Vec::new();

        let mut paint_cell = |ui: &mut Ui,
                              origin_x: f32,
                              label: &str,
                              lit: bool,
                              action: SessionClick,
                              w: f32,
                              glyph: CellGlyph|
         -> f32 {
            let cell = Rect::from_min_size(Pos2::new(origin_x, y), Vec2::new(w, h));
            let id = ui.id().with(("sess", action, label));
            let resp = ui.interact(cell, id, Sense::click());
            let painter = ui.painter();
            painter.rect_filled(cell, 0.0, if lit { on } else { off });
            // Lit / enabled → sunken bevel (classic GP toggle look).
            paint_bevel(painter, cell, lit || resp.is_pointer_button_down_on());
            match glyph {
                CellGlyph::Text => {
                    painter.text(
                        cell.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.0),
                        text_c,
                    );
                }
                CellGlyph::TriUp | CellGlyph::TriDown => {
                    // Drawn triangles — egui default fonts often lack ▲/▼ (tofu boxes).
                    let c = cell.center();
                    let s = 5.0;
                    let pts = if matches!(glyph, CellGlyph::TriUp) {
                        vec![
                            Pos2::new(c.x, c.y - s),
                            Pos2::new(c.x - s, c.y + s * 0.7),
                            Pos2::new(c.x + s, c.y + s * 0.7),
                        ]
                    } else {
                        vec![
                            Pos2::new(c.x, c.y + s),
                            Pos2::new(c.x - s, c.y - s * 0.7),
                            Pos2::new(c.x + s, c.y - s * 0.7),
                        ]
                    };
                    painter.add(egui::Shape::convex_polygon(pts, text_c, Stroke::NONE));
                }
            }
            if resp.clicked() {
                clicks.push(action);
            }
            w + gap
        };

        // Trailing controls always visible on the right.
        let trail = [
            (
                "ScrLock",
                self.session.scr_lock,
                SessionClick::ScrLock,
                54.0_f32,
                CellGlyph::Text,
            ),
            ("", false, SessionClick::ScrollUp, 22.0, CellGlyph::TriUp),
            ("", false, SessionClick::ScrollDown, 22.0, CellGlyph::TriDown),
            (
                "B",
                self.session.scrollbar,
                SessionClick::Scrollbar,
                22.0,
                CellGlyph::Text,
            ),
        ];
        let trail_w: f32 = trail.iter().map(|(_, _, _, w, _)| w + gap).sum();
        let mut tx = rect.right() - trail_w - 2.0;
        for (label, lit, action, w, glyph) in trail {
            tx += paint_cell(ui, tx, label, lit, action, w, glyph);
        }

        // Monitor: spy slots + COM sit just left of ScrLock (right-justified).
        let com_badge = if self.serial_baud == 0 {
            format!("TCP {}", self.serial_port)
        } else {
            format!("{} {}", self.serial_port, self.serial_baud)
        };
        let spy_cell_w = 20.0_f32;
        let spy_n = if matches!(self.active, ActivePane::Monitor | ActivePane::Spy(_)) {
            self.channels.min(10)
        } else {
            0
        };
        let badge_w = (com_badge.len() as f32 * 7.2 + 12.0).clamp(72.0, 180.0);
        let right_mid_w = if spy_n > 0 {
            spy_n as f32 * (spy_cell_w + gap) + badge_w + gap
        } else {
            0.0
        };

        // Leading controls stay on the left.
        let mut x = rect.left() + 2.0;
        let left_limit =
            (rect.right() - trail_w - right_mid_w - 8.0).max(x + 40.0);

        let qrg = self.session.qrg_label();
        let ct = self.session.ctext_label();
        let lead_owned = [
            (qrg.as_str(), false, SessionClick::Qrg, 86.0_f32),
            (ct.as_str(), false, SessionClick::Ctext, 70.0),
            ("äöü", self.session.extended_ascii, SessionClick::Ascii, 34.0),
            (
                self.session.line_step.label(),
                matches!(self.session.line_step, super::session::LineStep::Smooth),
                SessionClick::LineStep,
                26.0,
            ),
            ("Echo", self.session.echo, SessionClick::Echo, 38.0),
            ("♪", self.session.bell, SessionClick::Bell, 26.0),
            ("Clear", false, SessionClick::Clear, 46.0),
        ];
        for (label, lit, action, w) in lead_owned {
            if x + w > left_limit {
                break;
            }
            x += paint_cell(ui, x, label, lit, action, w, CellGlyph::Text);
        }

        match self.active {
            ActivePane::Monitor | ActivePane::Spy(_) => {
                let trail_left = rect.right() - trail_w - 2.0;
                let mut rx = (trail_left - right_mid_w).max(x + 4.0);
                // Only lit while that spy window is active (off when on channel/monitor/…).
                let active_spy = match self.active {
                    ActivePane::Spy(n) => Some(n),
                    _ => None,
                };
                for n in 1..=spy_n {
                    let lit = active_spy == Some(n);
                    rx += paint_cell(
                        ui,
                        rx,
                        &format!("{n}"),
                        lit,
                        SessionClick::Spy(n),
                        spy_cell_w,
                        CellGlyph::Text,
                    );
                }
                let _ = paint_static_cell(
                    ui,
                    rx,
                    y,
                    h,
                    gap,
                    &com_badge,
                    badge_w,
                    self.serial_open,
                );
            }
            ActivePane::Channel(_) => {
                for (label, lit, action, w) in [
                    ("7+", self.session.seven_plus, SessionClick::SevenPlus, 28.0_f32),
                    ("Remote", self.session.remote, SessionClick::Remote, 50.0),
                    ("BIN-RX", self.session.bin_rx, SessionClick::BinRx, 50.0),
                    ("BIN-TX", self.session.bin_tx, SessionClick::BinTx, 50.0),
                ] {
                    if x + w > left_limit {
                        break;
                    }
                    x += paint_cell(ui, x, label, lit, action, w, CellGlyph::Text);
                }
            }
            _ => {}
        }

        for action in clicks {
            self.handle_session_click(action);
        }
    }

    fn persist_session_chrome(&mut self) {
        self.cfg.session.chrome = self.session.to_config();
        if let Err(e) = self.cfg.persist_chrome(&self.config_path) {
            self.status = format!("config: {e}");
            self.log_event(format!("persist chrome: {e}"));
        }
    }

    fn handle_session_click(&mut self, action: SessionClick) {
        match action {
            SessionClick::Qrg => self.session.open_qrg_dialog(),
            SessionClick::Ctext => self.pick_ctext_file(),
            SessionClick::Ascii => {
                self.session.extended_ascii = !self.session.extended_ascii;
                self.status = if self.session.extended_ascii {
                    "ASCII: extended DOS (äöü)".into()
                } else {
                    "ASCII: plain".into()
                };
                self.persist_session_chrome();
            }
            SessionClick::LineStep => {
                self.session.line_step = self.session.line_step.cycle();
                self.status = format!("line step: {:?}", self.session.line_step);
                self.persist_session_chrome();
            }
            SessionClick::Echo => {
                self.session.echo = !self.session.echo;
                self.status = format!("Echo {}", on_off(self.session.echo));
                self.persist_session_chrome();
            }
            SessionClick::Bell => {
                self.session.bell = !self.session.bell;
                self.status = format!("Bell {}", on_off(self.session.bell));
                if self.session.bell {
                    play_bell_wav(&self.bell_wav);
                }
                self.persist_session_chrome();
            }
            SessionClick::Clear => self.clear_active_pad(),
            SessionClick::Spy(n) => {
                self.session.spy_slot = Some(n);
                self.active = ActivePane::Spy(n);
                let call = self.spy_call(n);
                self.status = if call.is_empty() {
                    format!("Spy {n} — no call set")
                } else {
                    format!("Spying Call {call}")
                };
            }
            SessionClick::SevenPlus => {
                self.session.seven_plus = !self.session.seven_plus;
                self.status = format!("7+ {}", on_off(self.session.seven_plus));
                self.log_event(format!("7Plus {}", on_off(self.session.seven_plus)));
                self.persist_session_chrome();
            }
            SessionClick::Remote => {
                self.session.remote = !self.session.remote;
                self.status = format!("Remote {}", on_off(self.session.remote));
                self.persist_session_chrome();
            }
            SessionClick::BinRx => {
                self.session.bin_rx = !self.session.bin_rx;
                self.status = format!("BIN-RX {}", on_off(self.session.bin_rx));
            }
            SessionClick::BinTx => {
                self.session.bin_tx = !self.session.bin_tx;
                self.status = format!("BIN-TX {}", on_off(self.session.bin_tx));
            }
            SessionClick::ScrLock => {
                self.session.scr_lock = !self.session.scr_lock;
                self.status = format!("ScrLock {}", on_off(self.session.scr_lock));
                self.persist_session_chrome();
            }
            SessionClick::ScrollUp => {
                if !self.session.scr_lock {
                    self.session.nudge_lines(-1);
                }
            }
            SessionClick::ScrollDown => {
                if !self.session.scr_lock {
                    self.session.nudge_lines(1);
                }
            }
            SessionClick::Scrollbar => {
                self.session.scrollbar = !self.session.scrollbar;
                self.status = format!("scrollbar {}", on_off(self.session.scrollbar));
            }
        }
    }

    fn pick_ctext_file(&mut self) {
        let start = self
            .session
            .ctext_path
            .parent()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| Some(crate::paths::data_dir()));
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Connect text (GPI)", &["gpi", "GPI", "txt"])
            .set_title("Select Connect text (CTEXT)");
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.session.set_ctext_path(path);
            self.status = format!("CT: {}", self.session.ctext_display_name());
            self.log_event(format!("Connect text → {}", self.session.ctext_display_name()));
            self.persist_session_chrome();
        }
    }

    fn qrg_dialog(&mut self, ctx: &Context) {
        if !self.session.show_qrg_dialog {
            return;
        }
        let mut open = true;
        egui::Window::new("QRG — frequency (MHz)")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("QRG:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session.qrg_edit)
                            .desired_width(100.0)
                            .font(FontId::monospace(16.0)),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        match self.session.set_qrg_from_edit() {
                            Ok(()) => {
                                self.status = self.session.qrg_label();
                                self.log_event(self.session.qrg_label());
                                self.persist_session_chrome();
                            }
                            Err(e) => self.status = e,
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.session.show_qrg_dialog = false;
                    }
                });
            });
        if !open {
            self.session.show_qrg_dialog = false;
        }
    }

    fn mycall_slot_for_active(&self) -> u8 {
        match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        }
    }

    fn mycall_for_active(&self) -> String {
        self.effective_mycall(self.mycall_slot_for_active())
    }

    fn open_mycall_dialog(&mut self) {
        let slot = self.mycall_slot_for_active();
        let current = self.cfg.mycall_for_slot(slot).to_string();
        self.session.open_mycall_dialog(slot, &current);
    }

    fn apply_mycall_from_edit(&mut self) -> Result<(), String> {
        let slot = self.session.mycall_slot;
        let edited = self.session.mycall_edit.clone();
        let call = if slot == 0 {
            self.cfg.set_mycall_for_slot(0, &edited)?
        } else {
            self.claim_identity(slot, &edited)?
        };
        self.cfg.persist_mycall(&self.config_path)?;
        self.session.show_mycall_dialog = false;
        let where_ = if slot == 0 {
            if !self.any_channel_busy() {
                self.restore_global_identities();
            }
            "Monitor / global".into()
        } else {
            format!("channel {slot}")
        };
        self.status = format!("MYCALL {call} ({where_})");
        if slot == 0 {
            self.pads[0].push(PadKind::System, version::startup_local_call(&call));
        }
        Ok(())
    }

    fn open_spy_call_dialog(&mut self) {
        let slot = match self.active {
            ActivePane::Spy(n) => n,
            _ => self.session.spy_slot.unwrap_or(1),
        };
        let slot = slot.clamp(1, 10);
        self.active = ActivePane::Spy(slot);
        self.session.spy_slot = Some(slot);
        let current = self.spy_call(slot).to_string();
        self.session.open_spy_dialog(slot, &current);
    }

    /// Classic GP: SPY on MHeard → pick a row → confirm → first empty spy slot.
    fn spy_from_mheard(&mut self) {
        self.mheard_spy_pick = true;
        if let Some(i) = self.mheard_selected {
            if let Some(e) = self.mheard.entries.get(i) {
                self.open_mheard_spy_confirm(e.call.clone());
                return;
            }
        }
        self.status = "Select a call in MHeard for Spy".into();
    }

    fn open_mheard_spy_confirm(&mut self, call: String) {
        self.mheard_spy_pending = call;
        self.show_mheard_spy_confirm = true;
    }

    fn first_empty_spy_slot(&self) -> Option<u8> {
        self.cfg
            .session
            .spy_calls
            .iter()
            .enumerate()
            .find(|(_, c)| c.is_empty())
            .map(|(i, _)| (i as u8) + 1)
    }

    /// First disconnected channel `1..=channels`.
    fn first_free_channel(&self) -> Option<u8> {
        (1..=self.channels).find(|&n| !self.channel_connected(n))
    }

    /// Parse `CALL` or `CALL,VIA[,VIA…]` into dest + digi path.
    fn parse_connect_target(raw: &str) -> Result<(String, Vec<String>), String> {
        let parts: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return Err("connect: empty call".into());
        }
        let dest = crate::config::normalize_mycall(parts[0])?;
        let mut vias = Vec::new();
        for p in &parts[1..] {
            vias.push(crate::config::normalize_mycall(p)?);
        }
        Ok((dest, vias))
    }

    /// WA8DED / TF hostmode connect: `C CALL` or `C CALL digi1 digi2…`.
    /// Does not mark the channel connected until the TNC reports CONNECTED.
    ///
    /// - Foreign call / directed own SSID (`NL0MSK-10`) → connect that name as typed.
    /// - Own global MYCALL (loopback) → caller stays on `channel`; first free channel
    ///   answers as `{mycall}-{n}` because the caller channel is busy.
    fn begin_connect(&mut self, channel: u8, dest: &str, vias: &[String]) {
        if !(1..=self.channels).contains(&channel) {
            self.status = format!("connect: bad channel {channel}");
            return;
        }

        let mut dest = dest.to_string();
        let mut vias = vias.to_vec();
        let primary = self.cfg.primary_mycall().to_string();
        let bare_own_global = dest.eq_ignore_ascii_case(&primary)
            || (self.cfg.is_own_call(&dest) && !dest.contains('-'));
        // Specific name we own (NL0MSK-10, or any dedicated slot call).
        let directed_own = !dest.eq_ignore_ascii_case(&primary)
            && (self.cfg.is_own_call(&dest) || self.channel_owning_call(&dest).is_some());

        let mut self_connect = false;
        if bare_own_global {
            // Loopback to global: caller busy on `channel`; answer as {base}-{free}.
            let Some(answer_ch) = (1..=self.channels)
                .find(|&n| n != channel && !self.channel_connected(n))
            else {
                self.status = "no free channel for self-connect".into();
                return;
            };
            match self.assign_channel_ssid(answer_ch) {
                Ok(answer_call) => {
                    dest = answer_call.clone();
                    vias.clear();
                    self_connect = true;
                    self.self_connect_pair = Some((channel, answer_ch));
                    let caller_id = self.effective_mycall(channel);
                    if let Some(c) = self.channel_connecting.get_mut(answer_ch as usize) {
                        *c = Some(caller_id);
                    }
                    if let Some(pad) = self.pads.get_mut(answer_ch as usize) {
                        pad.push(
                            PadKind::System,
                            format!(
                                "*** answer side {answer_call} (self-connect from ch{channel})"
                            ),
                        );
                    }
                }
                Err(e) => {
                    self.status = e;
                    return;
                }
            }
        } else if directed_own {
            // Dialled a specific slot name. If that channel is busy, redirect to a
            // free channel with a temporary {base}-{n} (still only one call each).
            if let Some(owner) = self.channel_owning_call(&dest) {
                if self.channel_connected(owner) {
                    match self.redirect_busy_dedicated(owner) {
                        Some((free, temp_call)) => {
                            dest = temp_call.clone();
                            vias.clear();
                            if let Some(pad) = self.pads.get_mut(free as usize) {
                                pad.push(
                                    PadKind::System,
                                    format!(
                                        "*** redirect {owner} busy → {temp_call} on ch{free}"
                                    ),
                                );
                            }
                        }
                        None => {
                            self.status = format!("{dest} busy — no free channel");
                            return;
                        }
                    }
                } else {
                    self.push_channel_identity(owner);
                }
            }
        }

        self.session.spy_slot = None;
        self.active = ActivePane::Channel(channel);
        let idx = channel as usize;
        if let Some(peer) = self.channel_peer.get_mut(idx) {
            *peer = None;
        }
        if let Some(c) = self.channel_connecting.get_mut(idx) {
            *c = Some(dest.clone());
        }
        if let Some(vs) = self.ax25_vs.get_mut(idx) {
            *vs = 0;
        }
        if let Some(vr) = self.ax25_vr.get_mut(idx) {
            *vr = 0;
        }
        if let Some(pad) = self.pads.get_mut(idx) {
            let via_note = if vias.is_empty() {
                String::new()
            } else {
                format!(" via {}", vias.join(","))
            };
            let kind = if self_connect {
                "self-connect"
            } else {
                "connecting"
            };
            pad.push(
                PadKind::System,
                format!("*** {kind} to {dest}{via_note}"),
            );
        }
        if self.kiss_mode() {
            let src = self.effective_mycall(channel);
            let frame = ax25::build_sabm(&dest, &src);
            self.tx_kiss_payload(0, &frame);
            self.push_kiss_mon(
                PadKind::Own,
                &src,
                &dest,
                &vias,
                Ax25FrameType::Sabm,
                0,
                0,
                true,
                true,
                None,
            );
            if self_connect {
                // Soundmodem / half-duplex will not RX our own SABM — finish the
                // answer side locally (UA on air + both channels linked + CTEXT).
                if let Some((caller, answer)) = self.self_connect_pair {
                    self.kiss_finish_self_connect(caller, answer);
                }
            } else {
                self.kiss_arm_sabm_timer(channel);
                self.status = format!("KISS SABM → {dest}");
            }
        } else {
            let cmd = if vias.is_empty() {
                format!("C {dest}")
            } else {
                format!("C {dest} {}", vias.join(" "))
            };
            self.tx_modem_line(channel, &cmd);
            self.status = if self_connect {
                format!("Self-connect ch{channel} → {dest}")
            } else {
                format!("Connecting to {dest}")
            };
        }
    }

    /// MHeard CONNECT: selected call → first free channel → `C call`.
    fn connect_from_mheard(&mut self) {
        let Some(i) = self.mheard_selected else {
            self.status = "Select a call in MHeard to Connect".into();
            return;
        };
        let Some(e) = self.mheard.entries.get(i) else {
            self.status = "Select a call in MHeard to Connect".into();
            return;
        };
        let call = e.call.clone();
        let Some(ch) = self.first_free_channel() else {
            self.status = "no free channel for Connect".into();
            return;
        };
        match crate::config::normalize_mycall(&call) {
            Ok(dest) => self.begin_connect(ch, &dest, &[]),
            Err(err) => self.status = err,
        }
    }

    fn open_connect_dialog_for_channel(&mut self, channel: u8) {
        let ch = channel.clamp(1, self.channels);
        self.session.open_connect_dialog(ch, "");
        self.connect_dialog_focus = true;
        self.status = format!("Connect — enter call for ch{ch}");
    }

    fn apply_connect_from_edit(&mut self) -> Result<(), String> {
        let ch = self.session.connect_channel.clamp(1, self.channels);
        let (dest, vias) = Self::parse_connect_target(&self.session.connect_edit)?;
        self.session.show_connect_dialog = false;
        self.begin_connect(ch, &dest, &vias);
        Ok(())
    }

    fn connect_dialog(&mut self, ctx: &Context) {
        if !self.session.show_connect_dialog {
            return;
        }
        let ch = self.session.connect_channel;
        let center = ctx.screen_rect().center();
        // Do not use `.open(&mut)` — egui memory can keep the window closed.
        egui::Window::new(format!("Connect — channel {ch}"))
            .id(egui::Id::new(("gp_connect_dialog", ch)))
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(center)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.label("Whom to connect to?");
                ui.label(
                    RichText::new("Callsign — optional via after comma (CALL,VIA)")
                        .small()
                        .color(Color32::DARK_GRAY),
                );
                let te = ui.add(
                    egui::TextEdit::singleline(&mut self.session.connect_edit)
                        .desired_width(220.0)
                        .font(FontId::monospace(16.0))
                        .hint_text("N0CALL"),
                );
                if self.connect_dialog_focus {
                    te.request_focus();
                    self.connect_dialog_focus = false;
                }
                let enter = te.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Connect").clicked() || enter {
                        if let Err(e) = self.apply_connect_from_edit() {
                            self.status = e;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.session.show_connect_dialog = false;
                        self.status = "Connect cancelled".into();
                    }
                });
            });
    }

    fn assign_spy_first_empty(&mut self, call: &str) -> Result<u8, String> {
        let call = crate::config::normalize_mycall(call)?;
        self.cfg.normalize_spy_calls();
        let Some(slot) = self.first_empty_spy_slot() else {
            return Err("all spy slots are full".into());
        };
        let idx = slot.saturating_sub(1) as usize;
        if let Some(c) = self.cfg.session.spy_calls.get_mut(idx) {
            *c = call.clone();
        }
        self.cfg.persist_spy_calls(&self.config_path)?;
        Ok(slot)
    }

    fn mheard_spy_confirm_dialog(&mut self, ctx: &Context) {
        if !self.show_mheard_spy_confirm {
            return;
        }
        let call = self.mheard_spy_pending.clone();
        let mut open = true;
        egui::Window::new("Start Spy")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Start Spy for {call}"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        match self.assign_spy_first_empty(&call) {
                            Ok(slot) => {
                                self.show_mheard_spy_confirm = false;
                                self.mheard_spy_pick = false;
                                self.session.spy_slot = Some(slot);
                                self.active = ActivePane::Spy(slot);
                                self.status = format!("Spying Call {call}");
                            }
                            Err(e) => self.status = e,
                        }
                    }
                    if ui.button("No").clicked() {
                        // Keep pick mode so another MHeard row can be chosen.
                        self.show_mheard_spy_confirm = false;
                        self.mheard_spy_pending.clear();
                        self.status = "Select a call in MHeard for Spy".into();
                    }
                    if ui.button("Abort").clicked() {
                        self.show_mheard_spy_confirm = false;
                        self.mheard_spy_pick = false;
                        self.mheard_spy_pending.clear();
                        self.status = "Spy from MHeard aborted".into();
                    }
                });
            });
        if !open {
            self.show_mheard_spy_confirm = false;
            self.mheard_spy_pick = false;
            self.mheard_spy_pending.clear();
        }
    }

    fn kill_spy_call(&mut self) {
        let slot = match self.active {
            ActivePane::Spy(n) => n,
            _ => match self.session.spy_slot {
                Some(n) => n,
                None => {
                    self.status = "KILL SPY — open a spy window first".into();
                    return;
                }
            },
        };
        let idx = slot.saturating_sub(1) as usize;
        self.cfg.normalize_spy_calls();
        if let Some(c) = self.cfg.session.spy_calls.get_mut(idx) {
            c.clear();
        }
        if let Err(e) = self.cfg.persist_spy_calls(&self.config_path) {
            self.status = format!("config: {e}");
        } else {
            self.status = format!("Spy {slot} cleared");
        }
    }

    fn apply_spy_call_from_edit(&mut self) -> Result<(), String> {
        let slot = self.session.spy_slot.unwrap_or(1).clamp(1, 10);
        let raw = self.session.spy_edit.trim();
        self.cfg.normalize_spy_calls();
        let idx = slot.saturating_sub(1) as usize;
        if raw.is_empty() {
            if let Some(c) = self.cfg.session.spy_calls.get_mut(idx) {
                c.clear();
            }
        } else {
            let call = crate::config::normalize_mycall(raw)?;
            if let Some(c) = self.cfg.session.spy_calls.get_mut(idx) {
                *c = call;
            }
        }
        self.cfg.persist_spy_calls(&self.config_path)?;
        self.session.show_spy_dialog = false;
        let call = self.spy_call(slot);
        self.status = if call.is_empty() {
            format!("Spy {slot} — no call set")
        } else {
            format!("Spying Call {call}")
        };
        Ok(())
    }

    fn spy_dialog(&mut self, ctx: &Context) {
        if !self.session.show_spy_dialog {
            return;
        }
        let slot = self.session.spy_slot.unwrap_or(1);
        let mut open = true;
        egui::Window::new(format!("SPY — window {slot}"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Call to spy (empty = off):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.session.spy_edit)
                        .desired_width(120.0)
                        .font(FontId::monospace(16.0)),
                );
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        if let Err(e) = self.apply_spy_call_from_edit() {
                            self.status = e;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.session.show_spy_dialog = false;
                    }
                });
            });
        if !open {
            self.session.show_spy_dialog = false;
        }
    }

    fn mycall_dialog(&mut self, ctx: &Context) {
        if !self.session.show_mycall_dialog {
            return;
        }
        let title = if self.session.mycall_slot == 0 {
            "MYCALL — Monitor (global)".to_string()
        } else {
            format!("MYCALL — Channel {}", self.session.mycall_slot)
        };
        let mut open = true;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Call:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session.mycall_edit)
                            .desired_width(120.0)
                            .font(FontId::monospace(16.0)),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        if let Err(e) = self.apply_mycall_from_edit() {
                            self.status = e;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.session.show_mycall_dialog = false;
                    }
                });
            });
        if !open {
            self.session.show_mycall_dialog = false;
        }
    }

    fn icon_strip(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, VGA_DARK_RED);

        if self.icon_tex.is_empty() {
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "No icons — expected data/ICONS.GP ({}×{})",
                            ICON_WIDTH, ICON_HEIGHT
                        ))
                        .color(Color32::YELLOW),
                    );
                });
            });
            return;
        }

        let toolbar = match self.active {
            ActivePane::Channel(n) => toolbar_for_channel(self.channel_linked(n)),
            other => toolbar_for(other),
        };
        let n = toolbar.len().max(1) as f32;
        // Fill strip height with 1 px air above/below; width follows aspect.
        let vpad = 1.0;
        let icon_h = (rect.height() - vpad * 2.0).max(1.0);
        let scale = icon_h / ICON_HEIGHT as f32;
        let icon_w = ICON_WIDTH as f32 * scale;
        let gap = ((rect.width() - n * icon_w) / (n + 1.0)).max(4.0);

        let mut clicked: Option<&'static str> = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            let mut x = rect.left() + gap;
            let y = rect.top() + vpad;
            for &icon_idx in toolbar {
                let label = ICON_SLOT_LABELS.get(icon_idx).copied().unwrap_or("?");
                let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::new(icon_w, icon_h));
                let id = ui.id().with(("icon", icon_idx, label));
                let resp = ui.interact(cell, id, Sense::click());
                // Grey raised button face (classic GP), then the bitmap.
                ui.painter().rect_filled(cell, 0.0, VGA_GRAY);
                if label == "EMPTY" || self.icon_tex.get(icon_idx).is_none() {
                    // empty slot keeps grey face on dark-red strip
                } else if let Some(tex) = self.icon_tex.get(icon_idx).cloned() {
                    ui.painter().image(
                        tex.id(),
                        cell,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                let icon_on = match (label, self.active) {
                    ("MONITOR", ActivePane::Monitor) => true,
                    ("EDIT", ActivePane::Editor) => true,
                    ("MHEARD", ActivePane::Mh) => true,
                    ("SPY", ActivePane::Spy(_)) => true,
                    ("SPY", ActivePane::Mh) if self.mheard_spy_pick => true,
                    ("CHECK", ActivePane::Check) | ("BBS", ActivePane::Check) => true,
                    ("LIST", ActivePane::List) => true,
                    _ => false,
                };
                paint_bevel(
                    ui.painter(),
                    cell,
                    icon_on || resp.is_pointer_button_down_on(),
                );
                if resp.clicked() {
                    clicked = Some(label);
                }
                x += icon_w + gap;
            }
        });

        if let Some(label) = clicked {
            match label {
                "MONITOR" => self.select(ActivePane::Monitor),
                "EDIT" => self.open_editor_default(),
                "SAVE" => {
                    if !matches!(self.active, ActivePane::Editor) {
                        self.open_editor_default();
                    }
                    self.editor_save();
                }
                "LOAD" => self.editor_load_dialog(),
                "MHEARD" => self.select(ActivePane::Mh),
                "SPY" => {
                    if matches!(self.active, ActivePane::Mh) {
                        self.spy_from_mheard();
                    } else {
                        self.open_spy_call_dialog();
                    }
                }
                "KILL SPY" => self.kill_spy_call(),
                "EXIT" => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                // Classic GP shelled to DOS; here we just minimize (can't stay resident).
                "DOS" => {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    self.status = "minimized".into();
                }
                "MYCALL" => self.open_mycall_dialog(),
                "CONNECT" => match self.active {
                    ActivePane::Mh => self.connect_from_mheard(),
                    ActivePane::Channel(n) => self.open_connect_dialog_for_channel(n),
                    _ => {
                        if let Some(ch) = self.first_free_channel() {
                            self.open_connect_dialog_for_channel(ch);
                        } else {
                            self.status = "no free channel for Connect".into();
                        }
                    }
                },
                "DISCONNECT" => {
                    if let ActivePane::Channel(n) = self.active {
                        self.disconnect_channel(n);
                    } else {
                        self.status = "DISCONNECT — select a channel".into();
                    }
                }
                other => self.status = format!("icon: {other}"),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellGlyph {
    Text,
    TriUp,
    TriDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SessionClick {
    Qrg,
    Ctext,
    Ascii,
    LineStep,
    Echo,
    Bell,
    Clear,
    Spy(u8),
    SevenPlus,
    Remote,
    BinRx,
    BinTx,
    ScrLock,
    ScrollUp,
    ScrollDown,
    Scrollbar,
}

fn on_off(v: bool) -> &'static str {
    if v { "ON" } else { "OFF" }
}

/// Local 24-hour `hh:mm` with leading zeros, inserted after `fm`.
fn stamp_monitor_fm(line: &str) -> String {
    let hhmm = Local::now().format("%H:%M").to_string();
    ax25::insert_monitor_time(line, &hhmm)
}

fn is_hhmm_token(token: &str) -> bool {
    let Some((h, m)) = token.split_once(':') else {
        return false;
    };
    h.len() == 2
        && m.len() == 2
        && h.bytes().all(|b| b.is_ascii_digit())
        && m.bytes().all(|b| b.is_ascii_digit())
}


/// Best-effort source call from monitor / APRS-ish text (`fm CALL`, `CALL>`).
fn extract_heard_call(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if let Some(pos) = lower.find("fm ") {
        let after = text[pos + 3..].trim_start();
        let mut tokens = after.split_whitespace();
        let mut token = tokens.next()?;
        if is_hhmm_token(token) {
            token = tokens.next()?;
        }
        let token = token
            .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
        if !token.is_empty() {
            return Some(token.to_ascii_uppercase());
        }
    }
    if let Some((src, _)) = text.split_once('>') {
        let token = src
            .trim()
            .split_whitespace()
            .last()?
            .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
        if !token.is_empty() && token.len() <= 9 {
            return Some(token.to_ascii_uppercase());
        }
    }
    None
}

/// Session-strip badge with no click handler (COM open indicator).
fn paint_static_cell(
    ui: &mut Ui,
    origin_x: f32,
    y: f32,
    h: f32,
    gap: f32,
    label: &str,
    w: f32,
    lit: bool,
) -> f32 {
    let cell = Rect::from_min_size(Pos2::new(origin_x, y), Vec2::new(w, h));
    let _ = ui.interact(cell, ui.id().with(("sess-static", label)), Sense::hover());
    let painter = ui.painter();
    let fill = if lit { VGA_GREEN } else { VGA_GRAY };
    painter.rect_filled(cell, 0.0, fill);
    paint_bevel(painter, cell, lit);
    painter.text(
        cell.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        Color32::BLACK,
    );
    w + gap
}

fn looks_like_monitor_mode(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.len() <= 8
        && t.chars()
            .all(|c| matches!(c, 'I' | 'U' | 'S' | 'C' | 'N' | '+' | '-' | 'i' | 'u' | 's' | 'c' | 'n'))
}

/// Modem / hostmode link chatter (may arrive as Monitor/Data, not only LinkStatus).
fn looks_like_link_status(s: &str) -> bool {
    let u = s.to_ascii_uppercase();
    u.contains("CONNECTED")
        || u.contains("DISCONNECTED")
        || u.contains("LINK FAIL")
        || u.contains("LINK FAILURE")
        || u.contains("BUSY FROM")
        || u.contains("FAILURE WITH")
        || u.contains("RETRY OUT")
        || u.contains("NO ANSWER")
}

/// `(2) CONNECTED to CALL` / `2: CONNECTED …` → channel number.
fn parse_hostmode_channel_tag(text: &str) -> Option<u8> {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix('(') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with(')') {
            return digits.parse().ok();
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let rest = &t[digits.len()..];
        if rest.starts_with(':') || rest.starts_with(' ') {
            return digits.parse().ok();
        }
    }
    None
}

fn parse_monitor_set(line: &str) -> Option<String> {
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    let cmd = parts[0].to_ascii_uppercase();
    if cmd == "M" || cmd == "MONITOR" {
        if parts.len() >= 2 {
            return Some(parts[1].to_ascii_uppercase());
        }
    }
    None
}

fn load_icon_textures(ctx: &Context) -> Vec<TextureHandle> {
    let path = crate::paths::data_file("ICONS.GP");
    if !path.is_file() {
        return Vec::new();
    }
    tracing::info!(file = %path.display(), "loading ICONS.GP");
    let Ok(lib) = IconLibrary::load(&path) else {
        return Vec::new();
    };

    lib.icons
        .iter()
        .enumerate()
        .map(|(i, icon)| {
            let color =
                ColorImage::from_rgba_unmultiplied([icon.width, icon.height], &icon.rgba8());
            ctx.load_texture(format!("gp_icon_{i}"), color, TextureOptions::NEAREST)
        })
        .collect()
}

/// Prefer a real Windows console face for Monitor / Channel / BBS / Spy RX text
/// so DOS-era ASCII art and box-drawing line up on a fixed grid.
fn install_console_font(ctx: &Context) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\consola.ttf",      // Consolas
        r"C:\Windows\Fonts\CascadiaMono.ttf", // Cascadia Mono
        r"C:\Windows\Fonts\lucon.ttf",        // Lucida Console
        r"C:\Windows\Fonts\cour.ttf",         // Courier New
    ];
    let Some(path) = CANDIDATES
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists())
    else {
        tracing::warn!("no system console TTF found; keeping egui default monospace");
        return;
    };
    let Ok(bytes) = std::fs::read(path) else {
        tracing::warn!("failed to read console font {}", path.display());
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("gp_console".into(), FontData::from_owned(bytes));
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "gp_console".into());
    ctx.set_fonts(fonts);
    tracing::info!("console font: {}", path.display());
}

/// Draw the tall clock column. Returns `true` when the ♫ gong button was clicked.
fn draw_analog_clock(ui: &mut Ui, rect: Rect, gong_on: bool) -> bool {
    let painter = ui.painter_at(rect);
    // DOSBox / V1.63: clock column is dark green, not monitor blue.
    painter.rect_filled(rect, 0.0, VGA_DARK_GREEN);

    let now = Local::now();
    // Dial fills most of the slim column; badges sit centered under it.
    let badge_h = 18.0;
    let badge_gap = 4.0;
    let side_pad = 6.0;
    let radius = ((rect.width() - side_pad * 2.0) * 0.5 - 1.0)
        .min((rect.height() - badge_h * 2.0 - badge_gap - 28.0) * 0.42)
        .clamp(36.0, 70.0);
    let cx = rect.center().x;
    let dial_center = Pos2::new(cx, rect.top() + 12.0 + radius);

    // No green rim — dark face only on the dark-green field.
    painter.circle_filled(dial_center, radius + 1.0, Color32::from_rgb(20, 40, 20));
    painter.circle_stroke(
        dial_center,
        radius,
        Stroke::new(1.5, Color32::from_rgb(200, 200, 200)),
    );

    for i in 0..12 {
        let a = (i as f32) / 12.0 * TAU - TAU / 4.0;
        let outer = dial_center + Vec2::angled(a) * (radius - 2.0);
        let inner = dial_center + Vec2::angled(a) * (radius - 10.0);
        painter.line_segment([inner, outer], Stroke::new(2.0, Color32::WHITE));
    }

    let h = now.hour() as f32 % 12.0 + now.minute() as f32 / 60.0;
    let m = now.minute() as f32 + now.second() as f32 / 60.0;
    let s = now.second() as f32 + now.nanosecond() as f32 / 1e9;

    let hand = |frac: f32, len: f32, stroke: Stroke| {
        let a = frac * TAU - TAU / 4.0;
        let tip = dial_center + Vec2::angled(a) * len;
        painter.line_segment([dial_center, tip], stroke);
    };
    hand(h / 12.0, radius * 0.55, Stroke::new(3.0, Color32::WHITE));
    hand(m / 60.0, radius * 0.75, Stroke::new(2.0, Color32::WHITE));
    hand(
        s / 60.0,
        radius * 0.85,
        Stroke::new(1.5, Color32::from_rgb(220, 40, 40)),
    );
    painter.circle_filled(dial_center, 3.0, Color32::from_rgb(220, 40, 40));

    let date = now.format("%d/%m/%y").to_string();
    let date_pos = Pos2::new(
        cx,
        dial_center.y + radius + 8.0 + badge_h * 0.5,
    );
    let date_rect = Rect::from_center_size(date_pos, Vec2::new(86.0, badge_h));
    // Date / version badges: dark grey raised buttons (unpressed bevel).
    painter.rect_filled(date_rect, 0.0, VGA_DARK_GRAY);
    paint_bevel(&painter, date_rect, false);
    painter.text(
        date_pos,
        egui::Align2::CENTER_CENTER,
        date,
        FontId::monospace(13.0),
        Color32::WHITE,
    );

    let ver_pos = Pos2::new(cx, date_pos.y + badge_h * 0.5 + badge_gap + badge_h * 0.5);
    let ver_rect = Rect::from_center_size(ver_pos, Vec2::new(92.0, badge_h));
    painter.rect_filled(ver_rect, 0.0, VGA_DARK_GRAY);
    paint_bevel(&painter, ver_rect, false);
    painter.text(
        ver_pos,
        egui::Align2::CENTER_CENTER,
        version::version_badge(),
        FontId::monospace(12.0),
        Color32::WHITE,
    );

    // Classic “Double-Note” hourly Gong toggle (ALT-G / mouse on clock).
    let btn = Rect::from_min_size(
        Pos2::new(rect.right() - 30.0, rect.top() + 4.0),
        Vec2::new(26.0, 20.0),
    );
    let resp = ui.allocate_rect(btn, Sense::click());
    let fill = if gong_on {
        VGA_GREEN
    } else {
        Color32::from_rgb(60, 60, 60)
    };
    let painter = ui.painter();
    painter.rect_filled(btn, 0.0, fill);
    // Gong enabled → sunken (same as classic ALT-G toggle).
    paint_bevel(painter, btn, gong_on || resp.is_pointer_button_down_on());
    painter.text(
        btn.center(),
        egui::Align2::CENTER_CENTER,
        "♫",
        FontId::proportional(14.0),
        if gong_on {
            Color32::BLACK
        } else {
            Color32::from_rgb(180, 180, 180)
        },
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // 1px white surround — inset so painter clip does not eat the stroke.
    // Drawn last (and with the unclipped painter) so it sits on top of the fill.
    ui.painter()
        .rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0, Color32::WHITE));

    resp.clicked()
}

fn load_window_icon() -> egui::IconData {
    const BYTES: &[u8] = include_bytes!("../../assets/GPNG.ico");
    // Asset is often a PNG saved as `.ico` — pick format from magic bytes.
    let img = if BYTES.starts_with(&[0x89, b'P', b'N', b'G']) {
        image::load_from_memory_with_format(BYTES, image::ImageFormat::Png)
    } else {
        image::load_from_memory(BYTES)
    };
    match img {
        Ok(img) => {
            let rgba = img.into_rgba8();
            egui::IconData {
                width: rgba.width(),
                height: rgba.height(),
                rgba: rgba.into_raw(),
            }
        }
        Err(_) => egui::IconData::default(),
    }
}

pub fn run(
    cfg: Config,
    config_path: PathBuf,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
) -> anyhow::Result<()> {
    let title = version::window_title();
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1024.0, 820.0])
        .with_min_inner_size([860.0, 700.0])
        .with_title(title.clone())
        .with_icon(load_window_icon());

    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let cfg = Arc::new(cfg);
    let config_path = Arc::new(config_path);
    eframe::run_native(
        &title,
        native,
        Box::new(move |cc| {
            let app = GpGuiApp::new(
                cc,
                (*cfg).clone(),
                (*config_path).clone(),
                events,
                cmd_tx,
            );
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;
    Ok(())
}
