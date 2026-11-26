//! Interactive terminal shell recreating the classic GP pane layout.
//!
//! Keyboard + mouse (crossterm). Channel tabs, BBS/List buttons, and the
//! bottom icon strip are clickable hit targets.

use std::collections::VecDeque;
use std::io::{stdout, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::Config;
use crate::remote::commands::{parse_line, RemoteAction};
use crate::tnc::{
    ax25, pack_text_payloads, ChannelConfig, HostCode, SessionCommand, SessionEvent, TncCommand,
    TncEvent,
};

use super::beacon::{self, BeaconSnap};
use super::channel_bits::{ChannelBits, SLOT_MONITOR};
use super::icons::{IconLibrary, TOOLBAR_LABELS};
use super::layout::{compute, toolbar_caption, ActivePane, PaneRects, SplitState};
use super::theme::Theme;

const MAX_LINES: usize = 2_000;
/// Fixed cell width for ` N: · · · · · · ` tabs (fits channel 10).
const CHANNEL_TAB_WIDTH: u16 = 16;

#[derive(Debug, Clone, Copy)]
enum HitAction {
    Channel(u8),
    Monitor,
    Check,
    List,
    Toolbar(usize),
}

#[derive(Debug, Clone, Copy)]
struct HitRegion {
    area: Rect,
    action: HitAction,
}

#[derive(Debug, Default)]
struct TextPad {
    lines: VecDeque<String>,
}

impl TextPad {
    fn push(&mut self, line: impl Into<String>) {
        self.lines.push_back(line.into());
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }
}

pub struct UiApp {
    cfg: Config,
    theme: Theme,
    active: ActivePane,
    split: SplitState,
    scrollbar: bool,
    status: String,
    input: String,
    pads: Vec<TextPad>,
    channels: u8,
    should_quit: bool,
    hits: Vec<HitRegion>,
    #[allow(dead_code)]
    icons: Option<IconLibrary>,
}

impl UiApp {
    pub fn new(cfg: Config) -> Self {
        let channels = cfg.session.channels.clamp(1, 10) as u8;
        let mut pads = Vec::with_capacity(channels as usize + 1);
        for _ in 0..=channels {
            pads.push(TextPad::default());
        }
        let theme = Theme::from_config(&cfg.colors);

        let icons = {
            let path = crate::paths::data_file("ICONS.GP");
            IconLibrary::load(&path).ok()
        };

        let icon_note = icons
            .as_ref()
            .map(|lib| format!("ICONS.GP×{}", lib.icons.len()))
            .unwrap_or_else(|| "ICONS.GP missing".into());

        let status = format!(
            "GP modern · {} · {} @ {} · {}",
            cfg.primary_mycall(),
            cfg.serial.port,
            cfg.serial.baud,
            icon_note
        );
        pads[0].push(format!(
            "*** Graphic Packet (modern) — MYCALL {} — click channels / toolbar; ALT-X quit",
            cfg.primary_mycall()
        ));

        Self {
            cfg,
            theme,
            active: ActivePane::Monitor,
            split: SplitState::default(),
            scrollbar: true,
            status,
            input: String::new(),
            pads,
            channels,
            should_quit: false,
            hits: Vec::new(),
            icons,
        }
    }

    fn pad_mut(&mut self, pane: ActivePane) -> Option<&mut TextPad> {
        let idx = match pane {
            ActivePane::Monitor => 0usize,
            ActivePane::Channel(n) if (1..=self.channels).contains(&n) => n as usize,
            _ => return None,
        };
        self.pads.get_mut(idx)
    }

    fn append_active(&mut self, line: impl Into<String>) {
        let pane = self.active;
        if let Some(pad) = self.pad_mut(pane) {
            pad.push(line);
        }
    }

    fn select_pane(&mut self, pane: ActivePane) {
        self.active = pane;
        self.status = format!("active: {}", pane.label());
    }

    pub fn handle_tnc_event(&mut self, ev: TncEvent) {
        match ev {
            TncEvent::Session(SessionEvent::Data { channel, data, .. }) => {
                if let Some(pad) = self.pads.get_mut(usize::from(channel)) {
                    pad.push(String::from_utf8_lossy(&data).into_owned());
                }
            }
            TncEvent::Session(event) => self.status = format!("{event:?}"),
            TncEvent::Status(msg) => self.status = msg,
            TncEvent::TimingBanner(banner) => {
                self.status = "timing parameters".into();
                for line in banner.split('\r').filter(|l| !l.is_empty()) {
                    self.pads[0].push(format!("*** {line}"));
                }
            }
            TncEvent::Warning(msg) => {
                self.status = format!("! {msg}");
                self.pads[0].push(format!("*** {msg}"));
            }
            TncEvent::LinkDown(msg) => {
                self.status = format!("link down: {msg}");
                self.pads[0].push(format!("*** link down: {msg}"));
            }
            TncEvent::RawRx(bytes) => {
                tracing::debug!(len = bytes.len(), "raw RX (ui)");
            }
            TncEvent::LinkIo { .. } => {}
            TncEvent::Modem { .. } => {}
            TncEvent::Buffers(_) => {}
            TncEvent::KissTx { port, frame } => {
                self.monitor_kiss_packet("TX", port, &frame);
            }
            TncEvent::HostFrame {
                channel,
                code,
                payload,
            } => {
                if self.cfg.tnc.protocol_kind().uses_software_ax25()
                    && matches!(code, HostCode::Data)
                {
                    self.monitor_kiss_packet("RX", channel, &payload);
                    return;
                }
                let text = String::from_utf8_lossy(&payload).to_string();
                let line = match code {
                    HostCode::Monitor | HostCode::MonitorHeader | HostCode::MonitorData => {
                        format!("[MON] {text}")
                    }
                    HostCode::CommandReply => format!("[CMD] {text}"),
                    HostCode::Error => format!("[ERROR] {text}"),
                    HostCode::Data => format!("[DATA] {text}"),
                    HostCode::LinkStatus => format!("[LINK] {text}"),
                    HostCode::Ok => return,
                    HostCode::Other(c) => format!("[{c:#04x}] {text}"),
                };
                let target = if channel == 0 {
                    ActivePane::Monitor
                } else if channel <= self.channels {
                    ActivePane::Channel(channel)
                } else {
                    ActivePane::Monitor
                };
                if let Some(pad) = self.pad_mut(target) {
                    pad.push(line);
                }
                if target != self.active {
                    self.status = format!("traffic on {}", target.label());
                }
            }
        }
    }

    fn monitor_kiss_packet(&mut self, direction: &str, port: u8, raw: &[u8]) {
        let Some(frame) = ax25::parse_frame(raw) else {
            self.pads[0].push(format!(
                "[{direction}] {}",
                super::session::kiss_monitor_fallback(port, raw)
            ));
            return;
        };
        self.pads[0].push(format!(
            "[{direction}] {}",
            ax25::tag_kiss_port(&frame.monitor_line(), port)
        ));
        let body = ax25::bytes_to_monitor_body(
            &frame.info,
            ChannelBits::new(self.cfg.session.chrome.umlaut).get(SLOT_MONITOR),
        );
        if !body.is_empty() {
            self.pads[0].push(format!("   {body}"));
        }
    }

    // To DO: Alt + P for Spy Menu (SLot 1)
    //        Alt + K for Kill Spy
    fn on_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        cmd_tx: &mpsc::Sender<TncCommand>,
    ) {
        if modifiers.contains(KeyModifiers::ALT) {
            match code {
                KeyCode::Char('x') | KeyCode::Char('X') => self.should_quit = true,
                KeyCode::Char('m') | KeyCode::Char('M') => self.select_pane(ActivePane::Monitor),
                KeyCode::Char('e') | KeyCode::Char('E') => self.select_pane(ActivePane::Editor),
                KeyCode::Char('s') | KeyCode::Char('S') => self.save_active(),
                KeyCode::Char('b') | KeyCode::Char('B') => self.scrollbar = !self.scrollbar,
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    let qrg = if self.cfg.session.qrg_text.trim().is_empty() {
                        super::session::SessionChrome::default().qrg_text()
                    } else {
                        self.cfg.session.qrg_text.clone()
                    };
                    self.status = beacon::send_now(&BeaconSnap {
                        kiss: self.cfg.tnc.runs_kiss_ax25(),
                        wa8ded: self.cfg.tnc.has_wa8ded(),
                        ports: self.cfg.tnc.kiss_beacon_ports(),
                        beacon: self.cfg.beacon.clone(),
                        my_call: self.cfg.primary_mycall().to_string(),
                        qrg,
                        version: crate::version::APP_VERSION.into(),
                        cmd_tx: cmd_tx.clone(),
                        mail_dir: crate::remote::mail::mail_dir(),
                    });
                }
                KeyCode::F(8) => self.split.enabled = !self.split.enabled,
                KeyCode::F(9) => self.split.nudge(-0.05),
                KeyCode::F(10) => self.split.nudge(0.05),
                KeyCode::F(1) => self.select_pane(ActivePane::Check),
                KeyCode::F(2) => self.select_pane(ActivePane::List),
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.status = "Setup pane not yet implemented".into();
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::F(n) if (1..=10).contains(&n) && n as u8 <= self.channels => {
                self.select_pane(ActivePane::Channel(n as u8));
            }
            KeyCode::Esc => {
                self.status = "TNC command entry (ESC) — placeholder".into();
            }
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => self.submit_input(cmd_tx),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    fn submit_input(&mut self, cmd_tx: &mpsc::Sender<TncCommand>) {
        let line = std::mem::take(&mut self.input);
        if line.is_empty() {
            return;
        }
        self.append_active(format!("> {line}"));

        if let Some((action, args)) = parse_line(&line) {
            self.status = format!("remote {:?}: {args}", action);
            if action == RemoteAction::Version {
                self.append_active(format!("*** Graphic Packet (modern) / compat GP 1.63"));
            }
            return;
        }

        let channel = match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        };
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        for chunk in pack_text_payloads(&line, max) {
            let command = if channel != 0 {
                TncCommand::Session(SessionCommand::Send {
                    channel,
                    data: chunk.into_bytes(),
                    id: None,
                })
            } else if self.cfg.tnc.runs_kiss_ax25() {
                let (remote, via) = crate::tnc::ax25::parse_unproto(&self.cfg.beacon.address);
                TncCommand::Session(SessionCommand::Unproto {
                    port: self.cfg.tnc.kiss_tx_port(),
                    local: self.cfg.primary_mycall().into(),
                    remote,
                    via,
                    data: chunk.into_bytes(),
                })
            } else {
                TncCommand::HostFrame {
                    channel,
                    data: chunk.into_bytes(),
                }
            };
            if let Err(err) = cmd_tx.try_send(command) {
                self.status = format!("radio command not queued: {err}");
                break;
            }
        }
    }

    fn save_active(&mut self) {
        self.status = "Save — placeholder".into();
    }

    fn on_mouse(&mut self, col: u16, row: u16) {
        if let Some(hit) = self
            .hits
            .iter()
            .copied()
            .find(|h| rect_contains(h.area, col, row))
        {
            match hit.action {
                HitAction::Channel(n) => self.select_pane(ActivePane::Channel(n)),
                HitAction::Monitor => self.select_pane(ActivePane::Monitor),
                HitAction::Check => self.select_pane(ActivePane::Check),
                HitAction::List => self.select_pane(ActivePane::List),
                HitAction::Toolbar(i) => {
                    let label = TOOLBAR_LABELS.get(i).copied().unwrap_or("?");
                    self.status = format!("icon: {label}");
                    match label {
                        "MONITOR" => self.select_pane(ActivePane::Monitor),
                        "EDIT" => self.select_pane(ActivePane::Editor),
                        "MHEARD" => self.select_pane(ActivePane::Mh),
                        "EXIT" => self.should_quit = true,
                        "SETUP" => self.status = "Setup — not yet implemented".into(),
                        "DOS" => self.status = "DOS shell — not in modern port".into(),
                        "SAVE" => self.save_active(),
                        "MYCALL" => {
                            self.status = format!("MYCALL {}", self.cfg.primary_mycall());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        self.hits.clear();
        let area = frame.area();
        let rects = compute(area, self.split);
        let theme = self.theme.clone();

        self.draw_channel_grid(frame, &rects, &theme);
        self.draw_clock(frame, rects.clock, &theme);

        let title = format!(
            " {}  │  {}  │  Global Mycall: {} ",
            self.active.label(),
            self.status,
            self.cfg.primary_mycall()
        );
        frame.render_widget(
            Paragraph::new(title).style(
                Style::default()
                    .fg(theme.state_fg)
                    .bg(theme.state_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            rects.title,
        );
        // Clicking the title bar returns to Monitor (classic feel).
        self.hits.push(HitRegion {
            area: rects.title,
            action: HitAction::Monitor,
        });

        self.draw_text_pane(frame, rects.primary, self.active, false);
        if let Some(sec) = rects.secondary {
            self.draw_text_pane(frame, sec, ActivePane::Monitor, true);
        }

        frame.render_widget(
            Paragraph::new(" QRG: --.--- │ CT: CTEXT │ Echo │ P │ ScrLock │ B ")
                .style(Style::default().fg(Color::Black).bg(Color::Gray)),
            rects.session,
        );

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .title(" Input ");
        frame.render_widget(
            Paragraph::new(format!("{}█", self.input))
                .style(Style::default().fg(theme.input_fg).bg(theme.input_bg))
                .block(input_block),
            rects.input,
        );

        self.draw_toolbar(frame, rects.icons);
    }

    fn draw_channel_grid(&mut self, frame: &mut ratatui::Frame<'_>, rects: &PaneRects, t: &Theme) {
        let grid_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(rects.channel_grid);

        let mut row1 = Vec::<Span<'_>>::new();
        let mut row2 = Vec::<Span<'_>>::new();
        let mut x1 = grid_rows[0].x;
        let mut x2 = grid_rows[1].x;

        for n in 1..=self.channels.min(6) {
            row1.push(channel_tab_span(n, self.active, t));
            self.hits.push(HitRegion {
                area: Rect {
                    x: x1,
                    y: grid_rows[0].y,
                    width: CHANNEL_TAB_WIDTH,
                    height: 1,
                },
                action: HitAction::Channel(n),
            });
            x1 = x1.saturating_add(CHANNEL_TAB_WIDTH);
        }

        for n in 7..=self.channels.min(10) {
            row2.push(channel_tab_span(n, self.active, t));
            self.hits.push(HitRegion {
                area: Rect {
                    x: x2,
                    y: grid_rows[1].y,
                    width: CHANNEL_TAB_WIDTH,
                    height: 1,
                },
                action: HitAction::Channel(n),
            });
            x2 = x2.saturating_add(CHANNEL_TAB_WIDTH);
        }

        const BBS: &str = " BBS-Menu ";
        const BOX: &str = " Box-List ";
        row2.push(Span::styled(
            BBS,
            Style::default().fg(t.menu_fg).bg(t.menu_bg),
        ));
        self.hits.push(HitRegion {
            area: Rect {
                x: x2,
                y: grid_rows[1].y,
                width: BBS.len() as u16,
                height: 1,
            },
            action: HitAction::Check,
        });
        x2 = x2.saturating_add(BBS.len() as u16);
        row2.push(Span::styled(
            BOX,
            Style::default().fg(t.menu_fg).bg(t.menu_bg),
        ));
        self.hits.push(HitRegion {
            area: Rect {
                x: x2,
                y: grid_rows[1].y,
                width: BOX.len() as u16,
                height: 1,
            },
            action: HitAction::List,
        });

        frame.render_widget(
            Paragraph::new(Line::from(row1)).style(Style::default().bg(Color::Blue)),
            grid_rows[0],
        );
        frame.render_widget(
            Paragraph::new(Line::from(row2)).style(Style::default().bg(Color::Blue)),
            grid_rows[1],
        );
    }

    fn draw_clock(&self, frame: &mut ratatui::Frame<'_>, area: Rect, t: &Theme) {
        let clock = chrono_lite();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "  (o)  ",
                    Style::default()
                        .fg(Color::White)
                        .bg(t.clock_bg)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!(" {clock} "),
                    Style::default().fg(Color::Black).bg(Color::Green),
                )),
            ]),
            area,
        );
    }

    fn draw_toolbar(&mut self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let mut spans = Vec::new();
        let mut x = area.x;
        for (i, label) in TOOLBAR_LABELS.iter().enumerate() {
            let cell = format!("[{label}]");
            let width = cell.len() as u16;
            spans.push(Span::styled(
                cell,
                Style::default().fg(Color::Black).bg(Color::Gray),
            ));
            spans.push(Span::raw(" "));
            self.hits.push(HitRegion {
                area: Rect {
                    x,
                    y: area.y,
                    width,
                    height: area.height.max(1),
                },
                action: HitAction::Toolbar(i),
            });
            x = x.saturating_add(width + 1);
        }
        // Fallback caption if terminal is tiny.
        if area.width < 20 {
            frame.render_widget(
                Paragraph::new(toolbar_caption())
                    .style(Style::default().fg(Color::Black).bg(Color::Gray)),
                area,
            );
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(spans))
                    .style(Style::default().fg(Color::Black).bg(Color::Gray)),
                area,
            );
        }
    }

    fn draw_text_pane(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        pane: ActivePane,
        is_secondary: bool,
    ) {
        let t = &self.theme;
        let (fg, bg) = match pane {
            ActivePane::Monitor => (t.monitor_fg, t.monitor_bg),
            ActivePane::Channel(_) => (t.connect_fg, t.connect_bg),
            ActivePane::Editor => (t.editor_fg, t.editor_bg),
            ActivePane::Check => (t.check_fg, t.check_bg),
            ActivePane::List => (t.list_fg, t.list_bg),
            ActivePane::Mh | ActivePane::Map => (t.mh_fg, t.mh_bg),
            ActivePane::Spy(_) => (t.spy_fg, t.spy_bg),
        };
        let idx = match pane {
            ActivePane::Monitor => 0,
            ActivePane::Channel(n) => n as usize,
            _ => 0,
        };
        let text = self
            .pads
            .get(idx)
            .map(|p| p.lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        let title = match pane {
            ActivePane::Monitor if is_secondary => " Monitor (split) ".to_string(),
            ActivePane::Monitor => " Monitor ".to_string(),
            ActivePane::Channel(n) => format!(" Channel {n} "),
            ActivePane::Editor => " Editor ".into(),
            ActivePane::Check => " Check ".into(),
            ActivePane::List => " List ".into(),
            ActivePane::Mh => " MH ".into(),
            ActivePane::Map => " Map ".into(),
            ActivePane::Spy(n) => format!(" Spy {n} "),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border))
            .title(title);
        let block = if self.scrollbar && !is_secondary {
            block.title_bottom(Line::from(" [B] scroll "))
        } else {
            block
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(fg).bg(bg))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

fn channel_tab_span(n: u8, active: ActivePane, t: &Theme) -> Span<'static> {
    let selected = active == ActivePane::Channel(n);
    let style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(t.menu_alert)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Black).bg(Color::Gray)
    };
    // Pad to CHANNEL_TAB_WIDTH for stable mouse hit boxes.
    let mut label = format!(" {n}: · · · · · · ");
    while label.chars().count() < CHANNEL_TAB_WIDTH as usize {
        label.push(' ');
    }
    label.truncate(CHANNEL_TAB_WIDTH as usize);
    Span::styled(label, style)
}

