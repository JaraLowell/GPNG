//! Pixel UI (egui) — `ICONS.GP` bitmaps + analog clock, classic GP chrome.
//!
//! Layout matched to V1.63 screenshots + operator notes:
//! ```text
//! ┌── purple pre-write (history + 3-line compose) ──┬── analog clock ──┐
//! ├── channel grid (2×6) ───────────────────────────┤   (tall: spans   │
//! ├── magenta status grid (2 rows, non-clickable) ──┤    all three)    │
//! ├─────────────────────────────────────────────────┴──────────────────┤
//! │ full-width content                                                  │
//! │ session strip: QRG CT äöü / Echo ♪ [spy|7+/Remote/BIN] ScrLock ▲▼ B │
//! │ icons evenly spaced (set depends on pane)                           │
//! ```
//!
//! Magenta is **not** one text line: left title cell is 2 rows high; right
//! side has Mode/Buffers on row 1 and Packets/Mycall on row 2 (Monitor).

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Local, Timelike};
use eframe::egui::{
    self, Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId, Frame, Pos2,
    Rect, RichText, Sense, Stroke, TextWrapMode, TextureHandle, TextureOptions, Ui, Vec2,
};
use eframe::egui::text::{LayoutJob, TextFormat};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::remote::commands::parse_line;
use crate::tnc::{HostCode, TncCommand, TncEvent};
use crate::version;

use super::icons::{
    toolbar_for, IconLibrary, ICON_HEIGHT, ICON_SLOT_LABELS, ICON_WIDTH,
};
use super::layout::ActivePane;
use super::session::{
    decode_rx_text, default_bell_wav, default_ctext_path, format_ax25_ui, format_hex_preview,
    play_bell_wav, SessionChrome,
};
use super::theme::vga;

const MAX_LINES: usize = 2_000;
const TYPED_HISTORY: usize = 10;
const COMPOSE_ROWS: usize = 3;

/// Classic VGA chrome (from GP V1.63 / DOSBox reference).
const VGA_BLUE: Color32 = Color32::from_rgb(0, 0, 170);
const VGA_DARK_GREEN: Color32 = Color32::from_rgb(0, 85, 0);
const VGA_GREEN: Color32 = Color32::from_rgb(0, 170, 0);
const VGA_DARK_RED: Color32 = Color32::from_rgb(170, 0, 0);
const VGA_GRAY: Color32 = Color32::from_rgb(170, 170, 170);
const VGA_DARK_GRAY: Color32 = Color32::from_rgb(85, 85, 85);
/// Classic GP text-window colours (Monitor / Channel / Spy pads).
const PAD_INCOMING: Color32 = Color32::from_rgb(85, 255, 85); // light green
const PAD_OWN: Color32 = Color32::WHITE;
const PAD_SYSTEM: Color32 = Color32::from_rgb(255, 168, 0); // orange (init / debug)

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

/// Classic GP line roles — colour carries meaning (no `***` needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PadKind {
    /// RX / decoded APRS / monitored frames.
    Incoming,
    /// Local TX echo (when Echo is on).
    Own,
    /// Init, status, warnings, raw UART debug.
    System,
}

impl PadKind {
    fn color(self) -> Color32 {
        match self {
            PadKind::Incoming => PAD_INCOMING,
            PadKind::Own => PAD_OWN,
            PadKind::System => PAD_SYSTEM,
        }
    }
}

struct PadLine {
    kind: PadKind,
    text: String,
}

struct TextPad {
    lines: VecDeque<PadLine>,
}

impl TextPad {
    fn push(&mut self, kind: PadKind, line: impl Into<String>) {
        self.lines.push_back(PadLine {
            kind,
            text: line.into(),
        });
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    fn layout_job(&self) -> LayoutJob {
        let mut job = LayoutJob::default();
        let font = FontId::monospace(14.0);
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                job.append(
                    "\n",
                    0.0,
                    TextFormat {
                        font_id: font.clone(),
                        color: line.kind.color(),
                        ..Default::default()
                    },
                );
            }
            job.append(
                &line.text,
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: line.kind.color(),
                    ..Default::default()
                },
            );
        }
        job
    }
}