fn chrono_lite() -> String {
    use std::time::SystemTime;
    let ok = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() % 86_400)
        .unwrap_or(0);
    let h = ok / 3600;
    let m = (ok % 3600) / 60;
    let s = ok % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Run the TUI until quit (blocking; call from `spawn_blocking`).
pub fn run(
    cfg: Config,
    mut events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
) -> Result<()> {
    let mut app = UiApp::new(cfg);
    cmd_tx
        .blocking_send(TncCommand::Session(SessionCommand::Configure(
            (1..=app.channels)
                .map(|channel| ChannelConfig {
                    channel,
                    port: app.cfg.tnc.channel_port(channel),
                    local: app.cfg.mycall_for_slot(channel).into(),
                })
                .collect(),
        )))
        .map_err(|err| anyhow::anyhow!("radio session configuration: {err}"))?;

    enable_raw_mode()?;
    let mut out: Stdout = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    info!("TUI online — click channels/toolbar; ALT-X or Ctrl-C to quit");

    let result = loop {
        terminal.draw(|f| app.draw(f))?;

        while let Ok(ev) = events.try_recv() {
            app.handle_tnc_event(ev);
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.should_quit = true;
                    } else {
                        app.on_key(key.code, key.modifiers, &cmd_tx);
                    }
                }
                Event::Mouse(mouse) => {
                    // Down + drag-release both useful; ignore scroll for now.
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Down(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                    ) {
                        // Only act on press to avoid double-fire with Up.
                        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                            app.on_mouse(mouse.column, mouse.row);
                        }
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if app.should_quit {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiss_rx_and_tx_use_monitor_not_logical_channel_pads() {
        let mut cfg = Config::default();
        cfg.tnc.protocol = "kiss".into();
        let mut app = UiApp::new(cfg);
        app.pads[0].lines.clear();
        let frames = [
            ax25::build_ui("CQ", "N1TEST", &[], b"hello\x00\xDB\xC0"),
            ax25::build_rr("N0ME", "N1TEST", 3, true, false),
            vec![0x01, 0xDB, 0xC0],
        ];
        for port in [0, 3, 12] {
            for frame in &frames {
                app.handle_tnc_event(TncEvent::HostFrame {
                    channel: port,
                    code: HostCode::Data,
                    payload: frame.clone(),
                });
                app.handle_tnc_event(TncEvent::KissTx {
                    port,
                    frame: frame.clone(),
                });
            }
        }
        for direction in ["RX", "TX"] {
            assert_eq!(
                app.pads[0]
                    .lines
                    .iter()
                    .filter(|line| line.starts_with(&format!("[{direction}]")))
                    .count(),
                9
            );
        }
        assert!(app.pads[0]
            .lines
            .iter()
            .any(|line| line.contains("fm12 N1TEST")));
        assert!(app.pads[0]
            .lines
            .iter()
            .any(|line| line.contains("01 DB C0")));
        assert!(app.pads.iter().skip(1).all(|pad| pad.lines.is_empty()));
    }

    #[test]
    fn alt_a_sends_beacon_and_alt_b_toggles_scrollbar() {
        let mut cfg = Config::default();
        cfg.tnc.protocol = "kiss".into();
        cfg.beacon.text = "Test beacon".into();
        let mut app = UiApp::new(cfg);
        let (tx, mut rx) = mpsc::channel(2);
        app.on_key(KeyCode::Char('a'), KeyModifiers::ALT, &tx);
        let TncCommand::Session(SessionCommand::UnprotoBroadcast { ports, data, .. }) =
            rx.try_recv().unwrap()
        else {
            panic!("expected beacon broadcast");
        };
        assert_eq!(ports, [0]);
        assert_eq!(data, b"Test beacon");
        assert!(app.scrollbar);
        app.on_key(KeyCode::Char('b'), KeyModifiers::ALT, &tx);
        assert!(!app.scrollbar);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn alt_s_matches_save_toolbar_action() {
        let mut app = UiApp::new(Config::default());
        let (tx, _rx) = mpsc::channel(1);
        let save_index = TOOLBAR_LABELS
            .iter()
            .position(|label| *label == "SAVE")
            .unwrap();
        app.hits.push(HitRegion {
            area: Rect::new(0, 0, 1, 1),
            action: HitAction::Toolbar(save_index),
        });
        for pane in [ActivePane::Monitor, ActivePane::Editor, ActivePane::Check] {
            app.active = pane;
            app.on_mouse(0, 0);
            let icon_status = app.status.clone();
            app.status.clear();
            app.on_key(KeyCode::Char('s'), KeyModifiers::ALT, &tx);
            assert_eq!(app.status, icon_status);
        }
    }
}