pub struct GpGuiApp {
    cfg: Config,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
    active: ActivePane,
    status: String,
    /// Current compose buffer (shown in a 3-line editor).
    input: String,
    /// Last typed lines (pre-write history), newest at the end.
    typed_history: VecDeque<String>,
    pads: Vec<TextPad>,
    /// Soft event / command log (mode changes, etc.).
    event_log: VecDeque<String>,
    channels: u8,
    /// Peer callsign per channel index (`1..=channels`); `None` = disconnected.
    channel_peer: Vec<Option<String>>,
    icon_tex: Vec<TextureHandle>,
    /// WA8DED monitor flags, e.g. `IUSC` (from TNC / local set).
    monitor_mode: String,
    /// Scroll-back capacity shown on the magenta bar (classic “Buffers:”).
    buffer_lines: u32,
    /// Compact port id for the session-bar badge (`COM6` → shown as Com 6).
    serial_port: String,
    /// UART link up (drives `[ <-> Com n open ]` vs closed).
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
    bell_wav: PathBuf,
    /// Hourly gong (classic ALT-G / click ♫ on the clock).
    hour_gong: bool,
    /// Prevents double-fire within the same hour.
    last_gong: Option<(chrono::NaiveDate, u32)>,
}

impl GpGuiApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        events: mpsc::Receiver<TncEvent>,
        cmd_tx: mpsc::Sender<TncCommand>,
    ) -> Self {
        let channels = cfg.session.channels.clamp(1, 10) as u8;
        let mut pads = Vec::with_capacity(channels as usize + 1);
        let mut channel_peer = Vec::with_capacity(channels as usize + 1);
        for _ in 0..=channels {
            pads.push(TextPad {
                lines: VecDeque::new(),
            });
            channel_peer.push(None);
        }
        pads[0].push(
            PadKind::System,
            version::startup_banner(cfg.primary_mycall()),
        );

        let buffer_lines = cfg.buffers.text_buffer.first().copied().unwrap_or(500);
        install_console_font(&cc.egui_ctx);
        let icon_tex = load_icon_textures(&cc.egui_ctx);
        let status = if icon_tex.is_empty() {
            "tools/ICONS.GP not found".into()
        } else {
            format!("{} icons", icon_tex.len())
        };

        let colors = &cfg.colors;
        let serial_port = cfg.serial.port.clone();
        let monitor_mode = if cfg.tnc.monitor_mode.is_empty() {
            "IUSC".into()
        } else {
            cfg.tnc.monitor_mode.clone()
        };
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
            events,
            cmd_tx,
            active: ActivePane::Monitor,
            status,
            input: String::new(),
            typed_history: VecDeque::new(),
            pads,
            event_log: VecDeque::new(),
            channels,
            channel_peer,
            icon_tex,
            monitor_mode,
            buffer_lines,
            serial_port,
            serial_open: false,
            serial_rx_bytes: 0,
            serial_rx_frames: 0,
            session: SessionChrome {
                ctext_path: default_ctext_path(),
                ..SessionChrome::default()
            },
            bell_wav: default_bell_wav(),
            hour_gong: true,
            last_gong: None,
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
                    // Monitor pad: only the quiet “serial OPEN …” line (from Config port/baud).
                    if low.starts_with("serial open") {
                        self.serial_open = true;
                        self.log_event(m);
                    } else if low.contains("wa8ded driver") || low.contains("tfpcx driver") {
                        self.serial_open = true;
                    }
                }
                TncEvent::Warning(m) => {
                    self.status = m.clone();
                    // Keep real failures; skip routine bring-up noise.
                    if !m.to_ascii_lowercase().starts_with("kiss") {
                        self.log_event(format!("! {m}"));
                    }
                }
                TncEvent::LinkDown(m) => {
                    self.status = format!("link down: {m}");
                    self.serial_open = false;
                    for peer in &mut self.channel_peer {
                        *peer = None;
                    }
                }
                // UART counters only — full KISS frames land as Incoming (green).
                TncEvent::RawRx(bytes) => {
                    self.serial_rx_bytes += bytes.len() as u64;
                }
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
                    if matches!(code, HostCode::LinkStatus) {
                        self.apply_link_status(channel, &text);
                    }
                    let (kind, line) = match code {
                        HostCode::Ok => continue,
                        HostCode::Monitor => (PadKind::Incoming, text),
                        HostCode::LinkStatus => (PadKind::System, text),
                        HostCode::Data => {
                            if let Some(ax) = format_ax25_ui(&payload) {
                                (PadKind::Incoming, ax)
                            } else if text.chars().any(|c| c.is_ascii_graphic()) {
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
                    // KISS port 0 and hostmode ch0 → Monitor; else channel pad.
                    let idx = if channel == 0 {
                        0
                    } else if channel <= self.channels {
                        channel as usize
                    } else {
                        0
                    };
                    if let Some(pad) = self.pads.get_mut(idx) {
                        pad.push(kind, line);
                    }
                }
            }
        }
    }

    fn select(&mut self, pane: ActivePane) {
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
        if upper.contains("DISCONNECTED")
            || upper.contains("LINK FAILURE")
            || upper.contains("LINK FAIL")
            || upper.contains("BUSY FROM")
            || upper.contains("FAILURE WITH")
        {
            self.channel_peer[idx] = None;
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
            self.channel_peer[idx] = if call.is_empty() {
                Some("****".into())
            } else {
                Some(call)
            };
        } else if upper.contains("CONNECTED") {
            if self.channel_peer[idx].is_none() {
                self.channel_peer[idx] = Some("****".into());
            }
        }
    }

    fn channel_connected(&self, n: u8) -> bool {
        self.channel_peer
            .get(n as usize)
            .and_then(|p| p.as_ref())
            .is_some()
    }

    fn channel_label(&self, n: u8) -> String {
        match self.channel_peer.get(n as usize).and_then(|p| p.as_ref()) {
            Some(call) => format!("{n}: {call}"),
            None => format!("{n}: ......"),
        }
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

        let idx = match self.active {
            ActivePane::Channel(n) => n as usize,
            _ => 0,
        };
        // Own / echo: white. Channel pads always show TX; Monitor only when Echo is on.
        if idx != 0 {
            if let Some(pad) = self.pads.get_mut(idx) {
                pad.push(PadKind::Own, line.clone());
            }
            if self.session.echo {
                self.pads[0].push(PadKind::Own, line.clone());
            }
        } else if self.session.echo {
            self.pads[0].push(PadKind::Own, line.clone());
        }

        if let Some((action, args)) = parse_line(&line) {
            // Local `//` always runs. `Remote` gates *incoming* peer remotes (TNC RX path).
            self.status = format!("remote {action:?}: {args}");
            self.log_event(format!("remote {action:?} {args}"));
            return;
        }

        let channel = match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        };
        let _ = self.cmd_tx.try_send(TncCommand::HostFrame {
            channel,
            data: line.into_bytes(),
        });
    }

    fn active_layout(&self) -> LayoutJob {
        let idx = match self.active {
            ActivePane::Channel(n) => n as usize,
            _ => 0,
        };
        self.pads
            .get(idx)
            .map(TextPad::layout_job)
            .unwrap_or_default()
    }

    fn show_session_bar(&self) -> bool {
        matches!(self.active, ActivePane::Monitor | ActivePane::Channel(_))
    }

    fn pane_title(&self) -> String {
        match self.active {
            ActivePane::Monitor => "Monitor".into(),
            ActivePane::Channel(n) => format!("Channel {n}"),
            ActivePane::Mh => "MHeard".into(),
            ActivePane::Editor => "GP-Editor".into(),
            ActivePane::Check => "BBS-Menu".into(),
            ActivePane::List => "DieBox-List".into(),
            ActivePane::Spy => "Spy".into(),
        }
    }
}

impl eframe::App for GpGuiApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_tnc();
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

        let my = self.cfg.primary_mycall();
        let owned = self.status_field_rows(my);
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

    fn status_field_rows(&self, my: &str) -> (Vec<String>, Vec<String>) {
        match self.active {
            // Classic GP Monitor magenta grid (COM lives on the session strip).
            ActivePane::Monitor => (
                vec![
                    format!("Monitor Mode: {}", self.monitor_mode),
                    format!("Buffers: {}", self.buffer_lines),
                ],
                vec![
                    "Packets not yet displayed: 0".into(),
                    format!("Global Mycall: {my}"),
                ],
            ),
            ActivePane::Channel(n) => (
                vec![
                    format!("Ready for Connect"),
                    format!("My: {my}"),
                    format!("Buffers: {}", self.buffer_lines),
                ],
                vec![
                    format!("Ch {n}  Not disp: 0"),
                    "Not sent: 0".into(),
                    "Tries: 0".into(),
                    "Unack: 0".into(),
                ],
            ),
            ActivePane::Editor => (
                vec![
                    "NONAME.TXT".into(),
                    "Line: 1".into(),
                    "Col: 1".into(),
                ],
                vec!["Insert".into(), "Filled Lines: 1/780".into()],
            ),
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
            ActivePane::Mh => (vec!["stations heard".into()], vec![]),
            ActivePane::Spy => (vec!["Spy".into()], vec![]),
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
        let bg = match self.active {
            ActivePane::Channel(_) => self.connect_bg,
            ActivePane::Mh | ActivePane::Check | ActivePane::List => {
                Color32::from_rgb(170, 170, 170)
            }
            _ => self.monitor_bg,
        };
        let nudge = self.session.take_scroll_nudge();
        let stick = self.session.scr_lock;
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

        // Leading + middle fill the remaining left region.
        let mut x = rect.left() + 2.0;
        let left_limit = (rect.right() - trail_w - 6.0).max(x + 40.0);

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
        ];
        for (label, lit, action, w) in lead_owned {
            if x + w > left_limit {
                break;
            }
            x += paint_cell(ui, x, label, lit, action, w, CellGlyph::Text);
        }

        match self.active {
            ActivePane::Monitor => {
                for n in 1..=self.channels.min(10) {
                    let w = 20.0_f32;
                    if x + w > left_limit {
                        break;
                    }
                    let lit = self.session.spy_slot == Some(n);
                    x += paint_cell(
                        ui,
                        x,
                        &format!("{n}"),
                        lit,
                        SessionClick::Spy(n),
                        w,
                        CellGlyph::Text,
                    );
                }
                // Non-clickable COM badge (classic GP had no COM in the magenta grid).
                let com_badge = com_open_badge(&self.serial_port, self.serial_open);
                let badge_w = (com_badge.len() as f32 * 7.2 + 12.0).clamp(88.0, 160.0);
                if x + badge_w <= left_limit {
                    let _ = paint_static_cell(ui, x, y, h, gap, &com_badge, badge_w, self.serial_open);
                }
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
            }
            SessionClick::LineStep => {
                self.session.line_step = self.session.line_step.cycle();
                self.status = format!("line step: {:?}", self.session.line_step);
            }
            SessionClick::Echo => {
                self.session.echo = !self.session.echo;
                self.status = format!("Echo {}", on_off(self.session.echo));
            }
            SessionClick::Bell => {
                self.session.bell = !self.session.bell;
                self.status = format!("Bell {}", on_off(self.session.bell));
                if self.session.bell {
                    play_bell_wav(&self.bell_wav);
                }
            }
            SessionClick::Spy(n) => {
                if self.session.spy_slot == Some(n) {
                    self.session.spy_slot = None;
                    self.select(ActivePane::Monitor);
                } else {
                    self.session.spy_slot = Some(n);
                    self.select(ActivePane::Spy);
                    self.status = format!("Spy {n}");
                    self.log_event(format!("Spy window {n}"));
                }
            }
            SessionClick::SevenPlus => {
                self.session.seven_plus = !self.session.seven_plus;
                self.status = format!("7+ {}", on_off(self.session.seven_plus));
                self.log_event(format!("7Plus {}", on_off(self.session.seven_plus)));
            }
            SessionClick::Remote => {
                self.session.remote = !self.session.remote;
                self.status = format!("Remote {}", on_off(self.session.remote));
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
            .filter(|p| p.exists())
            .or_else(|| {
                let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
                root.canonicalize().ok()
            });
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

    fn icon_strip(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, VGA_DARK_RED);

        if self.icon_tex.is_empty() {
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "No icons — expected tools/ICONS.GP ({}×{})",
                            ICON_WIDTH, ICON_HEIGHT
                        ))
                        .color(Color32::YELLOW),
                    );
                });
            });
            return;
        }

        let toolbar = toolbar_for(self.active);
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
                    ("SPY", ActivePane::Spy) => true,
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
                "EDIT" => self.select(ActivePane::Editor),
                "MHEARD" => self.select(ActivePane::Mh),
                "SPY" => self.select(ActivePane::Spy),
                "EXIT" => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                "MYCALL" => {
                    self.status = format!("MYCALL {}", self.cfg.primary_mycall());
                }
                "CONNECT" => {
                    self.status = "CONNECT …".into();
                    self.log_event("CONNECT requested");
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

/// `COM6` / `com6` → `6`; otherwise keep the raw port token.
fn com_port_number(port: &str) -> String {
    let t = port.trim();
    let rest = t
        .strip_prefix("COM")
        .or_else(|| t.strip_prefix("com"))
        .unwrap_or(t);
    rest.trim().to_string()
}

fn com_open_badge(port: &str, open: bool) -> String {
    let n = com_port_number(port);
    // ASCII `<->` — session-bar face has no ⇄ glyph (shows as tofu □).
    if open {
        format!("[ <-> Com {n} open ]")
    } else {
        format!("[ <-> Com {n} ]")
    }
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
    let path = [
        PathBuf::from("tools/ICONS.GP"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/ICONS.GP"),
    ]
    .into_iter()
    .find(|p| p.exists());

    let Some(path) = path else {
        return Vec::new();
    };
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

pub fn run(
    cfg: Config,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
) -> anyhow::Result<()> {
    let title = version::window_title();
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1024.0, 820.0])
        .with_min_inner_size([860.0, 700.0])
        .with_title(title.clone());

    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let cfg = Arc::new(cfg);
    eframe::run_native(
        &title,
        native,
        Box::new(move |cc| {
            let app = GpGuiApp::new(cc, (*cfg).clone(), events, cmd_tx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;
    Ok(())
}
