//! Pixel UI (egui) — `ICONS.GP` bitmaps + analog clock, classic GP chrome.
//!
//! Layout matched to V1.63 screenshots + operator notes:
//! ```text
//! ┌── purple pre-write (history + 3-line compose) ──┬── analog clock ──┐
//! ├── channel grid (2×6) ───────────────────────────┤   (tall: spans   │
//! ├── magenta status grid (2 rows, non-clickable) ──┤    all three)    │
//! ├─────────────────────────────────────────────────┴──────────────────┤
//! │ full-width content (QSO / monitor / editor / …)                     │
//! │ session strip: QRG CT äöü / Echo ♪ Clear [spy|7+/Remote/BIN] 🔑 🖥 ScrLock ▲▼ B │
//! │ channel splitscreen only: the same monitor pad, under that strip    │
//! │ icons evenly spaced (set depends on pane)                           │
//! ```
//!
//! Splitscreen (ALT-F8, or right-click the channel) is channel-only. The
//! upper pane stays the QSO. The lower pane is the monitor pad (`pads[0]`),
//! not a second buffer. ALT-F9 / ▲ move the bar up (more monitor); ALT-F10 / ▼
//! move it down. Drag anywhere on the grey session bar; a click on ▲▼ still steps.
//!
//! Magenta is **not** one text line: left title cell is 2 rows high; right
//! side has Mode/Buffers on row 1 and Packets/Mycall on row 2 (Monitor).
//! Buffers, Not disp, Not sent, Tries, and Unack come from the modem driver.

use std::collections::{HashMap, VecDeque};
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use chrono::{Datelike, Local, Timelike};
use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId, Frame, Pos2,
    Rect, RichText, Sense, Stroke, TextWrapMode, TextureHandle, TextureOptions, Ui, Vec2,
};
use tokio::sync::mpsc;

use crate::bbs::{self, Entry as MailEntry};
use crate::cipher;
use crate::config::{
    beacon_minutes, clip_beacon, normalize_mycall, BeaconConfig, Config, HostProtocol,
    BEACON_MINUTES_MAX, BEACON_TEXT_MAX,
};
use crate::gpw::{self, Reply as GpwReply};
use crate::journal::{self, OpenQso};
use crate::names::{qrg_key, ListedName, NameEntry, NameScope, NamesFile, Route, StationKind};
use crate::remote::commands::{
    parse_line, report, report_block, rtt_ask, rtt_from_stamps, rtt_phrase, slash_verb,
    split_cross_channel, typed_runs_locally, unix_millis, RemoteAction, RttAsk, RttRead,
    NOT_RECOGNIZED, REMOTE_OFF,
};
use crate::remote::external;
use crate::remote::gpi;
use crate::remote::macros::{self, MacroContext};
use crate::remote::mail::{self, MailAsk};
use crate::remote::userdir;
use crate::tnc::ax25::{self, Ax25Frame, Ax25FrameType};
use crate::tnc::{
    apply_configured_paclen, detect_serial_ports, kiss_param_lines, kiss_payload_len,
    load_kiss_paclen, load_kiss_params, normalize_call, pack_text_payloads, parse_heard,
    parse_kiss_esc, payload_lines, resolve_kiss_ini, resolve_wa8ded_ini, store_kiss_paclen,
    store_kiss_param, take_rx_lines_bytes, timing_banner_fallback, wa8ded_section_lines,
    BufferReport, ChannelBuffers, DetectedPort, EscKiss, FrameClass, HostCode, LinkPhase,
    MonitorFilter, MonitorInput, RadioLink, TncCommand, TncEvent, TNC_RAM,
};
use crate::tnc::{ChannelConfig, SessionCommand, SessionEvent};
use crate::transfer::autobin::{self, AutoBinState, BinReceiveDone, TxReply};
use crate::transfer::sevenplus::{self, SevenPlusAutosave};
use crate::version;

use super::ansi::{expand_sgr_arrows, strip_ansi, AnsiDecoder, AnsiStyle};
use super::beacon::{BeaconSchedule, BeaconSnap};
use super::channel_bits::{slot_for, SLOT_BBS, SLOT_LIST, SLOT_MONITOR};
use super::editor::{
    channel_store_text, decode_channel_bytes, decode_cp437, encode_channel_wire, packet_chunks,
    show_channel_text, GpEditor,
};
use super::icons::{
    icon_file_label, list_icon_files, resolve_icon_path, toolbar_for, toolbar_for_channel,
    IconEdit, IconLibrary, ICON_HEIGHT, ICON_SLOT_LABELS, ICON_WIDTH, VGA16_RGB,
};
use super::layout::ActivePane;
use super::map::{self, MapView};
use super::mheard::{self, MheardList};
use super::padlog::{self, PadKind, TextPad};
use super::retro;
use super::session::{
    chime_strikes, connect_bell_plays, decode_rx_text, default_bell_wav, default_chime_wav,
    default_connect_wav, default_disconnect_wav, default_newline_wav, disconnect_plays,
    format_hex_preview, parse_ax25_ui, play_bell_wav, play_wav_at, play_wav_times, rx_click_plays,
    Ax25UiFrame, SessionChrome,
};
use super::setup::{self, ColorSlot, EditKind, RowId, RowKind, View as SetupView};
use super::theme::vga;

const TYPED_HISTORY: usize = 10;
const COMPOSE_ROWS: usize = 3;
/// Pre-write font. Sized so three rows fill the compose strip with no orphan gap.
const COMPOSE_FONT_SIZE: f32 = 15.0;
/// After `//Q` queues QRT, wait at least this long for TX counters before
/// treating an idle buffer as "already sent" (avoids DISC wiping the queue).
const QRT_DISC_ARM: Duration = Duration::from_millis(400);

/// One pre-write buffer: Monitor is slot 0, channel *n* is slot *n*.
#[derive(Debug, Clone, Default)]
struct ComposeSlot {
    input: String,
    /// Last typed lines, newest at the end. Each entry is a single display line.
    history: VecDeque<String>,
    hist_cursor: Option<usize>,
    hist_stash: String,
    compose_caret_end: bool,
}
/// Hard stop if TX never drains after QRT (N2 / dead peer).
const QRT_DISC_TIMEOUT: Duration = Duration::from_secs(180);

/// `//Quit`: send QRT first; DISC only when the channel TX queue is empty.
#[derive(Debug, Clone, Copy)]
enum QrtDiscState {
    Idle,
    /// QRT handed to the modem; wait for a busy TX snapshot (or arm timeout).
    WaitReport {
        since: Instant,
    },
    /// Outstanding frames seen; disconnect when not_sent/unack return to 0.
    Drain {
        since: Instant,
    },
}

/// Classic VGA chrome (from GP V1.63 / DOSBox reference).
const VGA_BLUE: Color32 = Color32::from_rgb(0, 0, 170);
const VGA_DOS: Color32 = Color32::from_rgb(12, 0, 0);
const VGA_GREEN: Color32 = Color32::from_rgb(0, 170, 0);
const VGA_GRAY: Color32 = Color32::from_rgb(170, 170, 170);
const VGA_DARK_GRAY: Color32 = Color32::from_rgb(85, 85, 85);
const VGA_CYAN: Color32 = Color32::from_rgb(0, 170, 170);
/// CHANNEL1.png scroll column: 16px, same scale as the 1px bevels.
const DOS_SCROLL_W: f32 = 16.0;
/// Classic GP text-window colours (Monitor / Channel / Spy pads).

/// Thin 3D rim for the small session cells. Same colours as the menus:
/// white on the left and top, black on the bottom and right.
fn mix_rgb(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    Color32::from_rgb(
        ch(from.r(), to.r()),
        ch(from.g(), to.g()),
        ch(from.b(), to.b()),
    )
}

fn paint_bevel(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let hi_face = Color32::WHITE;
    let (hi, lo) = if pressed {
        (Color32::BLACK, hi_face)
    } else {
        (hi_face, Color32::BLACK)
    };
    paint_bevel_colors(painter, rect, hi, lo);
}

/// Classic DOS icon bevel: hard white / black.
/// Raised = white top+left, black bottom+right.
/// Sunken (active pane / mouse down) = reversed.
fn paint_icon_bevel(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let (hi, lo) = if pressed {
        (Color32::BLACK, Color32::WHITE)
    } else {
        (Color32::WHITE, Color32::BLACK)
    };
    paint_bevel_colors(painter, rect, hi, lo);
}

fn paint_bevel_colors(painter: &egui::Painter, rect: Rect, hi: Color32, lo: Color32) {
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

/// Which startup-card list is open. Choosing a line writes it back and closes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RadioPick {
    Protocol,
    Connection,
    Port,
    Baud,
}

/// One of the three TFPCX modems on the radio card.
#[derive(Clone)]
struct RadioLinkForm {
    /// `off`, `kiss`, or `wa8ded`.
    protocol: String,
    ports: Vec<u8>,
    tcp: bool,
    port_name: String,
    baud: u32,
    host: String,
    tcp_port: String,
}

/// Text captured by `//Write` until `//END` or Ctrl-Z.
#[derive(Clone)]
struct UserWrite {
    name: String,
    body: String,
}

/// `//SEND <call>` is waiting for the next line, which is the mail.

pub struct GpGuiApp {
    cfg: Config,
    /// Path used to persist MYCALL (and later settings) into Config.toml.
    config_path: PathBuf,
    events: mpsc::Receiver<TncEvent>,
    cmd_tx: mpsc::Sender<TncCommand>,
    active: ActivePane,
    /// Channel splitscreen. Off on Monitor, Spy, BBS-Menu, and Box-List.
    split_on: bool,
    /// Fraction of the QSO+monitor body given to the monitor (0.15..=0.75).
    split_lower: f32,
    /// Last QSO+monitor height, so a drag can turn pixels into `split_lower`.
    split_body_h: f32,
    /// Session-bar press is still held. The bar moves with the pointer, so the
    /// gesture stays latched until release instead of re-testing the hit rect.
    split_dragging: bool,
    /// Pointer travel on the session bar this gesture. Under the slop, ▲▼ click.
    split_drag_dy: f32,
    status: String,
    /// Last TNC link report (ALT-F6).
    last_link_status: String,
    /// Last modem warning (ALT-F7).
    last_error: String,
    /// Ctrl-Y / Ctrl-C line, pasted with Ctrl-P.
    line_clip: String,
    /// Pre-write compose + history per Monitor (0) and channel (1..=N).
    compose: Vec<ComposeSlot>,
    pads: Vec<TextPad>,
    /// Spy windows `1..=10` (index 0 = spy 1).
    spy_pads: Vec<TextPad>,
    /// AutoSpy file number for that slot (`CALL.000` …). `None` = manual `.spy`.
    spy_auto_nnn: Vec<Option<u16>>,
    /// Per call, the AutoSpy `.NNN` chosen for this program run (reuse after Kill).
    spy_auto_run: HashMap<String, u16>,
    /// BBS-Menu pad (`ActivePane::Check`).
    bbs_pad: TextPad,
    /// DieBox-List pad (`ActivePane::List`).
    box_pad: TextPad,
    bbs_rows: Vec<MailEntry>,
    box_rows: Vec<MailEntry>,
    bbs_cursor: usize,
    box_cursor: usize,
    bbs_pending: Vec<MailEntry>,
    box_pending: Vec<MailEntry>,
    bbs_from: Option<u8>,
    box_from: Option<u8>,
    bbs_sort_desc: bool,
    /// New DieBox list waiting for replace / append / interrupt.
    list_offer: Option<ListOffer>,
    /// AutoBin receive. DOS waits on `#OK#` before the file bytes.
    autobin: Option<LiveAutoBin>,
    /// Paced 7+ part send (`//Read name.p01`) with Transfer80 progress.
    seven_tx: Option<LiveSevenTx>,
    /// One in-flight file frame after a local `#ABORT#`. Hidden unless it is `<GP>:`.
    bin_rx_swallow: Option<u8>,
    /// `//WPrg name`: next `#BIN#` on this channel is saved into USER as `name`.
    wprg_name: Option<(u8, String)>,
    /// `//CUT` already stopped the transfer while this line was sniffed.
    transfer_cut: bool,
    find_open: bool,
    find_text: String,
    find_last: String,
    /// Soft event / command log (mode changes, etc.).
    event_log: VecDeque<String>,
    channels: u8,
    /// Peer callsign per channel index (`1..=channels`); `None` = disconnected.
    channel_peer: Vec<Option<String>>,
    /// Last real call on that channel. Kept after disconnect so Save still names the file.
    channel_last_call: Vec<Option<String>>,
    /// Channel lock. Typed chat is sealed; a sealed incoming line turns it on.
    channel_lock: Vec<bool>,
    /// `//RTT` stamp we sent, and the monotonic instant we sent it.
    /// The number on the air is Unix milliseconds. The instant is what we
    /// subtract when that same number comes back, so a clock step mid-trip
    /// does not change the answer.
    rtt_probe: Vec<Option<(u64, Instant)>>,
    /// Per channel: `//n` chat-mode target (`None` = not bridging).
    channel_chat: Vec<Option<u8>>,
    /// Set while a chat bridge/join notice is on the air — blocks re-relay.
    chat_bridging: bool,
    /// Label attention. A connect or new text while this channel is not
    /// on screen. Opening the channel clears it. A disconnect does not:
    /// the label goes back to dots and keeps blinking until it is opened.
    /// The callsign blinks white/red. The button face stays unreversed.
    channel_unread: Vec<bool>,
    /// Open QSO for `<base call>.GPL`. Index matches `channel_peer`.
    qso_open: Vec<Option<OpenQso>>,
    /// Waiting for a sysop challenge (`SY` / `PW`, or a link that has a `.GPW`).
    gpw_armed: Vec<bool>,
    /// Last calculated sysop reply. Ctrl-B pastes it into the compose box.
    last_gpw: String,
    /// In-progress connect target per channel (not linked yet).
    channel_connecting: Vec<Option<String>>,
    /// This station placed the call. Incoming links stay false.
    channel_outgoing: Vec<bool>,
    disconnecting: Vec<bool>,
    next_packet_id: u64,
    /// Ephemeral on-air SSID when the same from↔to pair is already up.
    /// Not the channel number, and not written to Config.toml.
    temp_slot_call: Vec<Option<String>>,
    /// Digipeaters for the live KISS link on this channel.
    channel_via: Vec<Vec<String>>,
    /// Radio port for a live link. Empty until the session reports one.
    channel_link_port: Vec<Option<u8>>,
    /// Node `C` / `C!` lines still to send after the AX.25 link is up.
    channel_down: Vec<Vec<String>>,
    /// Callsign the next incoming line must mention before the next downlink.
    channel_down_wait: Vec<Option<String>>,
    /// `data/NAMES.GP` — names and autorouter paths.
    names: NamesFile,
    /// ALT-N / Name: edit station details in `NAMES.GP`.
    show_name_dialog: bool,
    name_edit: NameEntry,
    name_source: Option<usize>,
    name_error: String,
    name_channel: u8,
    name_dialog_focus: bool,
    /// Focus the connect dialog call field once when opened.
    connect_dialog_focus: bool,
    /// Name list popped up from the connect dialog's Name field.
    show_connect_names: bool,
    icon_tex: Vec<TextureHandle>,
    /// `assets/start.png`, drawn above the radio card until Use this.
    start_screen: Option<TextureHandle>,
    /// Last measured height of the TFPCX radio card, so the picture can leave room for it.
    start_card_height: f32,
    /// `assets/network.png` — COM send/receive lamp left of ScrLock.
    link_tex: Option<TextureHandle>,
    /// Startup gate. The serial or TCP link stays closed until Use this.
    show_radio_gate: bool,
    radio: RadioLink,
    /// Global call (`mycall[0]`) shown first on the startup card.
    radio_call: String,
    /// Own Maidenhead QTH (`station.locator`) on the startup card.
    radio_locator: String,
    /// Call loaded from Config, so a change can be announced once.
    radio_call_loaded: String,
    radio_protocol: HostProtocol,
    /// KISS serial/TCP choice. WA8DED stays on serial.
    radio_tcp: bool,
    /// TFPCX: up to three modems. Unused for the other protocols.
    radio_links: Vec<RadioLinkForm>,
    /// Which TFPCX link the port / baud / connection list is editing.
    radio_link: Option<usize>,
    radio_port: String,
    /// Last port written to Config.toml, and the device name seen on it.
    radio_saved_port: String,
    radio_saved_detail: String,
    radio_baud: u32,
    radio_host: String,
    radio_tcp_port: String,
    radio_ports: Vec<DetectedPort>,
    radio_ports_at: Option<Instant>,
    /// Ports the OS reported, not counting a saved name that is unplugged.
    radio_live_ports: usize,
    radio_error: String,
    /// Second panel for Protocol, Port, Baud, and the KISS connection.
    radio_pick: Option<RadioPick>,
    /// WA8DED monitor flags, e.g. `IUSC` (from TNC / local set).
    monitor_mode: String,
    /// KISS / TFPCX monitor listing. WA8DED applies the same rule in the modem.
    monitor_filter: MonitorFilter,
    /// Next info-only monitor row follows a header already judged.
    /// `Some(false)` hides it; `Some(true)` keeps it with that header.
    monitor_follow: Option<bool>,
    /// Firmware-native `@B` count, or synthetic KISS/TFPCX capacity.
    tnc_free: u32,
    /// Receive frames the modem has not handed up yet (monitor row).
    mon_not_disp: u32,
    ch_not_disp: Vec<u32>,
    ch_not_sent: Vec<u32>,
    ch_tries: Vec<u32>,
    ch_unack: Vec<u32>,
    /// Per channel: `//Q` waiting for QRT to leave the TX queue before DISC.
    qrt_disc: Vec<QrtDiscState>,
    /// Open terminal row on a channel. `None` means the next byte starts a row.
    rx_row: Vec<Option<u64>>,
    /// AX.25 from/to for the text arriving on this channel. The lock uses these,
    /// not a key carried in the line.
    rx_calls: Vec<Option<(String, String)>>,
    /// Previous packet ended on CR; the next LF is the same newline.
    rx_cr: Vec<bool>,
    /// Colour pen for the channel. A packet cut does not reset it.
    rx_ansi: Vec<AnsiDecoder>,
    /// Unfinished 7Plus row per channel (raw payload bytes, not the painted text).
    seven_pending: Vec<Vec<u8>>,
    /// Last 7Plus packet ended on CR; skip a leading LF on the next chunk.
    seven_cr: Vec<bool>,
    /// One 7Plus sniffer per channel. Index 0 (monitor) is unused.
    seven: Vec<SevenPlusAutosave>,
    /// `//Write` capture, one slot per channel (0 = local compose).
    user_write: Vec<Option<UserWrite>>,
    /// Call waiting for the next line after `//SEND`.
    mail_send: Vec<Option<String>>,
    /// Lowercase names this session stored with `//Write`. `//DELete` only removes those.
    user_written: Vec<String>,
    /// `//RUN` script, one per channel. The link ending drops it.
    external_run: Vec<external::Gate>,
    /// What that program has printed, plus lines typed into it. Sysop view only.
    external_screen: Vec<String>,
    /// `//PRompt ON` — send the prompt template after a remote command.
    remote_prompt: bool,
    /// Last WA8DED timing banner (`//P` / open `[status]`).
    timing_banner: Option<String>,
    /// Live `QueryTiming` reply target: (channel, pane to restore afterward).
    pending_timing: Option<(u8, ActivePane)>,
    /// One session-bar badge and lamp per modem. A single TNC has one.
    modems: Vec<ModemFace>,
    /// UART link up when any modem badge is green.
    serial_open: bool,
    /// Raw UART bytes seen since start (proves RX even without frames).
    serial_rx_bytes: u64,
    /// Decoded hostmode/KISS frames since start.
    serial_rx_frames: u64,
    monitor_bg: Color32,
    /// Output-window foreground: `fm … to …` header text.
    monitor_fg: Color32,
    /// Output-window inverse: payload under that header.
    monitor_inv: Color32,
    /// Output-window Info / Error: TNC, COM, warnings, "Local call set to".
    monitor_info: Color32,
    /// Clock column (`clock_background`).
    clock_bg: Color32,
    state_bg: Color32,
    state_fg: Color32,
    input_bg: Color32,
    input_fg: Color32,
    menu_bg: Color32,
    menu_fg: Color32,
    editor_bg: Color32,
    editor_fg: Color32,
    check_bg: Color32,
    list_bg: Color32,
    mh_bg: Color32,
    mh_fg: Color32,
    mh_inv: Color32,
    spy_bg: Color32,
    spy_fg: Color32,
    spy_inv: Color32,
    map_bg: Color32,
    map_fg: Color32,
    map_grid: Color32,
    map_qth: Color32,
    map_heard: Color32,
    /// Pan / zoom for the Map pane.
    map_view: MapView,
    /// First Map open this session: fit a centred world once we know the pane aspect.
    map_needs_world_fit: bool,
    /// Last hover Maidenhead under the pointer on the map.
    map_hover_loc: Option<String>,
    connect_bg: Color32,
    #[allow(dead_code)]
    connect_fg: Color32,
    session: SessionChrome,
    /// Internal GPI / text editor (EDIT / CT → session connect-text file).
    editor: GpEditor,
    /// The editor buffer stays until Load or quit. The first EDIT opens CTEXT.
    editor_held: bool,
    /// Put the caret back on the first frame after returning to Edit.
    editor_resume: bool,
    /// Stations heard (APRS/AX.25 UI) — JSON-backed.
    mheard: MheardList,
    /// Selected row in the MHeard list (`None` = none).
    mheard_selected: Option<usize>,
    /// Opening the list shows the newest stations, which are the first rows.
    mheard_scroll_top: bool,
    /// After SPY on MHeard: next list pick opens the confirm dialog.
    mheard_spy_pick: bool,
    /// Confirm dialog: Start Spy for {call}?
    show_mheard_spy_confirm: bool,
    mheard_spy_pending: String,
    /// Last Auto Clean pass. `None` runs on the next tick when the day count is set.
    mheard_clean_at: Option<Instant>,
    /// Day count when Setup opened. A new count is applied only when Setup closes.
    mheard_clean_opened: u32,
    bell_wav: PathBuf,
    /// Hourly clock ♫. `//BELL` keeps `bell_wav`.
    chime_wav: PathBuf,
    connect_wav: PathBuf,
    disconnect_wav: PathBuf,
    newline_wav: PathBuf,
    /// GPI / CTEXT echo: keep one tick for the whole send, not one per row.
    newline_sound_quiet: bool,
    newline_sound_pending: bool,
    /// Channel that queued a coalesced newline click.
    newline_sound_channel: u8,
    /// Prevents double-fire within the same hour.
    last_gong: Option<(chrono::NaiveDate, u32)>,
    /// Last key / mouse activity (for `//Activity`). Updated from egui input.
    last_sysop_activity: Instant,
    show_setup: bool,
    /// Face last given to egui. `true` is JetBrains Mono Bold (`Zoom` on).
    console_bold: bool,
    setup_pane: ActivePane,
    setup_cursor: usize,
    setup_view: SetupView,
    /// Which colour line has its name list open (Background, Foreground, Inverse).
    setup_color_pick: Option<usize>,
    /// System Icons list is open over the program parameters.
    setup_icon_pick: bool,
    /// Pixel editor for the icon library on the bar.
    icon_editor: Option<IconEdit>,
    show_tnc: bool,
    tnc_cmd: String,
    /// Wall-clock beacon. Not the paint loop: a covered window stops painting.
    beacon_sched: BeaconSchedule,
    /// Window place changed and still needs a write to Config.toml.
    window_dirty: bool,
    /// When the place last changed. The file is written after it settles.
    window_dirty_at: Instant,
    /// ALT-Enter exclusive fullscreen (title bar off). Session-only.
    fullscreen: bool,
}

impl GpGuiApp {
    pub fn new(ctx: &Context, cfg: Config, config_path: PathBuf, radio: RadioLink) -> Self {
        let channels = cfg.session.channels.clamp(1, 10) as u8;
        let mon_max = cfg.buffers.text_buffer.first().copied().unwrap_or(600) as usize;
        let ch_max = cfg.buffers.text_buffer.get(1).copied().unwrap_or(300) as usize;
        let spy_max = cfg.buffers.spy_buffer.max(1) as usize;
        let bbs_max = cfg.buffers.check_buffer.max(1) as usize;
        let box_max = cfg.buffers.list_buffer.max(1) as usize;
        let log_dir = cfg.paths.log_dir.clone();

        let mut pads = Vec::with_capacity(channels as usize + 1);
        let mut channel_peer = Vec::with_capacity(channels as usize + 1);
        let mut channel_lock = Vec::with_capacity(channels as usize + 1);
        let mut channel_chat = Vec::with_capacity(channels as usize + 1);
        let mut channel_unread = Vec::with_capacity(channels as usize + 1);
        let mut qso_open = Vec::with_capacity(channels as usize + 1);
        let mut gpw_armed = Vec::with_capacity(channels as usize + 1);
        let mut channel_connecting = Vec::with_capacity(channels as usize + 1);
        let mut channel_outgoing = Vec::with_capacity(channels as usize + 1);
        let mut temp_slot_call = Vec::with_capacity(channels as usize + 1);
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
            channel_lock.push(false);
            channel_chat.push(None);
            channel_unread.push(false);
            qso_open.push(None);
            gpw_armed.push(false);
            channel_connecting.push(None);
            channel_outgoing.push(false);
            temp_slot_call.push(None);
        }
        let mut channel_via = Vec::with_capacity(channels as usize + 1);
        let mut channel_down = Vec::with_capacity(channels as usize + 1);
        let mut channel_down_wait = Vec::with_capacity(channels as usize + 1);
        let mut channel_link_port = Vec::with_capacity(channels as usize + 1);
        for _ in 0..=channels {
            channel_via.push(Vec::new());
            channel_down.push(Vec::new());
            channel_down_wait.push(None);
            channel_link_port.push(None);
        }
        // Local call stays on the monitor. The credits line is on the start screen.
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
        let console_bold = zoom_bold(cfg.session.zoom);
        install_console_font(ctx, console_bold);
        let icon_tex = load_icon_textures(ctx, &cfg.session.icon_file);
        let start_screen = load_start_screen(ctx);
        let link_tex = load_link_lamps(ctx);
        let status = if icon_tex.is_empty() {
            "data/ICONS.GP not found".into()
        } else {
            format!("{} icons", icon_tex.len())
        };

        let (cmd_tx, _idle_rx) = mpsc::channel(1);
        let (_idle_tx, events) = mpsc::channel(1);
        let radio_call = cfg.primary_mycall().to_string();
        let radio_locator = cfg.station.locator.clone();
        let radio_call_loaded = radio_call.clone();
        let radio_protocol = cfg.tnc.protocol_kind();
        let radio_tcp = cfg.tnc.kiss_transport.eq_ignore_ascii_case("tcp");
        let radio_port = cfg.serial.port.clone();
        let radio_saved_port = radio_port.clone();
        let radio_saved_detail = cfg.serial.port_detail.clone();
        let radio_baud = if radio_protocol == HostProtocol::Baycom {
            1200
        } else {
            cfg.serial.baud
        };
        let radio_host = cfg.tnc.kiss_tcp_host.clone();
        let radio_tcp_port = cfg.tnc.kiss_tcp_port.to_string();
        let radio_links = radio_link_forms(
            &cfg.tnc,
            &radio_port,
            radio_baud,
            &radio_host,
            &radio_tcp_port,
        );

        let colors = &cfg.colors;
        let modems = modem_faces(&cfg);
        let monitor_filter = MonitorFilter::from_config(&cfg.tnc.monitor_mode);
        let monitor_mode = monitor_filter.label();
        let mut session = SessionChrome::from_config(&cfg.session.chrome);
        if !cfg.session.qrg_text.trim().is_empty() {
            session.qrg_log = journal::clip_qrg(&cfg.session.qrg_text);
            if let Ok(mhz) = session.qrg_log.parse::<f64>() {
                session.qrg_mhz = mhz;
            }
        }
        if let Err(err) = sevenplus::ensure_user_dir(&cfg.paths.user_dir) {
            tracing::warn!("USER dir: {err}");
        }
        if let Err(err) = crate::paths::ensure_dir(&cfg.paths.external_dir) {
            tracing::warn!("EXTERNAL dir: {err}");
        }
        let beacon_sched = BeaconSchedule::start(
            ctx.clone(),
            BeaconSnap {
                kiss: cfg.tnc.runs_kiss_ax25(),
                port: cfg.tnc.kiss_tx_port(),
                beacon: cfg.beacon.clone(),
                my_call: cfg.primary_mycall().to_string(),
                qrg: session.qrg_text(),
                version: version::APP_VERSION.into(),
                cmd_tx: cmd_tx.clone(),
                mail_dir: mail::mail_dir(),
            },
        );
        Self {
            monitor_bg: egui_color(&colors.monitor_background),
            monitor_fg: egui_color(&colors.monitor_foreground),
            monitor_inv: egui_color(&colors.monitor_invers_attr),
            monitor_info: egui_color(&colors.monitor_infoerror),
            clock_bg: egui_color(&colors.clock_background),
            state_bg: egui_color(&colors.state_background),
            state_fg: egui_color(&colors.state_foreground),
            input_bg: egui_color(&colors.input_background),
            input_fg: egui_color(&colors.input_foreground),
            menu_bg: egui_color(&colors.menu_background),
            menu_fg: egui_color(&colors.menu_foreground),
            editor_bg: egui_color(&colors.editor_background),
            editor_fg: egui_color(&colors.editor_foreground),
            check_bg: egui_color(&colors.check_background),
            list_bg: egui_color(&colors.list_background),
            mh_bg: egui_color(&colors.mh_background),
            mh_fg: egui_color(&colors.mh_foreground),
            mh_inv: egui_color(&colors.mh_invers_attr),
            spy_bg: egui_color(&colors.spy_background),
            spy_fg: egui_color(&colors.spy_foreground),
            spy_inv: egui_color(&colors.spy_invers_attr),
            map_bg: egui_color(&colors.map_background),
            map_fg: egui_color(&colors.map_foreground),
            map_grid: egui_color(&colors.map_grid),
            map_qth: egui_color(&colors.map_qth),
            map_heard: egui_color(&colors.map_heard),
            map_view: MapView::default(),
            map_needs_world_fit: true,
            map_hover_loc: None,
            connect_bg: egui_color(&colors.connect_background),
            connect_fg: egui_color(&colors.connect_foreground),
            cfg,
            config_path,
            events,
            cmd_tx,
            active: ActivePane::Monitor,
            split_on: false,
            split_lower: 0.40,
            split_body_h: 0.0,
            split_dragging: false,
            split_drag_dy: 0.0,
            status,
            last_link_status: String::new(),
            last_error: String::new(),
            line_clip: String::new(),
            compose: (0..slots).map(|_| ComposeSlot::default()).collect(),
            pads,
            spy_pads,
            spy_auto_nnn: vec![None; 10],
            spy_auto_run: HashMap::new(),
            bbs_pad,
            box_pad,
            bbs_rows: Vec::new(),
            box_rows: Vec::new(),
            bbs_cursor: 0,
            box_cursor: 0,
            bbs_pending: Vec::new(),
            box_pending: Vec::new(),
            bbs_from: None,
            box_from: None,
            bbs_sort_desc: false,
            list_offer: None,
            autobin: None,
            seven_tx: None,
            bin_rx_swallow: None,
            wprg_name: None,
            transfer_cut: false,
            find_open: false,
            find_text: String::new(),
            find_last: String::new(),
            event_log: VecDeque::new(),
            channels,
            channel_peer,
            channel_last_call: load_last_calls(
                &padlog::pads_dir(&log_dir).join("lastcall.txt"),
                channels as usize + 1,
            ),
            channel_lock,
            rtt_probe: vec![None; slots],
            channel_chat,
            chat_bridging: false,
            channel_unread,
            qso_open,
            gpw_armed,
            last_gpw: String::new(),
            channel_connecting,
            channel_outgoing,
            disconnecting: vec![false; channels as usize + 1],
            next_packet_id: 0,
            temp_slot_call,
            channel_via,
            channel_link_port,
            channel_down,
            channel_down_wait,
            names: NamesFile::load_default(),
            show_name_dialog: false,
            name_edit: NameEntry::default(),
            name_source: None,
            name_error: String::new(),
            name_channel: 1,
            name_dialog_focus: false,
            connect_dialog_focus: false,
            show_connect_names: false,
            icon_tex,
            start_screen,
            start_card_height: 0.0,
            link_tex,
            show_radio_gate: true,
            radio,
            radio_call,
            radio_locator,
            radio_call_loaded,
            radio_protocol,
            radio_tcp,
            radio_links,
            radio_link: None,
            radio_port,
            radio_saved_port,
            radio_saved_detail,
            radio_baud,
            radio_host,
            radio_tcp_port,
            radio_ports: Vec::new(),
            radio_ports_at: None,
            radio_live_ports: 0,
            radio_error: String::new(),
            radio_pick: None,
            monitor_mode,
            monitor_filter,
            monitor_follow: None,
            tnc_free: TNC_RAM,
            mon_not_disp: 0,
            ch_not_disp: vec![0; slots],
            ch_not_sent: vec![0; slots],
            ch_tries: vec![0; slots],
            ch_unack: vec![0; slots],
            qrt_disc: vec![QrtDiscState::Idle; slots],
            rx_row: vec![None; slots],
            rx_calls: vec![None; slots],
            rx_cr: vec![false; slots],
            rx_ansi: vec![AnsiDecoder::new(); slots],
            seven_pending: vec![Vec::new(); slots],
            seven_cr: vec![false; slots],
            seven: (0..slots)
                .map(|ch| SevenPlusAutosave::new(session.seven_plus_for(ch as u8)))
                .collect(),
            user_write: (0..slots).map(|_| None).collect(),
            mail_send: vec![None; slots],
            user_written: Vec::new(),
            external_run: (0..slots).map(|_| external::Gate::new()).collect(),
            external_screen: vec![String::new(); slots],
            remote_prompt: false,
            timing_banner: None,
            pending_timing: None,
            modems,
            serial_open: false,
            serial_rx_bytes: 0,
            serial_rx_frames: 0,
            session,
            editor: GpEditor::default(),
            editor_held: false,
            editor_resume: false,
            mheard: MheardList::load_default(),
            mheard_selected: None,
            mheard_scroll_top: false,
            mheard_spy_pick: false,
            show_mheard_spy_confirm: false,
            mheard_spy_pending: String::new(),
            mheard_clean_at: None,
            mheard_clean_opened: 0,
            bell_wav: default_bell_wav(),
            chime_wav: default_chime_wav(),
            connect_wav: default_connect_wav(),
            disconnect_wav: default_disconnect_wav(),
            newline_wav: default_newline_wav(),
            newline_sound_quiet: false,
            newline_sound_pending: false,
            newline_sound_channel: 0,
            last_gong: None,
            last_sysop_activity: Instant::now(),
            show_setup: false,
            console_bold,
            setup_pane: ActivePane::Monitor,
            setup_cursor: 0,
            setup_view: SetupView::Main,
            setup_color_pick: None,
            setup_icon_pick: false,
            icon_editor: None,
            show_tnc: false,
            tnc_cmd: String::new(),
            beacon_sched,
            window_dirty: false,
            window_dirty_at: Instant::now(),
            fullscreen: false,
        }
    }

    /// Load CTEXT the first time. After that, Edit returns to the open file.
    fn open_ctext_editor(&mut self) {
        if self.editor_held {
            self.select(ActivePane::Editor);
            self.editor_resume = true;
            self.status = format!("EDIT {}", self.editor.display_name());
            return;
        }
        self.select(ActivePane::Editor);
        let path = self.resolve_ctext_path();
        match self.editor.load_path(&path) {
            Ok(()) => {
                self.session.set_ctext_path(path);
                self.status = format!("EDIT {}", self.editor.display_name());
            }
            Err(e) => {
                self.editor.path = path.clone();
                self.editor.text.clear();
                self.editor.dirty = false;
                self.session.set_ctext_path(path);
                self.status = e;
            }
        }
        self.editor_held = true;
    }

    /// Prefer the path on the CT button; fall back to classic `CTEXT.GPI`.
    fn resolve_ctext_path(&self) -> PathBuf {
        self.resolve_stored_ctext(&self.session.ctext_path)
    }

    /// File this channel sends. Its own path when one is set, otherwise global.
    fn ctext_path_for(&self, channel: u8) -> PathBuf {
        self.resolve_stored_ctext(self.session.ctext_for(channel))
    }

    /// `0` is the Monitor (global file). A channel number is that channel's file.
    fn ctext_slot(&self) -> u8 {
        match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        }
    }

    fn resolve_stored_ctext(&self, path: &std::path::Path) -> PathBuf {
        if path.as_os_str().is_empty() {
            return gpi::default_ctext_path();
        }
        if path.is_file() {
            return path.to_path_buf();
        }
        // Relative name from Config → try under data/.
        if path.is_relative() {
            let under_data = crate::paths::data_dir().join(path);
            if under_data.is_file() {
                return under_data;
            }
        }
        // Stem-only / renamed: resolve CTEXT.GPI (or leftover GTEXT.GPI).
        gpi::default_ctext_path()
    }

    /// Remember the station on this channel. Disconnect does not clear it.
    fn note_last_call(&mut self, channel: u8, call: &str) {
        let call = transcript_call(call);
        if call.is_empty() {
            return;
        }
        let Some(slot) = self.channel_last_call.get_mut(channel as usize) else {
            return;
        };
        if slot.as_deref() == Some(call.as_str()) {
            return;
        }
        *slot = Some(call);
        self.store_last_calls();
    }

    fn store_last_calls(&self) {
        let path = padlog::pads_dir(&self.cfg.paths.log_dir).join("lastcall.txt");
        if let Some(parent) = path.parent() {
            let _ = crate::paths::ensure_dir(parent);
        }
        let mut body = String::new();
        for (i, call) in self.channel_last_call.iter().enumerate() {
            if let Some(call) = call {
                body.push_str(&format!("{i} {call}\n"));
            }
        }
        let _ = std::fs::write(path, body);
    }

    fn channel_call_for_save(&self, channel: u8) -> Option<String> {
        let live = self
            .channel_peer
            .get(channel as usize)
            .and_then(|p| p.as_deref())
            .filter(|c| !c.is_empty() && *c != "****");
        let last = self
            .channel_last_call
            .get(channel as usize)
            .and_then(|p| p.as_deref());
        let raw = live.or(last)?;
        let call = transcript_call(raw);
        (!call.is_empty()).then_some(call)
    }

    /// Suggested `*.LOG` name and the visible pad, for the Save dialog.
    fn transcript_for_save(&self) -> Option<(String, String)> {
        let text = |pad: &super::padlog::TextPad| pad.transcript_text();
        match self.active {
            ActivePane::Monitor => {
                let body = self.pads.first().map(text).unwrap_or_default();
                Some(("MONITOR.LOG".into(), body))
            }
            ActivePane::Channel(n) => {
                let name = self
                    .channel_call_for_save(n)
                    .map(|call| format!("{call}.LOG"))
                    .unwrap_or_else(|| format!("CH{n:02}.LOG"));
                let body = self.pads.get(n as usize).map(text).unwrap_or_default();
                Some((name, body))
            }
            ActivePane::Check => Some(("BBS.LOG".into(), self.bbs_pad.transcript_text())),
            ActivePane::List => Some(("BOX.LOG".into(), self.box_pad.transcript_text())),
            ActivePane::Spy(n) => {
                let call = transcript_call(self.spy_call(n));
                let name = if call.is_empty() {
                    format!("SPY{n:02}.LOG")
                } else {
                    format!("{call}.LOG")
                };
                let body = self
                    .spy_pads
                    .get(n.saturating_sub(1) as usize)
                    .map(text)
                    .unwrap_or_default();
                Some((name, body))
            }
            _ => None,
        }
    }

    /// Ask for a `*.LOG` name in the LOG folder, then write this screen's transcript.
    fn save_screen_transcript(&mut self) {
        let Some((name, body)) = self.transcript_for_save() else {
            self.status = "Save — this window has no transcript".into();
            return;
        };
        if body.is_empty() {
            self.status = format!("Save — {} has no text", self.pane_title());
            return;
        }
        let dir = self.cfg.paths.log_dir.clone();
        if let Err(e) = crate::paths::ensure_dir(&dir) {
            self.status = format!("Save: {e}");
            return;
        }
        let dialog = rfd::FileDialog::new()
            .add_filter("Log", &["log", "LOG"])
            .set_title("Save transcript")
            .set_file_name(&name)
            .set_directory(&dir);
        let Some(path) = dialog.save_file() else {
            return;
        };
        let path = with_log_extension(path);
        if let Some(parent) = path.parent() {
            if let Err(e) = crate::paths::ensure_dir(parent) {
                self.status = format!("Save: {e}");
                return;
            }
        }
        match std::fs::write(&path, body) {
            Ok(()) => {
                let shown = path.file_name().and_then(|n| n.to_str()).unwrap_or("log");
                self.status = format!("saved {shown}");
            }
            Err(e) => self.status = format!("Save: {e}"),
        }
    }

    /// Append one spy row. AutoSpy uses `SpyDir/CALL.NNN`; manual uses `SAVE/CALL.spy`.
    fn append_spy_line(&self, call: &str, line: &str, auto_nnn: Option<u16>) {
        let call = transcript_call(call);
        if call.is_empty() {
            return;
        }
        let text = line.replace(['\r', '\n'], " ").trim().to_string();
        if text.is_empty() {
            return;
        }
        let (dir, name) = match auto_nnn {
            Some(nnn) => (self.cfg.paths.spy_dir.as_path(), format!("{call}.{nnn:03}")),
            None => (self.cfg.paths.save_dir.as_path(), format!("{call}.spy")),
        };
        if let Err(e) = crate::paths::ensure_dir(dir) {
            tracing::warn!("spy save mkdir: {e}");
            return;
        }
        let path = dir.join(name);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);
        let mut file = match file {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!("spy save {}: {e}", path.display());
                return;
            }
        };
        use std::io::Write;
        if let Err(e) = write!(file, "{text}\r\n") {
            tracing::warn!("spy save {}: {e}", path.display());
        }
    }

    /// Save dialog with the current name filled in. Confirming writes that
    /// name; the folder and the name can both be changed first. Cancel writes nothing.
    fn editor_save(&mut self) {
        let current = self.editor.path.clone();
        let start = current
            .parent()
            .filter(|p| p.is_dir())
            .map(std::path::PathBuf::from)
            .or_else(|| Some(crate::paths::data_dir()));
        let mut dialog = rfd::FileDialog::new()
            .add_filter("GPI / text", &["gpi", "GPI", "txt", "TXT"])
            .set_title("Save GPI / text")
            .set_file_name(self.editor.display_name());
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        if self.cfg.session.editor_bak && path.exists() {
            let bak = path.with_extension("BAK");
            if let Err(e) = std::fs::copy(&path, &bak) {
                self.log_event(format!("BAK: {e}"));
            }
        }
        let before = current;
        match self.editor.save_as(&path) {
            Ok(()) => {
                self.status = format!("saved {}", self.editor.display_name());
                // Keep CT on the file we just edited (including a renamed save).
                if self.session.ctext_path == before
                    || self.session.ctext_path == self.editor.path
                    || paths_same_connect_text(&self.session.ctext_path, &before)
                {
                    self.session.set_ctext_path(self.editor.path.clone());
                    self.persist_session_chrome();
                }
            }
            Err(e) => {
                self.status = e.clone();
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
                    self.editor_held = true;
                    self.editor_resume = false;
                    self.status = format!("loaded {}", self.editor.display_name());
                    self.select(ActivePane::Editor);
                }
                Err(e) => {
                    self.status = e.clone();
                }
            }
        }
    }

    fn maybe_hour_gong(&mut self) {
        if !self.session.hour_gong {
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
        self.play_hour_chime(now.hour());
    }

    /// One `CHIME.WAV` per hour on a 12-hour clock, 40 ms between strikes.
    fn play_hour_chime(&self, hour24: u32) {
        play_wav_times(&self.chime_wav, chime_strikes(hour24), Duration::ZERO, 0.25);
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
                TncEvent::TimingBanner(banner) => {
                    self.timing_banner = Some(banner.clone());
                    if let Some((ch, prev)) = self.pending_timing.take() {
                        if ch >= 1 {
                            self.active = ActivePane::Channel(ch);
                        }
                        self.send_report_block("Parameters", &banner);
                        self.close_remote(prev);
                    }
                }
                TncEvent::Status(m) => {
                    self.status = m.clone();
                    let low = m.to_ascii_lowercase();
                    // Serial Kenwood / TCP Soundmodem / hostmode bring-up → green COM badge.
                    let link_up = low.starts_with("serial open")
                        || low.starts_with("tcp open")
                        || low == "wa8ded tnc2 hostmode ready"
                        || low == "baycom ft232r ready (experimental)"
                        || low.contains("tfpcx driver");
                    if link_up {
                        let was_open = self.serial_open;
                        if self.modems.len() <= 1 {
                            if let Some(face) = self.modems.first_mut() {
                                face.phase = LinkPhase::Open;
                            }
                            self.serial_open = true;
                        }
                        if low.starts_with("serial open") || low.starts_with("tcp open") {
                            self.log_event(m.clone());
                        }
                        if !was_open || low == "wa8ded tnc2 hostmode ready" {
                            self.sync_channel_identities();
                        }
                    }
                    if low.contains("serial refresh") && !low.starts_with("serial open") {
                        self.log_event(m);
                    }
                }
                TncEvent::Warning(m) => {
                    self.status = m.clone();
                    self.last_error = m.clone();
                    // Keep real failures; skip routine serial `KISS.INI:` bring-up noise.
                    let low = m.to_ascii_lowercase();
                    if !low.starts_with("kiss.ini") {
                        self.log_event(format!("! {m}"));
                    }
                }
                TncEvent::LinkDown(m) => {
                    self.timing_banner = None;
                    self.pending_timing = None;
                    self.status = format!("link down: {m}");
                    for face in &mut self.modems {
                        if face.phase == LinkPhase::Open {
                            face.phase = LinkPhase::Pending;
                        }
                    }
                    self.serial_open = false;
                    self.log_event(format!("link down: {m}"));
                    for ch in 1..=self.channels {
                        self.close_journal(ch);
                    }
                    for peer in &mut self.channel_peer {
                        *peer = None;
                    }
                    for flag in &mut self.channel_lock {
                        *flag = false;
                    }
                    for calls in &mut self.rx_calls {
                        *calls = None;
                    }
                    for c in &mut self.channel_connecting {
                        *c = None;
                    }
                    for flag in &mut self.channel_outgoing {
                        *flag = false;
                    }
                    for via in &mut self.channel_via {
                        via.clear();
                    }
                    for port in &mut self.channel_link_port {
                        *port = None;
                    }
                    for down in &mut self.channel_down {
                        down.clear();
                    }
                    for wait in &mut self.channel_down_wait {
                        *wait = None;
                    }
                }
                // UART counters only — decoded frames land on the monitor.
                TncEvent::RawRx(bytes) => {
                    self.serial_rx_bytes += bytes.len() as u64;
                }
                TncEvent::LinkIo { tx, index } => {
                    let until = Instant::now() + LINK_LAMP_HOLD;
                    if let Some(face) = self.modems.get_mut(usize::from(index)) {
                        if tx {
                            face.tx_until = Some(until);
                        } else {
                            face.rx_until = Some(until);
                        }
                    }
                }
                TncEvent::Modem {
                    index,
                    label,
                    phase,
                } => {
                    self.note_modem(index, label, phase);
                }
                TncEvent::Buffers(report) => self.apply_buffers(report),
                TncEvent::Session(event) => self.handle_session_event(event),
                TncEvent::KissTx { port, frame } => {
                    self.note_kiss_tx(port, &frame);
                }
                TncEvent::HostFrame {
                    channel,
                    code,
                    payload,
                } => {
                    self.serial_rx_frames += 1;
                    let text = decode_rx_text(&payload, self.session.umlaut_for(channel));
                    // WA8DED answers a bare `M` with the live flags (code 1). That
                    // reply is the monitor line, not a frame to list.
                    if !self.uses_software_monitor()
                        && channel == 0
                        && matches!(code, HostCode::CommandReply)
                    {
                        let reply = text.trim().trim_matches('\0').trim();
                        if !reply.is_empty() && reply.split_whitespace().count() <= 9 {
                            if let Ok(filter) =
                                MonitorFilter::apply(&MonitorFilter::default(), reply)
                            {
                                self.monitor_filter = filter;
                                self.monitor_mode = self.monitor_filter.label();
                                self.log_event(format!(
                                    "monitor mode ← TNC: {}",
                                    self.monitor_mode
                                ));
                                continue;
                            }
                        }
                    }
                    let mut spy_src: Option<String> = None;
                    // TNC monitor text (and ch0 text that is not an AX.25 summary).
                    // Those bytes are shown with `<0xNN>` for non-printables.
                    let mut monitor_bytes = false;
                    let (kind, line) = match code {
                        HostCode::Ok => continue,
                        HostCode::Monitor | HostCode::MonitorHeader | HostCode::MonitorData => {
                            spy_src = extract_heard_call(&text);
                            monitor_bytes = true;
                            (PadKind::Incoming, text)
                        }
                        HostCode::LinkStatus | HostCode::CommandReply | HostCode::Error => {
                            (PadKind::System, text)
                        }
                        HostCode::Data => {
                            // KISS: full AX.25 L2 (UI / SABM / I / …) — not hostmode text.
                            // TFPCX kiss ports arrive the same way. WA8DED payload stays text.
                            let kiss_payload = self.software_ax25()
                                || (self.cfg.tnc.protocol_kind() == HostProtocol::Tfpcx
                                    && ax25::parse_frame(&payload).is_some());
                            if kiss_payload {
                                if let Some(frame) = ax25::parse_frame(&payload) {
                                    self.kiss_show_transit(&frame, channel);
                                } else {
                                    self.push_mon_line(
                                        PadKind::System,
                                        super::session::kiss_monitor_fallback(channel, &payload),
                                    );
                                }
                                continue;
                            }
                            if let Some(frame) = parse_ax25_ui(&payload) {
                                spy_src = Some(frame.src.clone());
                                self.note_heard_on_monitor(
                                    &frame.src,
                                    &frame.dest,
                                    &frame.via,
                                    &frame.info,
                                    Some(frame.info_raw.as_slice()),
                                    Some(self.heard_text_port(channel)),
                                );
                                (PadKind::Incoming, frame.display_line())
                            } else if text.chars().any(|c| c.is_ascii_graphic()) {
                                spy_src = extract_heard_call(&text);
                                monitor_bytes = true;
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
                        HostCode::Other(c) => (PadKind::System, format!("{c:#04x}  {text}")),
                    };
                    // KISS port 0 and hostmode ch0 → Monitor; else channel pad.
                    let idx = if channel == 0 {
                        0
                    } else if channel <= self.channels {
                        channel as usize
                    } else {
                        0
                    };
                    // Monitor shows the payload bytes. CR/LF after an `fm` header
                    // stay a row break; every other non-printable is `<0xNN>`.
                    let monitor_rows = if idx == 0 && monitor_bytes {
                        ax25::monitor_display_lines_from_bytes(
                            &payload,
                            self.session.umlaut_for(SLOT_MONITOR),
                        )
                    } else {
                        Vec::new()
                    };
                    // KISS / TFPCX: `M` hides rows on the monitor. The frame
                    // itself was already delivered above when it is channel data.
                    let hide_monitor = idx == 0
                        && monitor_bytes
                        && !self.host_monitor_allowed(&monitor_rows, &line);
                    if !hide_monitor {
                        if let Some(src) = spy_src.as_deref() {
                            if monitor_rows.is_empty() {
                                self.feed_spy_pads(src, kind, &line);
                            } else {
                                for part in &monitor_rows {
                                    self.feed_spy_pads(src, kind, part);
                                }
                            }
                        }
                    }
                    // Ordinary terminal text and `//` commands are rebuilt
                    // across frames. Programs and chat consume packets now.
                    let reassemble = channel >= 1 && matches!(code, HostCode::Data);
                    if reassemble {
                        if kind == PadKind::Incoming {
                            self.note_seven_payload(channel, &payload);
                            if let Some(peer) = self.linked_call(channel) {
                                let mine = self.effective_mycall(channel);
                                self.note_rx_calls(channel, &peer, &mine);
                            }
                        }
                        self.accept_rx_text(channel, &decode_channel_bytes(&payload), kind);
                    } else if !hide_monitor {
                        if idx == 0 && monitor_bytes {
                            self.note_heard_lines(&monitor_rows);
                            let sgr_rows = ax25::monitor_display_lines_sgr(
                                &payload,
                                self.session.umlaut_for(SLOT_MONITOR),
                            );
                            for part in &sgr_rows {
                                if is_fm_header(part) {
                                    self.push_mon_line(kind, stamp_monitor_fm(part));
                                } else {
                                    self.push_mon_line_sgr(kind, part);
                                }
                            }
                        } else {
                            let parts = payload_lines(&line);
                            if idx == 0 {
                                self.note_heard_lines(&parts);
                                for part in parts {
                                    self.push_mon_line(kind, stamp_monitor_fm(&part));
                                }
                            } else if let Some(pad) = self.pads.get_mut(idx) {
                                for part in parts {
                                    pad.push(kind, part);
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
        } else if !matches!(self.active, ActivePane::Mh) {
            self.mheard_scroll_top = true;
        }
        if let ActivePane::Channel(n) = pane {
            self.clear_channel_attention(n);
        }
        self.active = pane;
        self.status = format!("active: {}", pane.label());
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
        if channel == 0 && self.cfg.tnc.has_wa8ded() {
            self.tx_modem_line(channel, &format!("I {call}"));
        }
        self.configure_sessions();
    }

    fn configure_sessions(&mut self) -> bool {
        let configs = (1..=self.channels)
            .map(|channel| ChannelConfig {
                channel,
                port: self.cfg.tnc.channel_port(channel),
                local: self.effective_mycall(channel),
            })
            .collect();
        self.send_session_command(SessionCommand::Configure(configs))
    }

    fn send_session_command(&mut self, command: SessionCommand) -> bool {
        match self.cmd_tx.try_send(TncCommand::Session(command)) {
            Ok(()) => true,
            Err(err) => {
                self.status = format!("radio command not queued: {err}");
                self.last_error = self.status.clone();
                self.log_event(self.status.clone());
                false
            }
        }
    }

    fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Error { channel, reason } => {
                if let Some(ch) = channel.filter(|ch| (1..=self.channels).contains(ch)) {
                    if !self.channel_linked(ch) {
                        self.channel_connecting[usize::from(ch)] = None;
                        self.disconnecting[usize::from(ch)] = false;
                    }
                    self.pads[usize::from(ch)].push(PadKind::System, format!("*** {reason}"));
                    if self
                        .autobin
                        .as_ref()
                        .is_some_and(|live| live.channel == ch && live.tx_id.is_some())
                    {
                        self.autobin = None;
                        self.session.bin_tx = false;
                    }
                    if self
                        .seven_tx
                        .as_ref()
                        .is_some_and(|tx| tx.channel == ch && tx.tx_id.is_some())
                    {
                        self.seven_tx = None;
                    }
                }
                self.status = reason.clone();
                self.last_error = reason.clone();
                self.log_event(format!("! {reason}"));
            }
            SessionEvent::Connected {
                channel,
                port,
                local,
                remote,
                via,
                outgoing,
            } => {
                let idx = usize::from(channel);
                if idx == 0 || idx > usize::from(self.channels) {
                    return;
                }
                let was_linked = self.channel_linked(channel);
                self.channel_connecting[idx] = None;
                self.channel_peer[idx] = Some(remote.clone());
                self.channel_via[idx] = via;
                self.channel_link_port[idx] = Some(port);
                self.disconnecting[idx] = false;
                if !self.effective_mycall(channel).eq_ignore_ascii_case(&local) {
                    self.temp_slot_call[idx] = Some(local);
                }
                self.set_link_outgoing(channel, outgoing);
                self.note_last_call(channel, &remote);
                self.status = format!("*** CONNECTED ch{channel} to {remote}");
                self.pads[idx].push(PadKind::System, self.status.clone());
                if !was_linked {
                    self.play_connect_bell(channel);
                    self.finish_link_up(channel, !outgoing);
                }
            }
            SessionEvent::Disconnected { channel, reason } => {
                if !(1..=self.channels).contains(&channel) {
                    return;
                }
                self.finish_disconnect(channel, true);
                self.pads[usize::from(channel)].push(PadKind::System, format!("*** {reason}"));
            }
            SessionEvent::Data {
                channel,
                local,
                remote,
                data,
            } => {
                if !(1..=self.channels).contains(&channel) {
                    return;
                }
                self.note_rx_calls(channel, &remote, &local);
                if let Some(bytes) = self.autobin_channel_text(channel, &data) {
                    self.note_seven_payload(channel, &bytes);
                    self.accept_rx_text(channel, &decode_channel_bytes(&bytes), PadKind::Incoming);
                }
            }
            SessionEvent::Acknowledged { channel, id } => self.note_bin_tx_ack(channel, id),
            SessionEvent::Reset { channel, reason } => {
                if !(1..=self.channels).contains(&channel) {
                    return;
                }
                let idx = usize::from(channel);
                self.channel_connecting[idx] = self.channel_peer[idx].take();
                if let Some(port) = self.channel_link_port.get_mut(idx) {
                    *port = None;
                }
                self.stop_external(channel);
                self.cancel_mail_send(channel);
                if self
                    .autobin
                    .as_ref()
                    .is_some_and(|live| live.channel == channel)
                {
                    if let Some(live) = self.autobin.as_mut() {
                        Self::discard_recv_temp(live);
                    }
                    self.autobin = None;
                    self.session.bin_rx = false;
                    self.session.bin_tx = false;
                }
                if self
                    .seven_tx
                    .as_ref()
                    .is_some_and(|tx| tx.channel == channel)
                {
                    self.seven_tx = None;
                }
                self.pads[usize::from(channel)]
                    .push(PadKind::System, format!("*** link reset: {reason}"));
            }
        }
    }

    /// Give `channel` a user/config MYCALL and put it on the air.
    /// Several channels may share one name; they form one answer pool.
    /// An empty call joins the global pool.
    fn claim_identity(&mut self, channel: u8, call: &str) -> Result<String, String> {
        if (channel == 0 && self.any_channel_busy()) || self.channel_connected(channel) {
            return Err("disconnect before changing a channel's callsign".into());
        }
        let call = self.cfg.set_mycall_for_slot(channel, call)?;
        if let Some(t) = self.temp_slot_call.get_mut(channel as usize) {
            *t = None;
        }
        self.push_channel_identity(channel);
        Ok(call)
    }

    /// On-air call for this connect. Not written to Config.toml.
    /// Cleared when the channel disconnects. The configured name clears
    /// any leftover `-n` rather than storing the bare call as a temp.
    fn adopt_air_call(&mut self, channel: u8, call: &str) {
        if self.effective_mycall(channel).eq_ignore_ascii_case(call) {
            return;
        }
        if call.eq_ignore_ascii_case(&self.pool_name(channel)) {
            self.clear_temp_identity(channel);
            return;
        }
        if let Some(t) = self.temp_slot_call.get_mut(channel as usize) {
            *t = Some(call.to_string());
        }
        self.push_channel_identity(channel);
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

    /// Configured pool name. Empty, or the same as global, is the global pool.
    fn pool_name(&self, channel: u8) -> String {
        let primary = self.cfg.primary_mycall().to_string();
        let call = self.cfg.mycall_for_slot(channel);
        if call.is_empty() || call.eq_ignore_ascii_case(&primary) {
            primary
        } else {
            call.to_string()
        }
    }

    /// Peer of a linked or still-connecting channel.
    fn channel_other_call(&self, n: u8) -> Option<String> {
        self.channel_peer
            .get(n as usize)
            .and_then(|p| p.clone())
            .or_else(|| {
                self.channel_connecting
                    .get(n as usize)
                    .and_then(|p| p.clone())
            })
    }

    /// Call this station uses for a new link with `peer`.
    ///
    /// The bare pool name while this from↔to pair (either direction) is free,
    /// even when the free channel is 2 or 3. The first extra link of that
    /// same pair is `NAME-1`, the next free number after that. The number
    /// stays on that channel until it disconnects.
    fn air_call_for_peer(&self, pool: &str, peer: &str, except: Option<u8>) -> String {
        let mut circuits: Vec<(String, String)> = Vec::new();
        for n in 1..=self.channels {
            if except == Some(n) || !self.pool_name(n).eq_ignore_ascii_case(pool) {
                continue;
            }
            if except
                .is_some_and(|ch| self.cfg.tnc.channel_port(ch) != self.cfg.tnc.channel_port(n))
            {
                continue;
            }
            let Some(other) = self.channel_other_call(n) else {
                continue;
            };
            circuits.push((self.effective_mycall(n), other));
        }
        let refs: Vec<(&str, &str)> = circuits
            .iter()
            .map(|(our, other)| (our.as_str(), other.as_str()))
            .collect();
        air_call_for_pair(pool, peer, &refs)
    }

    fn sync_channel_identities(&mut self) {
        self.restore_global_identities();
    }

    /// Idle / link-up: **one** global `I` on the radio, plus any user-dedicated
    /// channel overrides. Auto `-n` SSIDs stay internal / ephemeral only.
    fn restore_global_identities(&mut self) {
        for ch in 1..=self.channels {
            if !self.channel_connected(ch) {
                self.temp_slot_call[ch as usize] = None;
            }
        }
        // Single global identity on the modem. A user call on any channel,
        // including channel 1, stays as they set it.
        self.push_channel_identity(0);
    }

    /// Expand and TX the CT button file (classic `CTEXT.GPI`) on the channel.
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
        let mut ctx = self.macro_ctx();
        self.apply_channel_peer(&mut ctx, channel);
        let greeting = if let Some(text) = self
            .linked_call(channel)
            .as_deref()
            .and_then(gpi::personal_connect_text)
        {
            Some(gpi::expand_gpi_text(&text, &ctx))
        } else {
            [
                self.ctext_path_for(channel),
                self.resolve_ctext_path(),
                gpi::default_ctext_path(),
                crate::paths::data_file("CTEXT.GPI"),
                crate::paths::data_file("GTEXT.GPI"),
            ]
            .into_iter()
            .find_map(|p| gpi::read_file(&p))
            .map(|raw| gpi::expand_gpi_text(&raw, &ctx))
        };
        let waiting = self.linked_call(channel).is_some_and(|call| {
            let _ = mail::ensure_mail_dir(&mail::mail_dir());
            mail::waiting(&mail::mail_dir(), &call)
        });
        let Some(greeting) = greeting else {
            return waiting.then(|| "You have mail waiting, to read do //MAIL\n".into());
        };
        if waiting {
            Some(format!(
                "You have mail waiting, to read do //MAIL\n\n{greeting}"
            ))
        } else {
            Some(greeting)
        }
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
            ctx.peer_name = self
                .names
                .name_for(call, &self.names_qrg())
                .unwrap_or_default();
            ctx.name_unknown = ctx.peer_name.is_empty();
        } else {
            ctx.peer_call.clear();
            ctx.peer_name.clear();
            ctx.name_unknown = false;
        }
    }

    fn channel_disconnecting(&self, channel: u8) -> bool {
        self.disconnecting
            .get(usize::from(channel))
            .copied()
            .unwrap_or(false)
    }

    fn disconnect_channel(&mut self, channel: u8) {
        if !(1..=self.channels).contains(&channel) || self.channel_disconnecting(channel) {
            return;
        }
        if !self.channel_connected(channel) {
            self.clear_channel_link_state(channel);
            return;
        }
        self.stop_external(channel);
        self.cancel_mail_send(channel);
        if self.send_session_command(SessionCommand::Disconnect { channel }) {
            if self.channel_linked(channel) {
                self.disconnecting[usize::from(channel)] = true;
                self.status = format!("Disconnecting ch{channel}");
            } else {
                self.finish_disconnect(channel, false);
                self.status = format!("Connect cancelled ch{channel}");
            }
        }
    }

    /// UA or DM answered our DISC, or T1/N2 gave up.
    fn finish_disconnect(&mut self, channel: u8, play: bool) {
        self.stop_external(channel);
        self.cancel_mail_send(channel);
        let was = self.channel_linked(channel) || self.channel_disconnecting(channel);
        self.disconnecting[usize::from(channel)] = false;
        self.clear_channel_link_state(channel);
        self.drop_mail_for(channel);
        if self
            .autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel)
        {
            if let Some(live) = self.autobin.as_mut() {
                Self::discard_recv_temp(live);
            }
            self.autobin = None;
            self.session.bin_rx = false;
            self.session.bin_tx = false;
            if self
                .wprg_name
                .as_ref()
                .is_some_and(|(ch, _)| *ch == channel)
            {
                self.wprg_name = None;
            }
        }
        if self.bin_rx_swallow == Some(channel) {
            self.bin_rx_swallow = None;
        }
        if self
            .seven_tx
            .as_ref()
            .is_some_and(|tx| tx.channel == channel)
        {
            self.seven_tx = None;
        }
        if was && play {
            self.play_disconnect_bell(channel);
        }
        if was {
            if let Some(pad) = self.pads.get_mut(channel as usize) {
                pad.push(PadKind::System, "*** DISCONNECTED");
            }
            self.last_link_status = format!("*** DISCONNECTED ch{channel}");
        }
        if !self.any_channel_busy() {
            self.restore_global_identities();
        }
        self.status = format!("Disconnect ch{channel}");
    }

    fn clear_channel_link_state(&mut self, channel: u8) {
        self.close_journal(channel);
        let idx = channel as usize;
        self.disconnecting[idx] = false;
        // Leave notices need the peer call — run before the peer slot is cleared.
        self.drop_chat_on_disconnect(channel);
        if let Some(p) = self.channel_peer.get_mut(idx) {
            *p = None;
        }
        self.set_link_outgoing(channel, false);
        self.clear_lock(channel);
        if let Some(calls) = self.rx_calls.get_mut(idx) {
            *calls = None;
        }
        if let Some(c) = self.channel_connecting.get_mut(idx) {
            *c = None;
        }
        if let Some(via) = self.channel_via.get_mut(idx) {
            via.clear();
        }
        if let Some(port) = self.channel_link_port.get_mut(idx) {
            *port = None;
        }
        if let Some(down) = self.channel_down.get_mut(idx) {
            down.clear();
        }
        if let Some(wait) = self.channel_down_wait.get_mut(idx) {
            *wait = None;
        }
        if let Some(q) = self.qrt_disc.get_mut(idx) {
            *q = QrtDiscState::Idle;
        }
        self.seal_term(channel);
        if let Some(dec) = self.rx_ansi.get_mut(idx) {
            *dec = AnsiDecoder::new();
        }
        // Pool overflow (`NL1STK-1`) lasts only for this connection.
        self.clear_temp_identity(channel);
    }

    fn names_qrg(&self) -> String {
        self.session.qrg_text()
    }

    fn link_vias(&self, channel: u8) -> Vec<String> {
        self.channel_via
            .get(channel as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// After the AX.25 link is up: send the next node command.
    ///
    /// `greet` sends CTEXT. Only the station that was connected to greets.
    /// The station that placed the call does not. A queued node downlink
    /// (`C` / `C!`) still goes out first, in place of the greeting.
    fn finish_link_up(&mut self, channel: u8, greet: bool) {
        self.note_link_up(channel);
        if !self.send_next_downlink(channel) && greet {
            self.send_connect_text(channel);
        }
    }

    fn journal_qrg(&self) -> String {
        journal::clip_qrg(&self.session.qrg_text())
    }

    /// Start the journal clock and arm a `.GPW` if this peer has one.
    /// A connect we are not watching lights the channel label.
    fn note_link_up(&mut self, channel: u8) {
        self.mark_channel_attention(channel);
        self.open_journal(channel);
        if self
            .linked_call(channel)
            .is_some_and(|peer| gpw::find_file(&peer).is_some())
        {
            if let Some(slot) = self.gpw_armed.get_mut(channel as usize) {
                *slot = true;
            }
        }
    }

    fn open_journal(&mut self, channel: u8) {
        if !self.cfg.session.log {
            return;
        }
        let Some(peer) = self.linked_call(channel) else {
            return;
        };
        let idx = channel as usize;
        if self.qso_open.get(idx).and_then(|s| s.as_ref()).is_some() {
            return;
        }
        let mycall = self.effective_mycall(channel);
        let Some(slot) = self.qso_open.get_mut(idx) else {
            return;
        };
        *slot = Some(OpenQso {
            start: journal::clock(self.cfg.session.utc_offset_min),
            peer,
            mycall,
        });
    }

    fn close_journal(&mut self, channel: u8) {
        if let Some(slot) = self.gpw_armed.get_mut(channel as usize) {
            *slot = false;
        }
        let Some(slot) = self.qso_open.get_mut(channel as usize) else {
            return;
        };
        let Some(open) = slot.take() else {
            return;
        };
        if !self.cfg.session.log {
            return;
        }
        let end = journal::clock(self.cfg.session.utc_offset_min);
        let qrg = self.journal_qrg();
        let path = journal::gpl_path(&self.cfg.paths.log_dir, &open.mycall);
        match journal::append(&path, &open, end, &qrg) {
            Ok(()) => tracing::info!("journal {}", path.display()),
            Err(err) => tracing::warn!("journal {}: {err}", path.display()),
        }
    }

    fn note_sysop_command(&mut self, text: &str) {
        let ActivePane::Channel(channel) = self.active else {
            return;
        };
        let word = text.split_whitespace().next().unwrap_or("");
        if word.eq_ignore_ascii_case("SY") || word.eq_ignore_ascii_case("PW") {
            if let Some(slot) = self.gpw_armed.get_mut(channel as usize) {
                *slot = true;
            }
        }
    }

    /// Incoming line after `SY`/`PW`, or just after connect when a `.GPW` exists.
    fn consider_gpw(&mut self, channel: u8, line: &str) {
        let idx = channel as usize;
        if !self.gpw_armed.get(idx).copied().unwrap_or(false) {
            return;
        }
        let Some(peer) = self.linked_call(channel) else {
            return;
        };
        let Some(path) = gpw::find_file(&peer) else {
            return;
        };
        let Some(secret) = gpw::load(&path) else {
            return;
        };
        let kind = self.names.kind_of(&peer, &self.names_qrg());
        match gpw::reply(kind, &secret, line) {
            GpwReply::Answer(text) => {
                if let Some(slot) = self.gpw_armed.get_mut(idx) {
                    *slot = false;
                }
                self.last_gpw = text.clone();
                if let Some(slot) = self.compose_slot_idx() {
                    if let Some(c) = self.compose.get_mut(slot) {
                        c.input = text;
                        c.compose_caret_end = true;
                    }
                }
                self.status = format!("sysop reply for {peer}");
                tracing::info!("sysop reply ready for {peer}");
            }
            GpwReply::GridNotCalculated => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("GPW");
                self.status = format!("{name} is a DieBox grid — reply not calculated");
            }
            GpwReply::Ignore => {}
        }
    }

    /// Send the next `C` / `C!` line. Returns false when the queue is empty.
    fn send_next_downlink(&mut self, channel: u8) -> bool {
        let cmd = {
            let Some(q) = self.channel_down.get_mut(channel as usize) else {
                return false;
            };
            if q.is_empty() {
                return false;
            }
            q.remove(0)
        };
        let wait = downlink_target(&cmd);
        if let Some(slot) = self.channel_down_wait.get_mut(channel as usize) {
            *slot = Some(wait);
        }
        let prev = self.active;
        self.active = ActivePane::Channel(channel);
        self.echo_own_line(&cmd);
        self.send_operator_text(&cmd);
        self.active = prev;
        self.status = format!("autoroute ch{channel}: {cmd}");
        true
    }

    /// A node reply that names the call we just asked for unlocks the next command.
    fn note_autoroute_rx(&mut self, channel: u8, text: &str) {
        let wait = self
            .channel_down_wait
            .get(channel as usize)
            .and_then(|w| w.clone());
        let Some(wait) = wait.filter(|w| !w.is_empty()) else {
            return;
        };
        if !text
            .to_ascii_uppercase()
            .contains(&wait.to_ascii_uppercase())
        {
            return;
        }
        if let Some(slot) = self.channel_down_wait.get_mut(channel as usize) {
            *slot = None;
        }
        if !self.send_next_downlink(channel) {
            if let Some(pad) = self.pads.get_mut(channel as usize) {
                pad.push(PadKind::System, "*** autoroute complete");
            }
            // We placed this call. The far station sends its own CTEXT.
        }
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

    /// True only after the connection handshake completes.
    fn channel_linked(&self, n: u8) -> bool {
        self.channel_peer
            .get(n as usize)
            .and_then(|p| p.as_ref())
            .is_some()
    }

    fn set_link_outgoing(&mut self, channel: u8, outgoing: bool) {
        if let Some(flag) = self.channel_outgoing.get_mut(channel as usize) {
            *flag = outgoing;
        }
    }

    fn link_is_outgoing(&self, channel: u8) -> bool {
        self.channel_outgoing
            .get(channel as usize)
            .copied()
            .unwrap_or(false)
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
            format!("{n}: · · · · ·")
        }
    }

    /// Local TX echo into the RX pad(s). The line is always stored.
    /// Echo on a channel only hides or shows those rows. Hostmode also copies
    /// the line onto the Monitor when Monitor Echo is on. KISS Monitor lines
    /// come from [`TncEvent::KissTx`] after a successful write to the modem.
    fn echo_own_line(&mut self, line: &str) {
        let idx = match self.active {
            ActivePane::Channel(n) => n as usize,
            _ => 0,
        };
        // Enter finishes a row. It does not glue onto an open received line.
        // Colour bytes in the typed line paint that row; they are not shown.
        if idx != 0 {
            let ch = idx as u8;
            let shown = channel_store_text(line, self.session.umlaut_for(ch));
            self.seal_term(ch);
            self.accept_rx_text(ch, &shown, PadKind::Own);
            let wrote = self.rx_row.get(idx).copied().flatten().is_some();
            self.seal_term(ch);
            if !wrote {
                if let Some(pad) = self.pads.get_mut(idx) {
                    pad.push(PadKind::Own, "");
                }
            }
        }
        if self.software_ax25() || !self.session.echo_for(SLOT_MONITOR) {
            return;
        }
        self.push_mon_line_sgr(PadKind::Own, line);
    }

    /// Monitor listing after a KISS write or completed BayCom burst.
    fn note_kiss_tx(&mut self, port: u8, raw: &[u8]) {
        let Some(frame) = ax25::parse_frame(raw) else {
            self.push_mon_line(
                PadKind::System,
                super::session::kiss_monitor_fallback(port, raw),
            );
            return;
        };
        let info = if matches!(frame.kind, Ax25FrameType::Info | Ax25FrameType::Ui) {
            let body =
                ax25::bytes_to_monitor_body(&frame.info, self.session.umlaut_for(SLOT_MONITOR));
            if body.is_empty() {
                None
            } else {
                Some(body)
            }
        } else {
            None
        };
        self.push_kiss_mon_frame(
            PadKind::Own,
            &frame,
            info.as_deref(),
            info.as_deref().unwrap_or(""),
            port,
        );
    }

    fn software_ax25(&self) -> bool {
        self.cfg.tnc.runs_kiss_ax25()
    }

    /// KISS, BayCom and TFPCX use the application's monitor filter.
    fn uses_software_monitor(&self) -> bool {
        self.cfg.tnc.protocol_kind() != HostProtocol::Wa8ded
    }

    /// A channel has a link, so `M` without `C` keeps the monitor quiet.
    fn any_link_up(&self) -> bool {
        (1..=self.channels).any(|n| self.channel_linked(n))
    }

    /// KISS / Baycom: apply `M` here. Returns true when the line was `M`.
    fn take_monitor_command(&mut self, line: &str) -> bool {
        let Some(input) = MonitorFilter::parse_command(line) else {
            return false;
        };
        if !self.uses_software_monitor() {
            return false;
        }
        match input {
            MonitorInput::Query => {
                let label = self.monitor_filter.label();
                self.monitor_mode = label.clone();
                self.status = format!("Monitor {label}");
                self.log_event(format!("monitor mode {label}"));
            }
            MonitorInput::Set(args) => match MonitorFilter::apply(&self.monitor_filter, &args) {
                Ok(filter) => {
                    let label = filter.label();
                    self.monitor_filter = filter;
                    self.monitor_mode = label.clone();
                    self.monitor_follow = None;
                    self.status = format!("Monitor {label}");
                    self.log_event(format!("monitor mode → {label}"));
                }
                Err(err) => {
                    self.status = format!("M: {err}");
                }
            },
        }
        true
    }

    fn monitor_lists_frame(
        &self,
        kind: Ax25FrameType,
        src: &str,
        dest: &str,
        via: &[String],
    ) -> bool {
        if !self.uses_software_monitor() {
            return true;
        }
        let class = match kind {
            Ax25FrameType::Ui => FrameClass::Ui,
            Ax25FrameType::Info => FrameClass::Info,
            _ => FrameClass::Supervisory,
        };
        self.monitor_filter.allows(
            Some(class),
            &frame_calls(src, dest, via),
            self.any_link_up(),
        )
    }

    fn monitor_lists_text(&self, line: &str) -> bool {
        if !self.uses_software_monitor() {
            return true;
        }
        let connected = self.any_link_up();
        if let Some(heard) = parse_heard(line) {
            self.monitor_filter
                .allows(heard.class, &heard.calls, connected)
        } else {
            self.monitor_filter.allows(None, &[], connected)
        }
    }

    /// Hostmode monitor text (TFPCX) and plain KISS text lines.
    /// An `fm` header decides its info. When that info arrives as the next
    /// frame, it keeps the header's answer.
    fn host_monitor_allowed(&mut self, rows: &[String], fallback: &str) -> bool {
        if !self.uses_software_monitor() {
            return true;
        }
        let header = rows
            .iter()
            .map(String::as_str)
            .find(|row| is_fm_header(row))
            .or_else(|| is_fm_header(fallback).then_some(fallback));
        if let Some(header) = header {
            let show = self.monitor_lists_text(header);
            let info_here = rows
                .iter()
                .any(|row| !is_fm_header(row) && !row.trim().is_empty());
            self.monitor_follow = if info_here { None } else { Some(show) };
            return show;
        }
        let text = rows.first().map(String::as_str).unwrap_or(fallback);
        if parse_heard(text).is_some() {
            self.monitor_follow = None;
            return self.monitor_lists_text(text);
        }
        if let Some(show) = self.monitor_follow.take() {
            return show;
        }
        self.monitor_lists_text(text)
    }

    /// One monitor row on pad 0. With InsertMonLine, a blank line is placed
    /// before each new `fm …` packet header (not before payload rows).
    fn push_mon_line(&mut self, kind: PadKind, line: impl Into<String>) {
        let line = line.into();
        let blank = self.cfg.session.insert_mon_line
            && should_insert_mon_blank(
                &line,
                self.pads
                    .first()
                    .and_then(|p| p.lines.back().map(|l| l.text.as_str())),
            );
        let Some(pad) = self.pads.get_mut(0) else {
            return;
        };
        if blank {
            pad.push_monitor(kind, "");
        }
        pad.push_monitor(kind, line);
    }

    fn push_mon_line_sgr(&mut self, kind: PadKind, line: &str) {
        self.pads[0].push_monitor_sgr(kind, line, self.cfg.session.allow_color_ansi);
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
        heard_info: &str,
        heard_raw: Option<&[u8]>,
        port: u8,
    ) {
        if !self.monitor_lists_frame(frame_kind, src, dest, via) {
            return;
        }
        self.note_heard_on_monitor(src, dest, via, heard_info, heard_raw, Some(port));
        let hdr = stamp_monitor_fm(&ax25::tag_kiss_port(
            &ax25::monitor_ctl_line(src, dest, via, frame_kind, ns, nr, poll, command),
            port,
        ));
        self.push_mon_line(kind, hdr);
        if let Some(text) = info_text {
            let line = if text.starts_with("   ") {
                text.to_string()
            } else {
                format!("   {text}")
            };
            self.push_mon_line_sgr(kind, &line);
        }
    }

    /// Every monitor `fm` line updates MHeard from its source call.
    /// Our own transmissions are not stations we heard.
    /// `info` is the payload row after the header, or empty when there is none.
    /// `info_raw` is the unmodified AX.25 information field when available (Mic-E).
    fn note_heard_on_monitor(
        &mut self,
        src: &str,
        dest: &str,
        via: &[String],
        info: &str,
        info_raw: Option<&[u8]>,
        port: Option<u8>,
    ) {
        let src = src.trim();
        if src.is_empty()
            || (0..=self.channels).any(|ch| self.effective_mycall(ch).eq_ignore_ascii_case(src))
        {
            return;
        }
        let via = via
            .iter()
            .map(|hop| hop.trim_end_matches('*').trim().to_string())
            .filter(|hop| !hop.is_empty())
            .collect();
        let info_raw = info_raw
            .map(|b| b.to_vec())
            .unwrap_or_else(|| info.as_bytes().to_vec());
        self.mheard.hear(
            &Ax25UiFrame {
                dest: dest.to_string(),
                src: src.to_string(),
                via,
                info: info.to_string(),
                info_raw,
            },
            port,
        );
    }

    /// `fm` headers in `lines`, each with the following payload row as its text.
    fn note_heard_lines(&mut self, lines: &[String]) {
        for (i, line) in lines.iter().enumerate() {
            let Some((src, dest, via)) = heard_from_fm(line) else {
                continue;
            };
            let info = lines
                .get(i + 1)
                .map(String::as_str)
                .filter(|next| !is_fm_header(next))
                .unwrap_or("");
            self.note_heard_on_monitor(
                &src,
                &dest,
                &via,
                info,
                None,
                Some(self.heard_text_port_line(line)),
            );
        }
    }

    /// `//MHeard` port cell: `0 Serial-link` or `1 TCP-link`.
    fn mheard_port_label(&self, port: Option<u8>) -> String {
        let Some(port) = port else {
            return "-".into();
        };
        let tcp = match self.cfg.tnc.protocol_kind() {
            HostProtocol::Kiss => self.cfg.tnc.kiss_use_tcp(),
            HostProtocol::Tfpcx => self
                .cfg
                .tnc
                .links
                .iter()
                .any(|link| link.is_enabled() && link.uses_tcp() && link.ports.contains(&port)),
            HostProtocol::Wa8ded | HostProtocol::Baycom => false,
        };
        let kind = if tcp { "TCP-link" } else { "Serial-link" };
        format!("{port} {kind}")
    }

    /// Port column on MHeard when TFPCX is running more than one radio.
    fn mheard_shows_port(&self) -> bool {
        if self.cfg.tnc.protocol_kind() != HostProtocol::Tfpcx {
            return false;
        }
        let ports: usize = self
            .cfg
            .tnc
            .links
            .iter()
            .filter(|link| link.is_enabled())
            .map(|link| link.ports.len())
            .sum();
        ports > 1
    }

    /// Radio port for a hostmode payload that is not a KISS frame.
    fn heard_text_port(&self, _channel: u8) -> u8 {
        if self.cfg.tnc.protocol_kind() == HostProtocol::Tfpcx {
            return self.cfg.tnc.wa8ded_logical_port().unwrap_or(0);
        }
        0
    }

    /// `fm2 …` is port 2. A plain `fm` on a TFPCX WA8DED link uses that radio's port.
    fn heard_text_port_line(&self, line: &str) -> u8 {
        if let Some((head, _)) = ax25::split_monitor_fm(line) {
            let word = head.trim();
            if let Some(digits) = word.get(2..) {
                if !digits.is_empty() {
                    if let Ok(port) = digits.parse() {
                        return port;
                    }
                }
            }
        }
        self.heard_text_port(0)
    }

    fn push_kiss_mon_frame(
        &mut self,
        kind: PadKind,
        frame: &Ax25Frame,
        info_text: Option<&str>,
        heard_info: &str,
        port: u8,
    ) {
        let colored_info = info_text.map(|_| {
            ax25::bytes_to_monitor_body_sgr(&frame.info, self.session.umlaut_for(SLOT_MONITOR))
        });
        self.push_kiss_mon(
            kind,
            &frame.src,
            &frame.dest,
            &frame.via_labels(),
            frame.kind,
            frame.ns,
            frame.nr,
            frame.poll,
            frame.command,
            colored_info.as_deref(),
            heard_info,
            Some(frame.info.as_slice()),
            port,
        );
    }

    /// One tick when channel chat appears. Quiet while a multi-line GPI/CTEXT
    /// send is echoing, then flush a single pending play.
    fn note_channel_chat_sound(&mut self, channel: u8) {
        if !self.session.bell_for(channel) {
            return;
        }
        if self.newline_sound_quiet {
            self.newline_sound_pending = true;
            self.newline_sound_channel = channel;
            return;
        }
        self.play_newline_clicks(channel);
    }

    fn begin_newline_coalesce(&mut self) {
        self.newline_sound_quiet = true;
        self.newline_sound_pending = false;
    }

    fn end_newline_coalesce(&mut self) {
        self.newline_sound_quiet = false;
        if self.newline_sound_pending {
            self.newline_sound_pending = false;
            let ch = self.newline_sound_channel;
            self.play_newline_clicks(ch);
        }
    }

    /// `RX-Click` copies of `NEWLINE.WAV`, one second apart. `0` is silent.
    /// The ♪ bit for this channel silences it.
    fn play_newline_clicks(&self, channel: u8) {
        if !self.session.bell_for(channel) {
            return;
        }
        play_wav_times(
            &self.newline_wav,
            rx_click_plays(self.cfg.session.rx_click),
            Duration::from_secs(1),
            0.25,
        );
    }

    /// `Connect Bell` copies of `CONNECT.WAV`. The next starts when the
    /// previous one ends, so a replaced file keeps its own length. `0` is silent.
    /// The ♪ bit for this channel silences it.
    fn play_connect_bell(&self, channel: u8) {
        if !self.session.bell_for(channel) {
            return;
        }
        play_wav_times(
            &self.connect_wav,
            connect_bell_plays(self.cfg.session.connect_bell),
            Duration::ZERO,
            0.25,
        );
    }

    /// One `DISCONECT.WAV` while Connect Bell is on. Off skips it.
    /// The ♪ bit for this channel silences it.
    fn play_disconnect_bell(&self, channel: u8) {
        if !self.session.bell_for(channel) || !disconnect_plays(self.cfg.session.connect_bell) {
            return;
        }
        play_wav_at(&self.disconnect_wav, 0.25)
    }

    /// Terminal write. Bytes append to the open row. `\n` / `\r` end that row.
    /// Packet input is also completed for programs and chat bridges.
    fn accept_rx_text(&mut self, channel: u8, chunk: &str, kind: PadKind) {
        // BEL (Ctrl-G, 0x07) rings only on the station that received it,
        // and only while the session bell is on.
        if kind == PadKind::Incoming
            && channel >= 1
            && self.session.bell_for(channel)
            && chunk.as_bytes().contains(&0x07)
        {
            play_bell_wav(&self.bell_wav);
        }
        // Channel pads only — not Monitor (ch0), not system status lines.
        // One play per write burst; multi-line CTEXT is coalesced by the caller.
        if channel >= 1 && matches!(kind, PadKind::Own | PadKind::Incoming) && chunk_has_chat(chunk)
        {
            self.note_channel_chat_sound(channel);
        }
        let bytes = chunk.as_bytes();
        let mut i = 0;
        if self.rx_cr.get(channel as usize).copied().unwrap_or(false) {
            if let Some(flag) = self.rx_cr.get_mut(channel as usize) {
                *flag = false;
            }
            if bytes.first() == Some(&b'\n') {
                i = 1;
            }
            // CR already closed the row when we answered a `//` command.
            // Only finish when that row is still open.
            if self
                .rx_row
                .get(channel as usize)
                .copied()
                .flatten()
                .is_some()
            {
                self.finish_rx_line(channel, kind);
            }
        }
        while i < bytes.len() {
            if bytes[i] == b'\n' {
                self.finish_rx_line(channel, kind);
                i += 1;
            } else if bytes[i] == b'\r' {
                if bytes.get(i + 1) == Some(&b'\n') {
                    self.finish_rx_line(channel, kind);
                    i += 2;
                } else if i + 1 == bytes.len() {
                    if let Some(flag) = self.rx_cr.get_mut(channel as usize) {
                        *flag = true;
                    }
                    i += 1;
                } else {
                    self.finish_rx_line(channel, kind);
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
        // Show the `//` line, close it, then answer. Do not answer before paint.
        if kind == PadKind::Incoming && channel >= 1 {
            self.finish_pending_remote_command(channel);
            self.finish_packet_input_line(channel);
        }
        // A packet that ends on CR keeps the row open (the next byte may be LF).
        // A sealed line is already complete, so show the plaintext now.
        if kind == PadKind::Incoming {
            self.reveal_locked_row(channel);
            self.note_channel_unread(channel, chunk);
        }
    }

    /// Light the channel label when text arrives on a channel that is not open.
    fn note_channel_unread(&mut self, channel: u8, chunk: &str) {
        if !(1..=self.channels).contains(&channel) {
            return;
        }
        if !chunk.chars().any(|c| !c.is_control() && !c.is_whitespace()) {
            return;
        }
        self.mark_channel_attention(channel);
    }

    /// White/red label. Skipped when that channel is the one on screen.
    fn mark_channel_attention(&mut self, channel: u8) {
        if !(1..=self.channels).contains(&channel) {
            return;
        }
        if matches!(self.active, ActivePane::Channel(n) if n == channel) {
            return;
        }
        if let Some(flag) = self.channel_unread.get_mut(channel as usize) {
            *flag = true;
        }
    }

    fn clear_channel_attention(&mut self, channel: u8) {
        if let Some(flag) = self.channel_unread.get_mut(channel as usize) {
            *flag = false;
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

    fn finish_rx_line(&mut self, channel: u8, kind: PadKind) {
        self.term_break(channel, kind);
    }

    fn term_break(&mut self, channel: u8, kind: PadKind) {
        let idx = channel as usize;
        let open = self.rx_row.get(idx).copied().flatten();
        if let Some(seq) = open {
            let mut text = self
                .pads
                .get(idx)
                .and_then(|pad| pad.text_if_seq(seq))
                .unwrap_or_default();
            if let Some(slot) = self.rx_row.get_mut(idx) {
                *slot = None;
            }
            if kind == PadKind::Incoming && channel >= 1 {
                if let Some(plain) = self.unlock_incoming(channel, &text) {
                    if let Some(pad) = self.pads.get_mut(idx) {
                        pad.set_text_if_seq(seq, &plain);
                    }
                    text = plain;
                }
            }
            let cut =
                parse_line(&text).is_some_and(|(action, _)| action == RemoteAction::CutTransfer);
            if !self.seven_capturing(channel) || cut {
                self.maybe_remote_line(channel, &text, kind == PadKind::Incoming);
            }
            if kind == PadKind::Incoming && channel >= 1 && !text.is_empty() {
                self.note_autoroute_rx(channel, &text);
                self.consider_gpw(channel, &text);
                self.note_mail_line(channel, &text);
            }
        } else if let Some(pad) = self.pads.get_mut(idx) {
            pad.push(kind, "");
        }
    }

    /// Packet ended on CR with a `//` command still on the open row: close that
    /// row so the operator sees the command, then answer on the next line.
    /// Keep `rx_cr` set so a following LF in the next packet is not a blank row.
    fn finish_pending_remote_command(&mut self, channel: u8) {
        if !self.rx_cr.get(channel as usize).copied().unwrap_or(false) {
            return;
        }
        let idx = channel as usize;
        let text = self
            .rx_row
            .get(idx)
            .copied()
            .flatten()
            .and_then(|seq| self.pads.get(idx).and_then(|pad| pad.text_if_seq(seq)))
            .unwrap_or_default();
        if slash_verb(&text).is_none() {
            return;
        }
        if let Some(flag) = self.rx_cr.get_mut(idx) {
            *flag = false;
        }
        self.finish_rx_line(channel, PadKind::Incoming);
        if let Some(flag) = self.rx_cr.get_mut(idx) {
            *flag = true;
        }
    }

    /// Programs and chat bridges consume packets without waiting for Enter.
    fn finish_packet_input_line(&mut self, channel: u8) {
        if channel < 1 {
            return;
        }
        let idx = channel as usize;
        let Some(seq) = self.rx_row.get(idx).copied().flatten() else {
            return;
        };
        if !self.external_busy(channel) {
            let in_chat = self.chat_target(channel).is_some()
                || (1..=self.channels).any(|ch| self.chat_target(ch) == Some(channel));
            if !in_chat
                || self.channel_transfer_busy(channel)
                || self.mail_send_open(channel)
                || self.user_write.get(idx).is_some_and(Option::is_some)
            {
                return;
            }
            // Remote commands may span packets, even in chat mode.
            if self
                .pads
                .get(idx)
                .and_then(|pad| pad.text_if_seq(seq))
                .is_some_and(|text| text.starts_with("//"))
            {
                return;
            }
        }
        let ended_cr = self.rx_cr.get(idx).copied().unwrap_or(false);
        if ended_cr {
            if let Some(flag) = self.rx_cr.get_mut(idx) {
                *flag = false;
            }
        }
        self.finish_rx_line(channel, PadKind::Incoming);
        if ended_cr {
            if let Some(flag) = self.rx_cr.get_mut(idx) {
                *flag = true;
            }
        }
    }

    fn maybe_remote_line(&mut self, channel: u8, line: &str, incoming: bool) {
        if incoming && self.mail_send_open(channel) && self.take_mail_line(channel, line) {
            return;
        }
        if channel >= 1
            && self
                .user_write
                .get(channel as usize)
                .and_then(|w| w.as_ref())
                .is_some()
        {
            self.capture_write_line(channel, line);
            return;
        }
        if channel < 1 || !incoming {
            return;
        }
        // Remote is per-channel. Off answers //RUN and does not start it.
        let remote_on = self.session.remote_for(channel);
        if !remote_on && self.another_run(line) {
            self.refuse_remote_run(channel);
            return;
        }
        // A script that is still running owns ordinary text. A `//` line is
        // still a command. A second //RUN is refused. //Quit can still drop the link.
        if self.external_busy(channel) {
            if self.another_run(line) {
                self.warn_running(channel);
                return;
            }
            if let Some((action, args)) = parse_line(line) {
                if action == RemoteAction::Bell || remote_on {
                    self.handle_remote_on_channel(channel, action, &args);
                }
                return;
            }
            if slash_verb(line).is_some() {
                if remote_on {
                    self.resolve_file_command(channel, line);
                }
                return;
            }
            self.feed_external(channel, line);
            return;
        }
        let Some((action, args)) = parse_line(line) else {
            if remote_on {
                self.resolve_file_command(channel, line);
            }
            // Ordinary chat on a channel that is in (or hosting) chat mode.
            if incoming && !line.trim().is_empty() {
                self.relay_chat_from(channel, line, true);
            }
            return;
        };
        // //Bell / //B / //Ring is a page, not general remote control.
        // It runs on the station that received the line and always
        // plays BELL.WAV (the ♪ mute does not silence this page).
        if action == RemoteAction::Bell {
            self.handle_remote_on_channel(channel, action, &args);
            return;
        }
        if remote_on {
            self.handle_remote_on_channel(channel, action, &args);
        }
    }

    fn seven_capturing(&self, channel: u8) -> bool {
        self.seven
            .get(channel as usize)
            .is_some_and(SevenPlusAutosave::is_capturing)
    }

    /// Toggle or set 7+ for `pane` (Monitor broadcasts to every channel).
    fn set_seven_plus_pane(&mut self, pane: ActivePane, on: Option<bool>) {
        let on = match on {
            Some(v) => {
                if matches!(pane, ActivePane::Monitor) {
                    self.session.set_seven_plus_for(SLOT_MONITOR, v);
                } else if let ActivePane::Channel(ch) = pane {
                    self.session.set_seven_plus_for(ch, v);
                } else {
                    self.session.set_seven_plus_for(slot_for(pane), v);
                }
                v
            }
            None => self.session.toggle_seven_plus(pane),
        };
        for (idx, sniff) in self.seven.iter_mut().enumerate() {
            sniff.set_enabled(self.session.seven_plus_for(idx as u8));
        }
        let dir = self.cfg.paths.user_dir.display().to_string();
        let label = match pane {
            ActivePane::Monitor => "all channels".into(),
            ActivePane::Channel(ch) => format!("ch{ch}"),
            _ => "channel".into(),
        };
        self.status = if on {
            format!("7+ ON ({label}) — saving to {dir}")
        } else {
            format!("7+ OFF ({label})")
        };
        self.log_event(self.status.clone());
        self.persist_session_chrome();
    }

    /// `//7DECODE <file>` — join parts already in USER.
    fn remote_seven_decode(&mut self, args: &str) {
        let dir = self.cfg.paths.user_dir.clone();
        let _ = sevenplus::ensure_user_dir(&dir);
        let name = args.trim();
        if name.is_empty() {
            self.status = "7DECODE needs a file name".into();
            self.send_report("7DECODE needs a file name");
            return;
        }
        self.apply_seven_assemble(sevenplus::decode_user_parts(&dir, name), true);
    }

    fn apply_seven_assemble(
        &mut self,
        result: Result<sevenplus::AssembleOutcome, String>,
        report_ignored: bool,
    ) {
        match result {
            Ok(sevenplus::AssembleOutcome::Joined(joined)) => {
                self.status = format!("7+ decoded {}", joined.name);
                self.log_event(format!(
                    "7+ decoded {} (removed {} part(s))",
                    joined.path.display(),
                    joined.deleted_parts.len()
                ));
                self.send_report(&format!("7+ decoded {}", joined.name));
            }
            Ok(sevenplus::AssembleOutcome::Incomplete { missing }) => {
                for part_name in &missing {
                    self.send_report(&format!("missing {part_name}"));
                }
                if let Some(first) = missing.first() {
                    self.status = if missing.len() == 1 {
                        format!("7+ missing {first}")
                    } else {
                        format!("7+ missing {} (+{})", first, missing.len() - 1)
                    };
                    self.log_event(format!("7+ missing {}", missing.join(", ")));
                } else {
                    self.status = "7+ waiting for more parts".into();
                }
            }
            Ok(sevenplus::AssembleOutcome::Ignored) => {
                if report_ignored {
                    self.status = "7+ decode: not a 7Plus part".into();
                    self.send_report("not a 7Plus part");
                }
            }
            Err(err) => {
                self.status = format!("7+ decode: {err}");
                self.log_event(self.status.clone());
                self.send_report(&err);
            }
        }
    }

    /// Split one payload on CR/LF and hand each finished row to the sniffer.
    /// Bytes stay as Latin-1 so the 7Plus 8-bit check (`0xB0 0xB1`) survives.
    /// A trailing CR completes the row (DOS); otherwise `stop_7+.` never saved.
    fn note_seven_payload(&mut self, channel: u8, payload: &[u8]) {
        if channel == 0 {
            return;
        }
        let idx = channel as usize;
        let Some(buf) = self.seven_pending.get_mut(idx) else {
            return;
        };
        let Some(cr_hold) = self.seven_cr.get_mut(idx) else {
            return;
        };
        let lines = take_rx_lines_bytes(buf, payload, cr_hold);
        for line in lines {
            self.feed_sevenplus(channel, &line);
        }
    }

    fn feed_sevenplus(&mut self, channel: u8, line: &str) {
        if channel == 0 {
            return;
        }
        if parse_line(line).is_some_and(|(action, _)| action == RemoteAction::CutTransfer)
            && self.seven_capturing(channel)
        {
            self.cut_transfer(channel);
            return;
        }
        if !self.session.seven_plus_for(channel) {
            if sevenplus::is_opening_line(line) {
                self.status = format!("7+ part seen — ch{channel} switch is OFF");
                self.log_event(self.status.clone());
            }
            return;
        }
        if sevenplus::is_opening_line(line) {
            let dir = self.cfg.paths.user_dir.display().to_string();
            self.status = format!("7+ capturing ch{channel} → {dir}");
        }
        let idx = channel as usize;
        let part = {
            let Some(sniff) = self.seven.get_mut(idx) else {
                return;
            };
            if !sniff.enabled {
                sniff.set_enabled(true);
            }
            sniff.on_line(line)
        };
        let Some(part) = part else {
            return;
        };
        let dir = self.cfg.paths.user_dir.clone();
        let fallback = format!("ch{channel}");
        match SevenPlusAutosave::write_part(&dir, &part, &fallback) {
            Ok(path) => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("part");
                self.status = format!("7+ saved {name}");
                self.log_event(format!("7+ saved {}", path.display()));
                self.apply_seven_assemble(sevenplus::try_decode_complete_set(&dir, &path), false);
            }
            Err(err) => {
                self.status = format!("7+ save: {err}");
                self.log_event(self.status.clone());
            }
        }
    }

    fn autobin_pending(&self, channel: u8) -> bool {
        self.autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.state.pending_offer().is_some())
    }

    fn autobin_receiving(&self, channel: u8) -> bool {
        self.autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.state.recv_progress().is_some())
    }

    /// File bytes stay off the channel. `#BIN#`, `#OK#`, `#NO#`, `#ABORT#`,
    /// and a `<GP>:` tail still belong there.
    /// `None` means this frame has nothing left to paint.
    fn autobin_channel_text(&mut self, channel: u8, info: &[u8]) -> Option<Vec<u8>> {
        if self.bin_rx_swallow == Some(channel) {
            self.bin_rx_swallow = None;
            self.session.bin_rx = false;
            return gp_notice_from(info);
        }
        if autobin::is_abort_payload(info)
            && self
                .autobin
                .as_ref()
                .is_some_and(|live| live.channel == channel && live.state.pending_offer().is_some())
        {
            self.autobin = None;
            self.session.bin_rx = false;
            self.show_proto_line(channel, "#ABORT#", PadKind::Incoming);
            self.status = "BIN-RX aborted".into();
            return None;
        }
        if self.autobin_receiving(channel) && autobin::is_abort_payload(info) {
            if let Some(mut live) = self.autobin.take() {
                Self::discard_recv_temp(&mut live);
            }
            self.session.bin_rx = false;
            self.show_proto_line(channel, "#ABORT#", PadKind::Incoming);
            self.status = "BIN-RX aborted".into();
            return None;
        }
        if self.autobin_receiving(channel) {
            let (n, done, io_err) = {
                let live = self.autobin.as_mut().expect("receiving");
                let (n, done) = live.state.on_binary(info);
                let io_err = if n > 0 {
                    live.recv_file
                        .as_mut()
                        .and_then(|file| std::io::Write::write_all(file, &info[..n]).err())
                        .map(|err| err.to_string())
                } else {
                    None
                };
                (n, done, io_err)
            };
            if let Some(err) = io_err {
                self.cancel_autobin();
                self.status = format!("BIN-RX: {err}");
                return None;
            }
            if let Some(done) = done {
                self.commit_autobin_file(done);
            }
            let tail = info[n..].to_vec();
            return if tail.is_empty() { None } else { Some(tail) };
        }
        if self.autobin_tx_active(channel) {
            return self.note_bin_tx_reply(channel, info);
        }
        if self.autobin.is_some() {
            return strip_bin_headers(info);
        }
        let text = decode_cp437(info);
        for line in text.split(['\r', '\n']) {
            if !line.starts_with("#BIN#") && !line.starts_with("#BIN_V2#") {
                continue;
            }
            let mut state = AutoBinState::Idle;
            let _ = state.on_text_line(line, false);
            let Some(label) = state.pending_offer().map(autobin_offer_label) else {
                self.status = "BIN-RX: could not read the #BIN# header".into();
                self.show_proto_line(channel, line, PadKind::Incoming);
                return None;
            };
            self.autobin = Some(LiveAutoBin {
                channel,
                state,
                started: Instant::now(),
                tx_id: None,
                recv_file: None,
                recv_part: None,
                recv_final: None,
            });
            self.session.bin_rx = true;
            self.active = ActivePane::Channel(channel);
            self.status = format!("BIN-RX: {label} — SAVE");
            self.log_event(self.status.clone());
            self.show_proto_line(channel, line, PadKind::Incoming);
            // `//WPrg` (or 7+ AutoSave) answers the offer without a SAVE click.
            let auto = self
                .wprg_name
                .as_ref()
                .is_some_and(|(ch, _)| *ch == channel)
                || self.session.seven_plus_for(channel);
            if auto {
                let _ = self.accept_autobin(channel);
            }
            return None;
        }
        Some(info.to_vec())
    }

    /// SAVE on the channel that holds the offer. Sends `#OK#` and starts the bar.
    fn accept_autobin(&mut self, channel: u8) -> bool {
        let pending = self
            .autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.state.pending_offer().is_some());
        if !pending {
            return false;
        }
        let wprg = match self.wprg_name.as_ref() {
            Some((ch, name)) if *ch == channel => Some(name.clone()),
            _ => None,
        };
        if wprg.is_some() {
            self.wprg_name = None;
        }
        let offer_name = self
            .autobin
            .as_ref()
            .and_then(|live| live.state.pending_offer())
            .and_then(|offer| offer.filename.clone())
            .unwrap_or_default();
        let raw_name = wprg.clone().unwrap_or(offer_name);
        let (_safe, final_path, part_path) = if wprg.is_some() {
            self.autobin_paths_in(&raw_name, &self.cfg.paths.user_dir)
        } else {
            self.autobin_paths(&raw_name)
        };
        if let Some(parent) = part_path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                self.status = format!("BIN-RX: {err}");
                return true;
            }
        }
        let file = match std::fs::File::create(&part_path) {
            Ok(file) => file,
            Err(err) => {
                self.status = format!("BIN-RX: {err}");
                return true;
            }
        };
        let live = self.autobin.as_mut().expect("pending");
        if !live.state.begin_receive_stream() {
            drop(file);
            let _ = std::fs::remove_file(&part_path);
            return false;
        }
        live.started = Instant::now();
        live.recv_file = Some(file);
        live.recv_part = Some(part_path);
        live.recv_final = Some(final_path);
        let ack = autobin::format_ok();
        if !self.send_channel_packet(channel, ack.as_bytes()) {
            if let Some(live) = self.autobin.as_mut() {
                Self::discard_recv_temp(live);
            }
            self.autobin = None;
            self.session.bin_rx = false;
            self.status = format!("BIN-RX: channel {channel} has no link");
            return true;
        }
        self.show_proto_line(channel, "#OK#", PadKind::Own);
        self.session.bin_rx = true;
        self.status = format!("BIN-RX ch{channel}");
        true
    }

    fn cancel_autobin(&mut self) {
        let Some(mut live) = self.autobin.take() else {
            return;
        };
        let receiving = live.state.recv_progress().is_some();
        let pending = live.state.pending_offer().is_some();
        if !receiving && !pending {
            self.autobin = Some(live);
            return;
        }
        Self::discard_recv_temp(&mut live);
        let channel = live.channel;
        let wire = if receiving { "#ABORT#\r" } else { "#NO#\r" };
        let shown = if receiving { "#ABORT#" } else { "#NO#" };
        let _ = self.send_channel_packet(channel, wire.as_bytes());
        self.show_proto_line(channel, shown, PadKind::Own);
        self.session.bin_rx = false;
        if receiving {
            self.bin_rx_swallow = Some(channel);
        }
        self.status = if receiving {
            "BIN-RX aborted".into()
        } else {
            "BIN-RX declined".into()
        };
    }

    fn autobin_tx_active(&self, channel: u8) -> bool {
        self.autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.state.tx_active())
    }

    fn note_bin_tx_reply(&mut self, channel: u8, info: &[u8]) -> Option<Vec<u8>> {
        let text = decode_cp437(info);
        let mut reply = TxReply::Ignore;
        let mut shown = String::new();
        for line in text.split(['\r', '\n']) {
            if line.is_empty() {
                continue;
            }
            let live = self.autobin.as_mut().expect("tx");
            reply = live.state.on_tx_reply(line);
            if !matches!(reply, TxReply::Ignore) {
                shown = line.to_string();
                break;
            }
        }
        match reply {
            TxReply::Start => {
                if let Some(live) = self.autobin.as_mut() {
                    live.started = Instant::now();
                    live.tx_id = None;
                }
                if !shown.is_empty() {
                    self.show_proto_line(channel, &shown, PadKind::Incoming);
                }
                self.pump_bin_tx(channel);
                None
            }
            TxReply::Rejected(reason) => {
                self.autobin = None;
                self.session.bin_tx = false;
                if !shown.is_empty() {
                    self.show_proto_line(channel, &shown, PadKind::Incoming);
                }
                self.status = if shown.contains("#ABORT#") {
                    "BIN-TX aborted".into()
                } else if reason.is_empty() {
                    "BIN-TX declined".into()
                } else {
                    format!("BIN-TX declined: {reason}")
                };
                None
            }
            TxReply::Ignore => Some(info.to_vec()),
        }
    }

    fn note_bin_tx_ack(&mut self, channel: u8, id: u64) {
        let acked = self
            .autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.tx_id == Some(id));
        if acked {
            if let Some(live) = self.autobin.as_mut() {
                live.tx_id = None;
            }
            self.pump_bin_tx(channel);
            return;
        }
        self.note_seven_tx_ack(channel, id);
    }

    fn pump_bin_tx(&mut self, channel: u8) {
        if self
            .autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.tx_id.is_some())
        {
            return;
        }
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        let chunk = {
            let Some(live) = self.autobin.as_mut() else {
                return;
            };
            if live.channel != channel {
                return;
            }
            live.state.next_chunk(max)
        };
        let Some(chunk) = chunk else {
            self.finish_bin_tx(channel);
            return;
        };
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if !self.send_session_command(SessionCommand::Send {
            channel,
            data: chunk,
            id: Some(id),
        }) {
            self.autobin = None;
            self.session.bin_tx = false;
            self.status = format!("BIN-TX: channel {channel} has no link");
            return;
        }
        if let Some(live) = self.autobin.as_mut() {
            live.tx_id = Some(id);
        }
    }

    fn finish_bin_tx(&mut self, channel: u8) {
        let Some(live) = self.autobin.take() else {
            return;
        };
        let (got, crc) = live.state.tx_summary().unwrap_or((0, 0));
        let elapsed = live.started.elapsed();
        let secs = elapsed.as_secs().max(1);
        let baud = (got.saturating_mul(8) / secs) as u32;
        let e = elapsed.as_secs();
        let tijd = format!("{:02}:{:02}:{:02}", e / 3600, (e / 60) % 60, e % 60);
        let line1 = format!("<GPNG>: Binare file transfer OK, CRC={crc}");
        let line2 = format!("      Bytes={got}, Tijd={tijd}, Baud={baud}");
        self.session.bin_tx = false;
        let wire = format!("{line1}\r{line2}\r");
        let _ = self.send_channel_packet(channel, wire.as_bytes());
        self.accept_rx_text(channel, &format!("{line1}\n{line2}\n"), PadKind::Own);
        self.status = format!("BIN-TX sent {got} bytes");
    }

    fn start_bin_tx(&mut self) {
        let Some(channel) = self.active_channel() else {
            self.status = "BIN-TX — select a channel".into();
            return;
        };
        if !self.channel_linked(channel) {
            self.status = "BIN-TX — connect the channel first".into();
            return;
        }
        if self.autobin.is_some() || self.seven_tx.is_some() {
            self.status = "BIN-TX — a transfer is already running".into();
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title("BIN-TX");
        dialog = dialog.set_directory(&self.cfg.paths.save_dir);
        let Some(path) = dialog.pick_file() else {
            self.status = "BIN-TX cancelled".into();
            return;
        };
        let data = match std::fs::read(&path) {
            Ok(data) if !data.is_empty() => data,
            Ok(_) => {
                self.status = "BIN-TX — file is empty".into();
                return;
            }
            Err(err) => {
                self.status = format!("BIN-TX: {err}");
                return;
            }
        };
        let ftime = file_ftime(&path);
        let mut shown = crate::paths::display_native(&path);
        let crc = crate::transfer::crc::crc_gp(&data);
        let overhead = format!("#BIN#{}#|{crc}#$XXXXXXXX#\r", data.len()).len();
        if overhead + shown.len() > 240 {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                shown = name.to_string();
            }
        }
        self.start_bin_tx_data(channel, &shown, data, ftime);
    }

    /// `//RPrg` — AutoBin-TX a file from USER.
    fn remote_rprg(&mut self, channel: u8, args: &str) {
        let name = args.trim();
        if name.is_empty() {
            self.status = "RPrg needs a file name".into();
            self.send_report("RPrg needs a file name");
            return;
        }
        if self.autobin.is_some() || self.seven_tx.is_some() {
            self.status = "BIN-TX — a transfer is already running".into();
            self.send_report("a file transfer is already running");
            return;
        }
        let dir = self.cfg.paths.user_dir.clone();
        let path = match sevenplus::resolve_user_file(&dir, name) {
            Ok(path) => path,
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
                return;
            }
        };
        let data = match std::fs::read(&path) {
            Ok(data) if !data.is_empty() => data,
            Ok(_) => {
                self.status = "USER file is empty".into();
                self.send_report("USER file is empty");
                return;
            }
            Err(err) => {
                let msg = format!("BIN-TX: {err}");
                self.status = msg.clone();
                self.send_report(&msg);
                return;
            }
        };
        let shown = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(name)
            .to_string();
        self.start_bin_tx_data(channel, &shown, data, file_ftime(&path));
    }

    /// `//WPrg` — next AutoBin on this channel is stored in USER as `name`.
    fn remote_wprg(&mut self, channel: u8, args: &str) {
        let name = match userdir::safe_name(args) {
            Ok(name) => name,
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
                return;
            }
        };
        if userdir::is_mail_name(&name) {
            self.status = "mail is not in USER".into();
            self.send_report("mail is not in USER");
            return;
        }
        let _ = sevenplus::ensure_user_dir(&self.cfg.paths.user_dir);
        self.wprg_name = Some((channel, name.clone()));
        self.session.bin_rx = true;
        self.status = format!("WPrg ready for {name}");
        self.send_report(&format!("WPrg ready for {name}"));
    }

    fn start_bin_tx_data(&mut self, channel: u8, shown: &str, data: Vec<u8>, ftime: u32) {
        let mut state = AutoBinState::Idle;
        let header = state.begin_gp_send(shown, data, ftime);
        self.autobin = Some(LiveAutoBin {
            channel,
            state,
            started: Instant::now(),
            tx_id: None,
            recv_file: None,
            recv_part: None,
            recv_final: None,
        });
        self.session.bin_tx = true;
        if !self.send_channel_packet(channel, header.as_bytes()) {
            self.autobin = None;
            self.session.bin_tx = false;
            self.status = format!("BIN-TX: channel {channel} has no link");
            return;
        }
        self.show_proto_line(channel, header.trim_end_matches(['\r', '\n']), PadKind::Own);
        self.status = format!("BIN-TX: {shown}");
    }

    /// `//CUT` on this channel. The receiver sends the abort token.
    /// The sender stops the file. Either way the channel is back in chat.
    fn cut_transfer(&mut self, channel: u8) -> bool {
        let bin_tx = self
            .autobin
            .as_ref()
            .is_some_and(|live| live.channel == channel && live.state.tx_active());
        if bin_tx {
            self.stop_bin_sender();
            self.transfer_cut = true;
            return true;
        }
        let bin_rx = self.autobin.as_ref().is_some_and(|live| {
            live.channel == channel
                && (live.state.recv_progress().is_some() || live.state.pending_offer().is_some())
        });
        if bin_rx {
            self.cancel_autobin();
            self.transfer_cut = true;
            return true;
        }
        if self
            .seven_tx
            .as_ref()
            .is_some_and(|tx| tx.channel == channel)
        {
            self.seven_tx = None;
            let _ = self.send_channel_packet(channel, b"#ABORT#\r");
            self.show_proto_line(channel, "#ABORT#", PadKind::Own);
            self.status = "7+-TX aborted".into();
            self.transfer_cut = true;
            return true;
        }
        if self.seven_capturing(channel) {
            self.cut_seven(channel);
            self.transfer_cut = true;
            return true;
        }
        false
    }

    /// BIN-TX: stop handing out file bytes. Once bytes are on the air, tell
    /// the receiver with `#ABORT#` so the next line is chat.
    fn stop_bin_sender(&mut self) {
        let Some(live) = self.autobin.take() else {
            self.session.bin_tx = false;
            return;
        };
        if !live.state.tx_active() {
            self.autobin = Some(live);
            return;
        }
        let channel = live.channel;
        self.session.bin_tx = false;
        // The offer is already on the other station. `#ABORT#` lets that
        // side leave the file. We send no further file bytes.
        let abort = autobin::format_abort();
        let _ = self.send_channel_packet(channel, abort.as_bytes());
        self.show_proto_line(channel, "#ABORT#", PadKind::Own);
        self.status = "BIN-TX stopped".into();
    }

    /// 7+ capture: drop the part and tell the sender to stop.
    fn cut_seven(&mut self, channel: u8) {
        let dropped = self
            .seven
            .get_mut(channel as usize)
            .is_some_and(|sniff| sniff.abort_capture());
        if !dropped {
            return;
        }
        let _ = self.send_channel_packet(channel, b"#ABORT#\r");
        self.show_proto_line(channel, "#ABORT#", PadKind::Own);
        self.status = "7+ aborted".into();
    }

    fn cancel_bin_tx(&mut self) {
        let Some(live) = self.autobin.take() else {
            self.session.bin_tx = false;
            return;
        };
        if !live.state.tx_active() {
            self.autobin = Some(live);
            return;
        }
        let channel = live.channel;
        let abort = autobin::format_abort();
        let _ = self.send_channel_packet(channel, abort.as_bytes());
        self.show_proto_line(channel, "#ABORT#", PadKind::Own);
        self.session.bin_tx = false;
        self.status = "BIN-TX aborted".into();
    }

    /// The part file is complete. Rename it onto the real name.
    fn commit_autobin_file(&mut self, done: BinReceiveDone) {
        let Some(mut live) = self.autobin.take() else {
            return;
        };
        if let Some(mut file) = live.recv_file.take() {
            let _ = std::io::Write::flush(&mut file);
        }
        let part = live.recv_part.take();
        let dest = live.recv_final.take();
        self.session.bin_rx = false;
        self.status = match (part, dest) {
            (Some(part), Some(dest)) => match replace_part_file(&part, &dest) {
                Ok(()) => {
                    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                    format!("BIN-RX saved {} ({bytes} bytes)", dest.display())
                }
                Err(err) => format!("BIN-RX save: {err}"),
            },
            _ => {
                let safe = autobin_safe_name(done.filename.as_deref().unwrap_or(""));
                let path = self.cfg.paths.save_dir.join(&safe);
                match std::fs::create_dir_all(&self.cfg.paths.save_dir)
                    .and_then(|_| std::fs::write(&path, &done.data))
                {
                    Ok(()) => format!(
                        "BIN-RX saved {} ({} bytes)",
                        path.display(),
                        done.data.len()
                    ),
                    Err(err) => format!("BIN-RX save: {err}"),
                }
            }
        };
        self.log_event(self.status.clone());
    }

    fn autobin_paths(&self, raw: &str) -> (String, PathBuf, PathBuf) {
        self.autobin_paths_in(raw, &self.cfg.paths.save_dir)
    }

    fn autobin_paths_in(&self, raw: &str, dir: &Path) -> (String, PathBuf, PathBuf) {
        let safe = autobin_safe_name(raw);
        let final_path = dir.join(&safe);
        let part = dir.join(format!(".{safe}.part"));
        (safe, final_path, part)
    }

    fn discard_recv_temp(live: &mut LiveAutoBin) {
        live.recv_file.take();
        if let Some(path) = live.recv_part.take() {
            let _ = std::fs::remove_file(path);
        }
        live.recv_final.take();
    }

    /// One protocol line on the channel, closed, the way DOS shows `#BIN#` / `#OK#`.
    fn show_proto_line(&mut self, channel: u8, line: &str, kind: PadKind) {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() || channel == 0 {
            return;
        }
        self.seal_term(channel);
        self.accept_rx_text(channel, &format!("{line}\n"), kind);
    }

    fn send_channel_packet(&mut self, n: u8, info: &[u8]) -> bool {
        if !self.channel_linked(n) || self.channel_disconnecting(n) {
            self.status = format!("ch{n}: not connected");
            return false;
        }
        self.send_session_command(SessionCommand::Send {
            channel: n,
            data: info.to_vec(),
            id: None,
        })
    }

    /// One already-packed chunk → AX.25 UI or I-frame. Bytes are sent as-is.
    fn send_kiss_text(&mut self, line: &str) {
        self.send_kiss_bytes(line.as_bytes());
    }

    fn send_kiss_bytes(&mut self, info: &[u8]) {
        let my = self.mycall_for_active();
        match self.active {
            ActivePane::Channel(n) if self.channel_linked(n) => {
                self.send_channel_packet(n, info);
            }
            ActivePane::Channel(_) => {
                let (dest, vias) = ax25::parse_unproto(&self.cfg.beacon.address);
                self.status = format!(
                    "KISS: not linked — Connect first, or type on Monitor ({})",
                    ax25::unproto_to(&dest, &vias)
                );
            }
            _ => {
                let (dest, vias) = ax25::parse_unproto(&self.cfg.beacon.address);
                self.send_session_command(SessionCommand::Unproto {
                    port: self.cfg.tnc.kiss_tx_port(),
                    local: my.clone(),
                    remote: dest.clone(),
                    via: vias.clone(),
                    data: info.to_vec(),
                });
                self.status = format!("KISS UI {my} {}", ax25::unproto_to(&dest, &vias));
            }
        }
    }

    /// List every decoded receive independently of endpoint delivery.
    fn kiss_show_transit(&mut self, frame: &Ax25Frame, port: u8) {
        let info_mon =
            ax25::bytes_to_monitor_body(&frame.info, self.session.umlaut_for(SLOT_MONITOR));
        let info_for_mon = if info_mon.is_empty() {
            None
        } else {
            Some(info_mon.as_str())
        };
        let info_text = decode_rx_text(&frame.info, self.session.umlaut_for(SLOT_MONITOR));
        if !self.monitor_lists_frame(frame.kind, &frame.src, &frame.dest, &frame.via) {
            // Hidden UI frames never reach the monitor header, which is the
            // other place a packet is counted. Record them here, once.
            if matches!(frame.kind, Ax25FrameType::Ui) {
                self.note_heard_on_monitor(
                    &frame.src,
                    &frame.dest,
                    &frame.via,
                    &info_text,
                    Some(frame.info.as_slice()),
                    Some(port),
                );
            }
            return;
        }
        self.push_kiss_mon_frame(PadKind::Incoming, frame, info_for_mon, &info_text, port);
        if matches!(frame.kind, Ax25FrameType::Ui) {
            let mon = ax25::tag_kiss_port(&frame.monitor_line(), port);
            self.feed_spy_pads(&frame.src, PadKind::Incoming, &mon);
            if let Some(text) = info_for_mon {
                if let Some(line) = ax25::monitor_info_line(text) {
                    self.feed_spy_pads(&frame.src, PadKind::Incoming, &line);
                }
            }
        }
    }

    /// Hostmode/modem command TX — skipped on KISS (identity lives in AX.25 headers).
    fn tx_modem_line(&mut self, channel: u8, line: &str) {
        if self.software_ax25() && !self.cfg.tnc.has_wa8ded() {
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
        let _ = self
            .cmd_tx
            .try_send(TncCommand::HostFrame { channel, data });
    }

    fn submit(&mut self) {
        self.finish_compose(true);
    }

    /// Monitor = 0, channel *n* = *n*. Other panes have no compose slot.
    fn compose_slot_idx(&self) -> Option<usize> {
        match self.active {
            ActivePane::Monitor => Some(0),
            ActivePane::Channel(n) if (1..=self.channels).contains(&n) => Some(n as usize),
            _ => None,
        }
    }

    fn compose_enabled(&self) -> bool {
        self.compose_slot_idx().is_some()
    }

    /// One history/display line — paste must not embed raw newlines.
    fn sanitize_compose_line(line: &str) -> String {
        line.replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
            .map(str::trim_end)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn push_compose_history(&mut self, slot: usize, line: String) {
        let Some(c) = self.compose.get_mut(slot) else {
            return;
        };
        c.history.push_back(line);
        while c.history.len() > TYPED_HISTORY {
            c.history.pop_front();
        }
        c.hist_cursor = None;
        c.hist_stash.clear();
    }

    /// Enter sends the compose line with a trailing CR. Ctrl-Enter leaves the CR off.
    fn finish_compose(&mut self, carriage_return: bool) {
        let Some(slot) = self.compose_slot_idx() else {
            return;
        };
        let raw = self
            .compose
            .get_mut(slot)
            .map(|c| std::mem::take(&mut c.input))
            .unwrap_or_default();
        let normalized = crate::tnc::normalize_newlines(&raw);
        if normalized.contains('\n') {
            self.dispatch_compose_paste(slot, &normalized);
            return;
        }

        let line = if carriage_return {
            normalized.trim_end().to_string()
        } else {
            normalized.trim_end_matches(['\r', '\n']).to_string()
        };
        let line = Self::sanitize_compose_line(&line);
        if line.is_empty() {
            if let Some(c) = self.compose.get_mut(slot) {
                c.hist_cursor = None;
                c.hist_stash.clear();
            }
            return;
        }
        self.dispatch_compose_line(slot, line, carriage_return);
    }

    /// Multi-line paste: send complete lines; keep a trailing fragment in compose.
    fn dispatch_compose_paste(&mut self, slot: usize, normalized: &str) {
        let (parts, remainder) = split_compose_paste(normalized);
        if let Some(c) = self.compose.get_mut(slot) {
            c.input = remainder;
        }
        for part in parts {
            let line = Self::sanitize_compose_line(&part);
            if !line.is_empty() {
                self.dispatch_compose_line(slot, line, true);
            }
        }
    }

    fn dispatch_compose_line(&mut self, slot: usize, line: String, carriage_return: bool) {
        self.push_compose_history(slot, line.clone());
        let channel_line = if matches!(self.active, ActivePane::Channel(_) | ActivePane::Monitor) {
            expand_sgr_arrows(&line)
        } else {
            line.clone()
        };

        // Channel transcript is always stored. Echo only hides our rows.
        // BIN-RX paints `#OK#` / `#NO#` itself, so Echo must not add a second copy.
        let wire_answer = matches!(self.active, ActivePane::Channel(n) if {
            let t = line.trim();
            self.autobin_pending(n)
                && (t.eq_ignore_ascii_case("#OK#") || t.eq_ignore_ascii_case("#NO#"))
        });
        if !wire_answer {
            self.echo_own_line(&channel_line);
        }

        let write_ch = match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        };
        if self
            .user_write
            .get(write_ch as usize)
            .and_then(|w| w.as_ref())
            .is_some()
        {
            self.capture_write_line(write_ch, &line);
            return;
        }

        let linked = matches!(self.active, ActivePane::Channel(n) if self.channel_linked(n));
        if !linked && self.mail_send_open(write_ch) && self.take_mail_line(write_ch, &line) {
            return;
        }
        // Local test of a script: the next typed line is its input.
        // A second //RUN is refused. A linked channel sends typed text onward.
        if !linked && self.external_busy(write_ch) {
            if self.another_run(&line) {
                self.warn_running(write_ch);
                return;
            }
            // CAT and 7+ are input for the program. //CAT and //7+ are commands.
            if slash_verb(&line).is_none() {
                self.feed_external(write_ch, &line);
                return;
            }
        }

        // `M` lists frames on the monitor. KISS and TFPCX keep the packet
        // and only change this filter. WA8DED still sends `M` to the modem.
        if self.take_monitor_command(&line) {
            return;
        }
        if let Some(input) = MonitorFilter::parse_command(&line) {
            if let MonitorInput::Set(args) = &input {
                if let Ok(filter) = MonitorFilter::apply(&self.monitor_filter, args) {
                    self.monitor_filter = filter;
                    self.monitor_mode = self.monitor_filter.label();
                    self.log_event(format!("monitor mode → {}", self.monitor_mode));
                }
            }
            let _ = self.cmd_tx.try_send(TncCommand::HostFrame {
                channel: 0,
                data: line.trim().as_bytes().to_vec(),
            });
            return;
        }

        // A linked channel: `//mh` and the rest go to the other station.
        // Running them here as well shows this station's answer (its own
        // heard list) and then the other station's answer on top.
        if let Some((action, args)) = parse_line(&line) {
            if typed_runs_locally(linked, action) {
                self.handle_remote(action, &args);
                return;
            }
            // A linked `//RTT` would make DOS answer `//e //rtt #<short clock>`
            // and then ignore the echo. Send our own millisecond stamp for
            // DOS to echo, so the trip is measured here when it comes back.
            if linked
                && action == RemoteAction::RoundTrip
                && matches!(rtt_ask(&args), RttAsk::Probe)
            {
                if let ActivePane::Channel(n) = self.active {
                    self.start_rtt(n);
                    return;
                }
            }
        } else if !linked && slash_verb(&line).is_some() {
            // Internal missed. GPI in data/, then an EXTERNAL program, then the refusal.
            self.resolve_file_command(write_ch, &line);
            return;
        }

        let line = &channel_line;
        if let ActivePane::Channel(n) = self.active {
            if self.channel_lock.get(n as usize).copied().unwrap_or(false) {
                if let Some(sealed) = self.seal_channel_line(n, line) {
                    self.write_operator_text(&sealed, carriage_return);
                    if n >= 1 && self.channel_linked(n) && !line.trim().is_empty() {
                        self.relay_chat_from(n, line, false);
                    }
                } else {
                    self.status = "Lock needs a linked call".into();
                }
                return;
            }
        }
        self.write_operator_text(line, carriage_return);
        if let ActivePane::Channel(n) = self.active {
            if n >= 1 && self.channel_linked(n) && !line.trim().is_empty() {
                self.relay_chat_from(n, line, false);
            }
        }
    }

    fn linked_call(&self, channel: u8) -> Option<String> {
        self.channel_peer
            .get(channel as usize)
            .and_then(|p| p.clone())
            .filter(|c| !c.is_empty() && c != "****")
    }

    fn clear_lock(&mut self, channel: u8) {
        if let Some(flag) = self.channel_lock.get_mut(channel as usize) {
            *flag = false;
        }
    }

    /// Seal one typed line with this channel's call and the linked call.
    fn seal_channel_line(&self, channel: u8, line: &str) -> Option<String> {
        let peer = self.linked_call(channel)?;
        let mine = self.effective_mycall(channel);
        Some(cipher::seal(&mine, &peer, line))
    }

    fn note_rx_calls(&mut self, channel: u8, from: &str, to: &str) {
        if let Some(slot) = self.rx_calls.get_mut(channel as usize) {
            *slot = Some((from.to_string(), to.to_string()));
        }
    }

    /// Open a sealed row that is already on screen. The CR that ends the packet
    /// may still be waiting, so this does not wait for the row to close.
    fn reveal_locked_row(&mut self, channel: u8) {
        if channel < 1 {
            return;
        }
        let idx = channel as usize;
        let Some(seq) = self.rx_row.get(idx).copied().flatten() else {
            return;
        };
        let text = self
            .pads
            .get(idx)
            .and_then(|pad| pad.text_if_seq(seq))
            .unwrap_or_default();
        let Some(plain) = self.unlock_incoming(channel, &text) else {
            return;
        };
        if let Some(pad) = self.pads.get_mut(idx) {
            pad.set_text_if_seq(seq, &plain);
        }
    }

    /// A line wrapped in `Æ`…`¿` becomes plaintext and turns this channel's lock on.
    /// The pair is the frame's from and to. The channel's own calls are the fallback.
    fn unlock_incoming(&mut self, channel: u8, text: &str) -> Option<String> {
        let from_frame = self
            .rx_calls
            .get(channel as usize)
            .and_then(|pair| pair.clone())
            .and_then(|(from, to)| cipher::open(&from, &to, text));
        let (plain, mine, peer) = if let Some(plain) = from_frame {
            let (from, to) = self
                .rx_calls
                .get(channel as usize)
                .and_then(|pair| pair.clone())
                .unwrap_or_default();
            (plain, to, from)
        } else {
            let peer = self.linked_call(channel)?;
            let mine = self.effective_mycall(channel);
            let plain = cipher::open(&mine, &peer, text)?;
            (plain, mine, peer)
        };
        let was_off = self.channel_lock.get(channel as usize).copied() != Some(true);
        if let Some(flag) = self.channel_lock.get_mut(channel as usize) {
            *flag = true;
        }
        if was_off {
            self.status = format!(
                "Lock ON — {} / {}",
                cipher::base_call(&mine),
                cipher::base_call(&peer)
            );
        }
        Some(plain)
    }

    /// `//Read` with no name sends this station's mail when `data/MAIL/{CALL}.GPM`
    /// exists. A file name is still a USER file. `*.GPM` is never a USER read.
    fn read_user_or_mail(&mut self, channel: u8, args: &str) {
        let peer = self.linked_call(channel);
        match mail::classify(peer.as_deref(), args) {
            MailAsk::Own => {
                if self.send_own_mail(channel) {
                    return;
                }
                if args.trim().is_empty() {
                    self.send_user_catalog("");
                } else {
                    self.status = "no mail".into();
                    self.send_report("no mail");
                }
            }
            MailAsk::Foreign => {
                self.status = "only your own mail".into();
                self.send_report("only your own mail");
            }
            MailAsk::NotMail => {
                if args.trim().is_empty() {
                    self.send_user_catalog("");
                } else {
                    self.read_from_user(args);
                }
            }
        }
    }

    /// `//MAIL` prints this station's mail as text. It does not send a USER file.
    fn reply_station_mail(&mut self, channel: u8) {
        let Some(call) = self.linked_call(channel) else {
            self.reply_report(channel, "You have no mail");
            self.status = "You have no mail".into();
            return;
        };
        let _ = mail::ensure_mail_dir(&mail::mail_dir());
        let Some(text) = mail::read_text(&mail::mail_dir(), &call) else {
            self.reply_report(channel, "You have no mail");
            self.status = "You have no mail".into();
            return;
        };
        if text.trim().is_empty() {
            self.reply_report(channel, "You have no mail");
            self.status = "You have no mail".into();
            return;
        }
        let mut ctx = self.macro_ctx();
        self.apply_channel_peer(&mut ctx, channel);
        let text = gpi::expand_gpi_text(&text, &ctx);
        self.status = format!("Mail {call}");
        self.reply_remote_text(channel, &text);
    }

    /// Send `{CALL}.GPM` when it exists. Returns false when there is no mail.
    fn send_own_mail(&mut self, channel: u8) -> bool {
        let Some(call) = self.linked_call(channel) else {
            return false;
        };
        let _ = mail::ensure_mail_dir(&mail::mail_dir());
        let Some(text) = mail::read_text(&mail::mail_dir(), &call) else {
            return false;
        };
        let mut ctx = self.macro_ctx();
        self.apply_channel_peer(&mut ctx, channel);
        let text = gpi::expand_gpi_text(&text, &ctx);
        self.status = format!("Mail {call}");
        self.send_remote_text(&text);
        true
    }

    /// `//Read` — list USER, send a part as text, or encode a normal file.
    fn read_from_user(&mut self, args: &str) {
        let dir = self.cfg.paths.user_dir.clone();
        let _ = sevenplus::ensure_user_dir(&dir);
        let name = args.trim();
        if name.is_empty() {
            let listing = userdir::catalog(&dir, "");
            self.status = "USER".into();
            self.send_remote_text(&listing);
            return;
        }
        match userdir::read_file(&dir, name) {
            Ok(bytes) => {
                if sevenplus::is_part_filename(name) {
                    self.status = format!("//Read {name}");
                    self.log_event(format!("//Read {} ({} bytes)", name, bytes.len()));
                    self.send_seven_part(name, &bytes);
                    return;
                }
                match sevenplus::encode_user_file(&dir, name, &bytes) {
                    Ok(n) => {
                        let msg = format!("{name} parts ready for reading");
                        self.status = format!("7+ encoded {name} → {n} part(s)");
                        self.log_event(self.status.clone());
                        self.send_report(&msg);
                    }
                    Err(err) => {
                        self.status = err.clone();
                        self.send_report(&err);
                    }
                }
            }
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
            }
        }
    }

    /// `//Read` of a 7+ part: paced TX with Transfer80 `7+-TX` when KISS-linked.
    fn send_seven_part(&mut self, name: &str, raw: &[u8]) {
        let wire = sevenplus::to_cr_bytes(raw);
        if wire.is_empty() {
            self.status = "USER file is empty".into();
            return;
        }
        let channel = match self.active_channel() {
            Some(ch) if self.channel_linked(ch) && self.software_ax25() => ch,
            _ => {
                self.echo_wire_lines(&wire);
                self.queue_wire_chunks(&wire);
                return;
            }
        };
        if self.autobin.is_some() || self.seven_tx.is_some() {
            self.status = "transfer already running".into();
            self.send_report("a file transfer is already running");
            return;
        }
        self.echo_wire_lines(&wire);
        self.seven_tx = Some(LiveSevenTx {
            channel,
            name: name.to_string(),
            data: wire,
            offset: 0,
            started: Instant::now(),
            tx_id: None,
        });
        self.status = format!("7+-TX: {name}");
        self.pump_seven_tx(channel);
    }

    fn echo_wire_lines(&mut self, wire: &[u8]) {
        let display = latin1_string(wire);
        let mut lines: Vec<&str> = display.split('\r').collect();
        if lines.last().is_some_and(|s| s.is_empty()) {
            lines.pop();
        }
        self.begin_newline_coalesce();
        for line in lines {
            self.echo_own_line(line);
        }
        self.end_newline_coalesce();
    }

    fn queue_wire_chunks(&mut self, wire: &[u8]) {
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        for chunk in wire.chunks(max) {
            if self.software_ax25() {
                self.send_kiss_bytes(chunk);
            } else {
                let channel = match self.active {
                    ActivePane::Channel(n) => n,
                    _ => 0,
                };
                self.send_session_command(SessionCommand::Send {
                    channel,
                    data: chunk.to_vec(),
                    id: None,
                });
            }
        }
    }

    fn pump_seven_tx(&mut self, channel: u8) {
        if self
            .seven_tx
            .as_ref()
            .is_some_and(|tx| tx.channel == channel && tx.tx_id.is_some())
        {
            return;
        }
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        let chunk = {
            let Some(tx) = self.seven_tx.as_mut() else {
                return;
            };
            if tx.channel != channel {
                return;
            }
            if tx.offset >= tx.data.len() {
                None
            } else {
                let end = (tx.offset + max).min(tx.data.len());
                let piece = tx.data[tx.offset..end].to_vec();
                tx.offset = end;
                Some(piece)
            }
        };
        let Some(chunk) = chunk else {
            self.finish_seven_tx();
            return;
        };
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if !self.send_session_command(SessionCommand::Send {
            channel,
            data: chunk,
            id: Some(id),
        }) {
            self.seven_tx = None;
            self.status = format!("7+-TX: channel {channel} has no link");
            return;
        }
        if let Some(tx) = self.seven_tx.as_mut() {
            tx.tx_id = Some(id);
        }
    }

    fn finish_seven_tx(&mut self) {
        let Some(tx) = self.seven_tx.take() else {
            return;
        };
        let got = tx.offset as u64;
        let elapsed = tx.started.elapsed();
        let secs = elapsed.as_secs().max(1);
        let baud = (got.saturating_mul(8) / secs) as u32;
        self.status = format!("7+-TX sent {got} bytes ({baud} Bd)");
        self.log_event(format!("7+-TX {} done, {got} bytes", tx.name));
    }

    fn note_seven_tx_ack(&mut self, channel: u8, id: u64) {
        let acked = self
            .seven_tx
            .as_ref()
            .is_some_and(|tx| tx.channel == channel && tx.tx_id == Some(id));
        if !acked {
            return;
        }
        if let Some(tx) = self.seven_tx.as_mut() {
            tx.tx_id = None;
        }
        self.pump_seven_tx(channel);
    }

    /// Typed text and text files: normalize breaks to `\n`, then one packet
    /// per `packet_payload` bytes. Same chunks for KISS and hostmode.
    fn send_operator_text(&mut self, text: &str) {
        self.write_operator_text(text, false);
    }

    /// `carriage_return` appends a CR after the text. Enter does. Ctrl-Enter does not.
    fn write_operator_text(&mut self, text: &str, carriage_return: bool) {
        let line = text.trim().trim_end_matches(['\r', '\n']);
        if let ActivePane::Channel(n) = self.active {
            if line.eq_ignore_ascii_case("#OK#") && self.accept_autobin(n) {
                return;
            }
            if line.eq_ignore_ascii_case("#NO#") && self.autobin_pending(n) {
                self.cancel_autobin();
                return;
            }
        }
        self.note_sysop_command(text);
        if matches!(self.active, ActivePane::Channel(_)) {
            self.write_channel_bytes(text, carriage_return, false);
            return;
        }
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        let mut chunks = pack_text_payloads(text, max);
        if chunks.is_empty() {
            return;
        }
        if carriage_return {
            let used = chunks.last().map(String::len).unwrap_or(0);
            if used + 1 <= max {
                if let Some(last) = chunks.last_mut() {
                    last.push('\r');
                }
            } else {
                chunks.push("\r".into());
            }
        }
        for chunk in chunks {
            if self.software_ax25() {
                self.send_kiss_text(&chunk);
            } else {
                let channel = match self.active {
                    ActivePane::Channel(n) => n,
                    _ => 0,
                };
                self.send_session_command(SessionCommand::Send {
                    channel,
                    data: chunk.into_bytes(),
                    id: None,
                });
            }
        }
    }

    /// Channel bytes. The äöü button selects CP437 or the Latin-1 umlaut table.
    /// A line the DOS set cannot hold (emoji, euro) is UTF-8 instead.
    /// Remote replies append LF after the usual DOS CR.
    fn write_channel_bytes(&mut self, text: &str, carriage_return: bool, line_feed: bool) {
        let channel = match self.active {
            ActivePane::Channel(n) => n,
            _ => 0,
        };
        let mut raw = encode_channel_wire(text, self.session.umlaut_for(channel));
        if carriage_return && raw.last() != Some(&b'\r') {
            raw.push(b'\r');
        }
        if line_feed && !raw.is_empty() {
            raw.push(b'\n');
        }
        if raw.is_empty() {
            return;
        }
        let max = self.cfg.buffers.packet_payload.max(1) as usize;
        for chunk in packet_chunks(&raw, max) {
            if self.software_ax25() {
                self.send_kiss_bytes(chunk);
            } else {
                self.send_session_command(SessionCommand::Send {
                    channel,
                    data: chunk.to_vec(),
                    id: None,
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
            zone: crate::remote::macros::local_zone(),
            news_hint: if gpi::news_available() {
                "News available — //NEws".into()
            } else {
                String::new()
            },
            origin_line: gpi::random_origin_line(),
            digi: String::new(),
            qrg: self.session.qrg_text(),
            activity: macros::activity_status(self.last_sysop_activity.elapsed()),
            name_unknown: false,
            home: macros::user_home(&self.cfg.paths.user_dir),
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
        self.send_remote_text(&text);
        true
    }

    /// Internal command status on the air: always `<GPNG>: …`.
    fn send_report(&mut self, text: &str) {
        self.send_remote_text(&report(text));
    }

    /// Title on the `<GPNG>:` line, body on the lines after it.
    fn send_report_block(&mut self, title: &str, body: &str) {
        self.send_remote_text(&report_block(title, body));
    }

    fn send_gpi_text(&mut self, text: &str) {
        self.send_displayed_text(text, false);
    }

    fn send_remote_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.send_displayed_text(text, true);
        }
    }

    fn send_displayed_text(&mut self, text: &str, end_reply: bool) {
        // CTEXT / QRT / mail: many rows on the pad, one newline.wav.
        self.begin_newline_coalesce();
        for line in payload_lines(text) {
            self.echo_own_line(&line);
        }
        self.end_newline_coalesce();
        if end_reply && matches!(self.active, ActivePane::Channel(_)) {
            self.note_sysop_command(text);
            self.write_channel_bytes(text, true, true);
        } else if end_reply {
            let mut text = crate::tnc::normalize_newlines(text);
            if !text.ends_with('\n') {
                text.push('\n');
            }
            self.send_operator_text(&text);
        } else {
            self.send_operator_text(text);
        }
    }

    /// Bare `//RTT`. Ask the peer to echo `//rtt #<unix milliseconds>`.
    fn start_rtt(&mut self, channel: u8) {
        if channel < 1 || !self.channel_linked(channel) {
            let msg = "Round trip needs a linked channel";
            self.status = msg.into();
            self.send_report(msg);
            return;
        }
        let token = unix_millis(SystemTime::now());
        if let Some(slot) = self.rtt_probe.get_mut(channel as usize) {
            *slot = Some((token, Instant::now()));
        }
        self.status = format!("RTT #{token}");
        self.send_remote_text(&format!("//e //rtt #{token}"));
    }

    fn answer_rtt(&mut self, channel: u8, args: &str) {
        match rtt_ask(args) {
            RttAsk::Probe => self.start_rtt(channel),
            RttAsk::Stamp(stamp) => self.finish_rtt(channel, stamp),
            RttAsk::Bad => {
                let msg = "RTT needs a time stamp";
                self.status = msg.into();
                self.send_report(msg);
            }
        }
    }

    /// `//rtt #<number>` came back. Our own stamp uses the monotonic instant
    /// from when we sent it. Any other full Unix time is compared with a
    /// new clock reading. The short DOS clock is reported and not subtracted.
    fn finish_rtt(&mut self, channel: u8, stamp: u64) {
        let matched = self
            .rtt_probe
            .get_mut(channel as usize)
            .and_then(|slot| match slot {
                Some((saved, _)) if *saved == stamp => slot.take().map(|(_, started)| started),
                _ => None,
            });
        let msg = if let Some(started) = matched {
            rtt_phrase(started.elapsed()).unwrap_or_else(|| "Round trip timed out".into())
        } else {
            match rtt_from_stamps(stamp, unix_millis(SystemTime::now())) {
                RttRead::Trip(elapsed) => {
                    rtt_phrase(elapsed).unwrap_or_else(|| "Round trip timed out".into())
                }
                RttRead::ShortClock => format!("RTT #{stamp} is too short to be a time"),
                RttRead::Ahead => format!("RTT #{stamp} is ahead of this clock"),
                RttRead::TooOld => format!("RTT #{stamp} is not a recent time"),
            }
        };
        self.status = msg.clone();
        self.send_report(&msg);
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
                // QRT first; DISC only after the TX queue drains. Immediate DISC
                // would clear KISS delivery and drop the rest of the farewell.
                let ch = self.resolve_disconnect_channel(channel);
                if ch < 1 {
                    self.status = "//Q — select a connected channel".into();
                    self.close_remote(prev);
                    return;
                }
                self.active = ActivePane::Channel(ch);
                let ctx = self.macro_ctx();
                if let Some(text) = gpi::load_expanded("qrt", &ctx) {
                    if text.trim().is_empty() {
                        self.disconnect_channel(ch);
                    } else {
                        self.send_remote_text(&text);
                        self.arm_qrt_disconnect(ch);
                    }
                } else {
                    if let Some(stem) = gpi::stem_for_action(action) {
                        self.status = format!("GPI {stem}.GPI not found");
                    }
                    self.disconnect_channel(ch);
                }
                self.close_remote(prev);
                return;
            }
            RemoteAction::Disconnect => {
                let ch = self.resolve_disconnect_channel(channel);
                if ch >= 1 {
                    self.disconnect_channel(ch);
                } else {
                    self.status = "//DISC — select a connected channel".into();
                }
                self.close_remote(prev);
                return;
            }
            RemoteAction::CrossChannelText => {
                self.handle_cross_channel(channel, args);
                self.close_remote(prev);
                return;
            }
            _ => {}
        }

        if action == RemoteAction::Info {
            if let Some(text) = gpi::load_info(&ctx) {
                self.send_remote_text(&text);
                self.close_remote(prev);
                return;
            }
            self.status = "GPI <mycall>.GPI / INFO.GPI not found".into();
            self.close_remote(prev);
            return;
        }

        if let Some(stem) = gpi::stem_for_action(action) {
            if let Some(text) = gpi::load_expanded(stem, &ctx) {
                self.send_remote_text(&text);
                self.close_remote(prev);
                return;
            }
            self.status = format!("GPI {stem}.GPI not found");
            self.close_remote(prev);
            return;
        }

        match action {
            RemoteAction::SevenPlusAutoSave => {
                let on = match args.trim().to_ascii_lowercase().as_str() {
                    "on" | "1" | "yes" => Some(true),
                    "off" | "0" | "no" => Some(false),
                    "" => None,
                    _ => {
                        self.status = "7+ ON or OFF".into();
                        self.send_report("7+ ON or OFF");
                        self.close_remote(prev);
                        return;
                    }
                };
                self.set_seven_plus_pane(ActivePane::Channel(channel), on);
                self.send_report(&format!(
                    "7+ {}",
                    on_off(self.session.seven_plus_for(channel))
                ));
                self.close_remote(prev);
                return;
            }
            RemoteAction::SevenPlusDecode => {
                self.remote_seven_decode(args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::ReadAutoBin => {
                self.remote_rprg(channel, args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::WriteAutoBin => {
                self.remote_wprg(channel, args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Read => {
                self.read_user_or_mail(channel, args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Catalog => {
                self.send_user_catalog(args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::ChangeDir => {
                self.remote_cd(args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Delete => {
                self.delete_user_file(channel, args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Write => {
                self.begin_user_write(channel, args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Send => {
                self.begin_mail_send(channel, args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Mail => {
                self.reply_station_mail(channel);
                self.close_remote(prev);
                return;
            }
            RemoteAction::EndWrite => {
                if !self.finish_user_write(channel) {
                    let msg = "no //Write in progress";
                    self.status = msg.into();
                    self.send_report(msg);
                }
                self.close_remote(prev);
                return;
            }
            RemoteAction::Help => {
                let text = crate::i18n::help_for(&self.cfg.session.language, args);
                let topic = args.trim();
                let title = if topic.is_empty() {
                    "Available Help commands".into()
                } else {
                    format!("Help {}", topic.to_ascii_uppercase())
                };
                self.status = format!("//Help {}", topic);
                self.send_report_block(&title, &text);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Language => {
                self.remote_language(args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Prompt => {
                self.remote_prompt_cmd(args);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Parameters => {
                // WA8DED: live F/@T2/@T3 query (Python //P). Else INI/config fallback.
                if self.cfg.tnc.protocol_kind() == HostProtocol::Wa8ded
                    && self.serial_open
                    && self.cmd_tx.try_send(TncCommand::QueryTiming).is_ok()
                {
                    self.pending_timing = Some((channel, prev));
                    self.status = "//P — querying TNC".into();
                    return;
                }
                let text = self.parameters_text();
                self.send_report_block("Parameters", &text);
                self.close_remote(prev);
                return;
            }
            RemoteAction::ChannelStatus => {
                let ctx = self.macro_ctx();
                let title = format!("Channel state {} {} {}", ctx.date, ctx.time, ctx.zone);
                let text = self.cstatus_text();
                self.send_report_block(&title, &text);
                self.close_remote(prev);
                return;
            }
            RemoteAction::Usage => {
                let text = self.usage_text(channel, args);
                self.send_report_block("Usage", &text);
                self.close_remote(prev);
                return;
            }
            RemoteAction::CutTransfer => {
                let already = self.transfer_cut;
                if self.cut_transfer(channel) {
                    self.transfer_cut = false;
                } else if already {
                    self.transfer_cut = false;
                } else {
                    self.status = "no file transfer".into();
                    self.send_report("no file transfer");
                }
                self.close_remote(prev);
                return;
            }
            RemoteAction::Run => {
                // Bare //RUN sends RUN.GPI. It does not list which
                // languages happen to be installed on this computer.
                if args.trim().is_empty() {
                    if let Some(text) = gpi::load_expanded("run", &ctx) {
                        self.status = "RUN.GPI".into();
                        self.send_remote_text(&text);
                    } else {
                        self.status = "RUN.GPI not found".into();
                        self.send_report("RUN.GPI not found");
                    }
                    self.close_remote(prev);
                    return;
                }
                let done = self.start_external(channel, args);
                if done {
                    self.close_remote(prev);
                } else {
                    self.active = prev;
                }
                return;
            }
            RemoteAction::Version => {
                let line = version::startup_credits();
                self.send_report_block("Version", &line);
            }
            RemoteAction::Echo => {
                self.send_remote_text(args);
            }
            RemoteAction::RoundTrip => {
                self.answer_rtt(channel, args);
            }
            RemoteAction::Bell => {
                // //Bell always plays BELL.WAV. The ♪ mute is only for
                // connect, disconnect, and newline clicks.
                self.status = "Waking up the Sysop".into();
                self.send_report("Waking up the Sysop");
                play_bell_wav(&self.bell_wav);
            }
            RemoteAction::MHeard => {
                let body = self
                    .mheard
                    .remote_table(args, |port| self.mheard_port_label(port));
                self.send_report_block("Monitor Heard - Last heard stations:", &body);
            }
            RemoteAction::Name => {
                self.store_remote_name(channel, args);
            }
            RemoteAction::Activity => {
                let age = self.last_sysop_activity.elapsed();
                let msg = format!(
                    "The last sysop activity was {} ago.",
                    activity_age_phrase(age)
                );
                self.status = msg.clone();
                self.send_report(&msg);
            }
            _ => {
                self.status = format!("remote {action:?} (not implemented)");
                self.send_report("not implemented");
            }
        }
        self.close_remote(prev);
    }

    fn close_remote(&mut self, prev: ActivePane) {
        self.active = prev;
        self.maybe_send_prompt();
    }

    /// egui input on any OS: key, text, click, move, or scroll counts as sysop activity.
    fn touch_sysop_activity(&mut self, ctx: &Context) {
        let busy = ctx.input(|i| {
            i.pointer.any_pressed()
                || i.pointer.any_released()
                || i.pointer.delta().length_sq() > 0.25
                || i.smooth_scroll_delta.length_sq() > 0.0
                || i.raw_scroll_delta.length_sq() > 0.0
                || !i.keys_down.is_empty()
                || i.events.iter().any(|ev| {
                    matches!(
                        ev,
                        egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::PointerMoved(_)
                            | egui::Event::PointerButton { .. }
                            | egui::Event::MouseWheel { .. }
                    )
                })
        });
        if busy {
            self.last_sysop_activity = Instant::now();
        }
    }

    fn maybe_send_prompt(&mut self) {
        if !self.remote_prompt {
            return;
        }
        let template = self.cfg.session.prompt.clone();
        if template.trim().is_empty() {
            return;
        }
        let ctx = self.macro_ctx();
        let line = macros::expand(&template, &ctx);
        if !line.is_empty() {
            self.send_gpi_text(&line);
        }
    }

    fn external_busy(&mut self, channel: u8) -> bool {
        self.external_run
            .get_mut(channel as usize)
            .is_some_and(|gate| gate.is_open())
    }

    fn another_run(&self, line: &str) -> bool {
        if parse_line(line).is_some_and(|(action, _)| action == RemoteAction::Run) {
            return true;
        }
        let Some(request) = external::slash_request(line) else {
            return false;
        };
        let name = request.split_whitespace().next().unwrap_or("");
        !name.is_empty() && external::has_program(&self.cfg.paths.external_dir, name)
    }

    fn warn_running(&mut self, channel: u8) {
        self.status = external::BUSY.into();
        self.reply_report(channel, external::BUSY);
    }

    fn refuse_remote_run(&mut self, channel: u8) {
        self.status = "Remote commands are turned off by the sysop".into();
        self.reply_remote_text(channel, REMOTE_OFF);
    }

    fn stop_external(&mut self, channel: u8) {
        if let Some(gate) = self.external_run.get_mut(channel as usize) {
            gate.close();
        }
        self.clear_external_screen(channel);
    }

    /// `true` when the command finished (a list, an error, or a refusal).
    /// `false` while the script is still running, so the prompt waits.
    fn start_external(&mut self, channel: u8, request: &str) -> bool {
        let cap = self.channels as usize;
        let mut others = 0;
        for (idx, gate) in self.external_run.iter_mut().enumerate() {
            if idx != channel as usize && gate.is_open() {
                others += 1;
            }
        }
        if others >= cap {
            self.warn_running(channel);
            return true;
        }
        let dir = self.cfg.paths.external_dir.clone();
        let _ = crate::paths::ensure_dir(&dir);
        // //RUN DOS shows this folder as C:\. Absolute, so the script's own
        // working directory (EXTERNAL) does not change where C: points.
        let users = &self.cfg.paths.user_dir;
        let users = match users.canonicalize() {
            Ok(path) => path,
            Err(_) => users.clone(),
        };
        std::env::set_var("GPNG_USERDIR", users);
        let opened = {
            let Some(gate) = self.external_run.get_mut(channel as usize) else {
                return true;
            };
            gate.open(&dir, request)
        };
        match opened {
            Ok(external::Opened::List(text)) => {
                self.status = "EXTERNAL".into();
                // Catalog text starts with EXTERNAL; the report title already says that.
                let body = text
                    .strip_prefix("EXTERNAL")
                    .unwrap_or(text.as_str())
                    .trim_start_matches(['\r', '\n']);
                self.send_report_block("EXTERNAL", body);
                true
            }
            Ok(external::Opened::Started) => {
                let linked = channel >= 1 && self.channel_linked(channel);
                let name = self.external_run[channel as usize]
                    .name()
                    .unwrap_or("")
                    .to_string();
                self.external_run[channel as usize].configure(linked);
                self.clear_external_screen(channel);
                self.status = format!("EXTERNAL {name}");
                false
            }
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
                true
            }
        }
    }

    /// Internal commands are already handled. Next a `data/*.GPI` (sent as
    /// text), then an EXTERNAL program whose whole name is the command word.
    /// Anything else is `<GPNG>: Command not recognized`.
    fn resolve_file_command(&mut self, channel: u8, line: &str) -> bool {
        let Some(verb) = slash_verb(line) else {
            return false;
        };
        let prev = self.active;
        if (1..=self.channels).contains(&channel) {
            self.active = ActivePane::Channel(channel);
        }
        if self.serve_gpi_verb(&verb) {
            self.close_remote(prev);
            return true;
        }
        self.active = prev;
        if self.try_external_line(channel, line) {
            return true;
        }
        self.status = "Command not recognized".into();
        self.reply_report(channel, NOT_RECOGNIZED);
        true
    }

    /// `//FREE` when FREE is a script and not a built-in command.
    /// The name must be the whole file (`//DOS`), never a shorter prefix (`//D`).
    fn try_external_line(&mut self, channel: u8, line: &str) -> bool {
        let Some(request) = external::slash_request(line) else {
            return false;
        };
        let name = request.split_whitespace().next().unwrap_or("");
        if name.is_empty() || !external::has_program(&self.cfg.paths.external_dir, name) {
            return false;
        }
        let prev = self.active;
        if (1..=self.channels).contains(&channel) {
            self.active = ActivePane::Channel(channel);
        }
        let done = self.start_external(channel, request);
        if done {
            self.close_remote(prev);
        } else {
            self.active = prev;
        }
        true
    }

    fn feed_external(&mut self, channel: u8, line: &str) {
        let Some(gate) = self.external_run.get_mut(channel as usize) else {
            return;
        };
        if !gate.is_open() {
            return;
        }
        if let Err(err) = gate.write_line(line) {
            self.status = err;
            return;
        }
        self.note_external_screen(channel, &format!("> {line}"));
    }

    fn poll_external(&mut self) {
        let n = self.external_run.len();
        for idx in 0..n {
            if !self.external_run[idx].occupied() {
                continue;
            }
            let channel = idx as u8;
            let name = self.external_run[idx].name().unwrap_or("").to_string();
            let link_up = channel >= 1 && self.channel_linked(channel);
            let Some(tick) = self.external_run[idx].tick(link_up) else {
                continue;
            };
            if !tick.text.is_empty() {
                self.note_external_screen(channel, &tick.text);
                self.reply_on_channel(channel, &tick.text);
            }
            let Some(fin) = tick.finished else {
                continue;
            };
            if !fin.air.is_empty() {
                self.note_external_screen(channel, &fin.air);
                self.reply_on_channel(channel, &fin.air);
            }
            self.status = if fin.code == Some(0) {
                format!("EXTERNAL {name} done")
            } else {
                format!("EXTERNAL {name} stopped")
            };
            self.prompt_on_channel(channel);
        }
    }

    fn clear_external_screen(&mut self, channel: u8) {
        if let Some(buf) = self.external_screen.get_mut(channel as usize) {
            buf.clear();
        }
    }

    /// Rolling copy of the side terminal. The channel log is separate.
    fn note_external_screen(&mut self, channel: u8, text: &str) {
        let Some(buf) = self.external_screen.get_mut(channel as usize) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        buf.push_str(text);
        if !buf.ends_with('\n') {
            buf.push('\n');
        }
        const CAP: usize = 4_000;
        if buf.len() > CAP {
            let mut cut = buf.len() - CAP;
            while cut < buf.len() && !buf.is_char_boundary(cut) {
                cut += 1;
            }
            if let Some(nl) = buf[cut..].find('\n') {
                cut += nl + 1;
            }
            buf.drain(..cut);
        }
    }

    fn reply_on_channel(&mut self, channel: u8, text: &str) {
        if text.is_empty() {
            return;
        }
        let prev = self.active;
        if (1..=self.channels).contains(&channel) {
            self.active = ActivePane::Channel(channel);
        }
        self.send_gpi_text(text);
        self.active = prev;
    }

    fn reply_report(&mut self, channel: u8, text: &str) {
        self.reply_remote_text(channel, &report(text));
    }

    fn reply_remote_text(&mut self, channel: u8, text: &str) {
        let prev = self.active;
        if (1..=self.channels).contains(&channel) {
            self.active = ActivePane::Channel(channel);
        }
        self.send_remote_text(text);
        self.active = prev;
    }

    fn prompt_on_channel(&mut self, channel: u8) {
        let prev = self.active;
        if (1..=self.channels).contains(&channel) {
            self.active = ActivePane::Channel(channel);
        }
        self.maybe_send_prompt();
        self.active = prev;
    }

    fn send_user_catalog(&mut self, spec: &str) {
        let dir = self.cfg.paths.user_dir.clone();
        let _ = sevenplus::ensure_user_dir(&dir);
        let listing = userdir::catalog(&dir, spec);
        self.status = "USER".into();
        self.send_remote_text(&listing);
    }

    fn remote_cd(&mut self, args: &str) {
        let args = args.trim();
        let msg = if args.is_empty() || args == "." || args.eq_ignore_ascii_case("user") {
            "USER"
        } else {
            "USER is the only directory"
        };
        self.status = msg.into();
        self.send_report(msg);
    }

    fn remote_language(&mut self, args: &str) {
        let args = args.trim();
        if args.is_empty() {
            let text = crate::i18n::language_list();
            self.status = "languages".into();
            self.send_report_block("Available languages", &text);
            return;
        }
        match crate::i18n::resolve_language(args) {
            Some((code, label)) => {
                self.cfg.session.language = code.to_string();
                crate::i18n::set_language(code);
                if let Err(err) = self.cfg.persist_language(&self.config_path) {
                    self.status = format!("language: {err}");
                } else {
                    self.status = format!("Language {label}");
                }
                self.send_report(&format!("Language {label}"));
            }
            None => {
                let text = crate::i18n::language_list();
                self.status = format!("Unknown language {args}");
                self.send_report_block(&format!("Unknown language {args}"), &text);
            }
        }
    }

    fn remote_prompt_cmd(&mut self, args: &str) {
        let on = match args.trim().to_ascii_lowercase().as_str() {
            "on" | "1" | "yes" => Some(true),
            "off" | "0" | "no" => Some(false),
            "" => Some(!self.remote_prompt),
            _ => None,
        };
        match on {
            Some(v) => {
                self.remote_prompt = v;
                let msg = format!("Prompt {}", on_off(v));
                self.status = msg.clone();
                self.send_report(&msg);
            }
            None => {
                self.status = "Prompt ON or OFF".into();
                self.send_report("Prompt ON or OFF");
            }
        }
    }

    /// Classic `//PARameters` / `//P` — TNC timing block (not the station chrome).
    fn parameters_text(&self) -> String {
        let mycall = self.mycall_for_active();
        if let Some(banner) = &self.timing_banner {
            return rewrite_timing_mycall(banner, &mycall);
        }
        self.fallback_timing_banner(&mycall)
    }

    fn fallback_timing_banner(&self, mycall: &str) -> String {
        if self.software_ax25() {
            let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
            if let Some(params) = load_kiss_params(&path) {
                let lines = kiss_param_lines(&params);
                return timing_banner_fallback(mycall, &lines, &self.cfg.tnc.monitor_mode);
            }
        }
        let path = resolve_wa8ded_ini(&self.cfg.tnc.wa8ded_ini);
        let open = if path.is_file() {
            wa8ded_section_lines(&path, "open")
        } else {
            self.cfg.tnc.init.clone()
        };
        timing_banner_fallback(mycall, &open, &self.cfg.tnc.monitor_mode)
    }

    /// GP DOS `//CSTATUS`: linked channels only.
    ///
    /// Header is `Channel state` plus the same `%D %T %Z` the macros use.
    /// `<--` is an incoming call, `-->` is one this station placed.
    fn cstatus_text(&self) -> String {
        (1..=self.channels)
            .filter_map(|n| self.channel_state_line(n))
            .collect::<Vec<_>>()
            .join("\r")
    }

    fn channel_state_line(&self, n: u8) -> Option<String> {
        let peer = self.channel_peer.get(n as usize).and_then(|p| p.as_ref())?;
        let my = self.effective_mycall(n);
        let port = self
            .channel_link_port
            .get(n as usize)
            .and_then(|port| *port);
        Some(format_channel_state_line(
            n,
            &my,
            self.link_is_outgoing(n),
            peer,
            port,
        ))
    }

    fn cstatus_line(&self, n: u8, io: bool) -> String {
        let io_mark = if io { "IO " } else { "" };
        let idx = n as usize;
        if self.channel_disconnecting(n) {
            let call = self
                .channel_peer
                .get(idx)
                .and_then(|p| p.as_ref())
                .or_else(|| self.channel_last_call.get(idx).and_then(|p| p.as_ref()))
                .map(String::as_str)
                .unwrap_or("?");
            return format!("Ch. {n} - {io_mark}DISCONNECT in progress to {call}");
        }
        if let Some(call) = self.channel_peer.get(idx).and_then(|p| p.as_ref()) {
            let vias = self.link_vias(n);
            if vias.is_empty() {
                format!("Ch. {n} - {io_mark}CONNECTED to {call}")
            } else {
                format!(
                    "Ch. {n} - {io_mark}CONNECTED to {call} via {}",
                    vias.join(" ")
                )
            }
        } else if let Some(call) = self.channel_connecting.get(idx).and_then(|p| p.as_ref()) {
            format!("Ch. {n} - {io_mark}CONNECT in progress to {call}")
        } else {
            format!("Ch. {n} - {io_mark}DISCONNECTED")
        }
    }

    fn usage_text(&self, channel: u8, args: &str) -> String {
        let args = args.trim();
        if args.is_empty() {
            // Bare //Usage: this channel only. Full grid is //CSTATUS.
            let n = if (1..=self.channels).contains(&channel) {
                channel
            } else {
                match self.active {
                    ActivePane::Channel(n) if (1..=self.channels).contains(&n) => n,
                    _ => 1,
                }
            };
            return self.cstatus_line(n, true);
        }
        let qrg = self.names_qrg();
        match self.names.route_for(args, &qrg) {
            Ok(Some(route)) => {
                let mut line = if route.name.is_empty() {
                    route.call.clone()
                } else {
                    format!("{} {}", route.call, route.name)
                };
                if !route.link_digis.is_empty() {
                    line.push_str(" via ");
                    line.push_str(&route.link_digis.join(" "));
                }
                line
            }
            Ok(None) => format!("{args} — not in NAMES.GP"),
            Err(err) => err,
        }
    }

    fn mail_send_open(&self, channel: u8) -> bool {
        self.mail_send
            .get(channel as usize)
            .and_then(|call| call.as_ref())
            .is_some()
    }

    fn cancel_mail_send(&mut self, channel: u8) {
        if let Some(slot) = self.mail_send.get_mut(channel as usize) {
            *slot = None;
        }
    }

    fn mail_from(&self, channel: u8) -> String {
        let raw = self
            .linked_call(channel)
            .unwrap_or_else(|| self.effective_mycall(channel));
        cipher::base_call(&raw)
    }

    fn begin_mail_send(&mut self, channel: u8, args: &str) {
        let mut words = args.split_whitespace();
        let call = words.next().unwrap_or("");
        if call.is_empty() || words.next().is_some() {
            let msg = "use //SEND and one station call";
            self.status = msg.into();
            self.send_report(msg);
            return;
        }
        if mail::filename(call).is_err() {
            let msg = "use a station call";
            self.status = msg.into();
            self.send_report(msg);
            return;
        }
        let base = cipher::base_call(call);
        if let Some(slot) = self.mail_send.get_mut(channel as usize) {
            *slot = Some(base.clone());
        }
        self.status = format!("mail to {base}");
    }

    /// `true` when this line was the mail message (or a blank line still waiting).
    /// `//Quit` and `//DISConnect` cancel the send and are handled as commands.
    fn take_mail_line(&mut self, channel: u8, line: &str) -> bool {
        let line = line.trim_end_matches(['\r', '\n']);
        if parse_line(line).is_some_and(|(action, _)| {
            action == RemoteAction::Quit || action == RemoteAction::Disconnect
        }) {
            self.cancel_mail_send(channel);
            return false;
        }
        let msg = line.trim();
        if msg.is_empty() {
            return true;
        }
        if msg.chars().count() > 240 || msg.chars().any(|c| c != '\t' && c.is_control()) {
            self.cancel_mail_send(channel);
            let err = "that line cannot be mail";
            self.status = err.into();
            self.reply_report(channel, err);
            return true;
        }
        let Some(to) = self
            .mail_send
            .get(channel as usize)
            .and_then(|call| call.clone())
        else {
            return false;
        };
        self.cancel_mail_send(channel);
        let from = self.mail_from(channel);
        let at = journal::clock(self.cfg.session.utc_offset_min)
            .format("%d-%m-%Y %H:%M:%S")
            .to_string();
        match mail::append_message(&mail::mail_dir(), &to, &from, &at, msg) {
            Ok(()) => {
                let reply = report(&format!("Mail send to {to}"));
                self.status = reply.clone();
                self.reply_remote_text(channel, &reply);
            }
            Err(err) => {
                self.status = err.clone();
                self.reply_report(channel, &err);
            }
        }
        true
    }

    fn begin_user_write(&mut self, channel: u8, args: &str) {
        if self
            .user_write
            .get(channel as usize)
            .and_then(|w| w.as_ref())
            .is_some()
        {
            let msg = "already writing — //END first";
            self.status = msg.into();
            self.send_report(msg);
            return;
        }
        let name = match userdir::safe_name(args) {
            Ok(name) => name,
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
                return;
            }
        };
        if userdir::is_protected(&name) {
            let msg = "README.txt stays";
            self.status = msg.into();
            self.send_report(msg);
            return;
        }
        let dir = self.cfg.paths.user_dir.clone();
        let _ = sevenplus::ensure_user_dir(&dir);
        let owned = self
            .user_written
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&name));
        if dir.join(&name).is_file() && !owned {
            let msg = format!("{name} is not a //Write file");
            self.status = msg.clone();
            self.send_report(&msg);
            return;
        }
        if let Some(slot) = self.user_write.get_mut(channel as usize) {
            *slot = Some(UserWrite {
                name: name.clone(),
                body: String::new(),
            });
        }
        let msg = format!("Writing {name} — //END to finish");
        self.status = msg.clone();
        self.send_report(&msg);
    }

    fn capture_write_line(&mut self, channel: u8, line: &str) {
        let line = line.trim_end_matches(['\r', '\n']);
        let end = parse_line(line).is_some_and(|(action, _)| action == RemoteAction::EndWrite);
        if end {
            self.finish_user_write(channel);
            self.maybe_send_prompt();
            return;
        }
        if let Some(cut) = line.find('\u{001a}') {
            let head = line[..cut].trim_end();
            if !head.is_empty() {
                self.append_user_write(channel, head);
            }
            self.finish_user_write(channel);
            self.maybe_send_prompt();
            return;
        }
        self.append_user_write(channel, line.trim_end());
    }

    fn append_user_write(&mut self, channel: u8, line: &str) {
        let Some(slot) = self
            .user_write
            .get_mut(channel as usize)
            .and_then(|w| w.as_mut())
        else {
            return;
        };
        if !slot.body.is_empty() {
            slot.body.push('\n');
        }
        slot.body.push_str(line);
    }

    /// `true` when a capture was open and has been stored (or the error was sent).
    fn finish_user_write(&mut self, channel: u8) -> bool {
        let Some(slot) = self.user_write.get_mut(channel as usize) else {
            return false;
        };
        let Some(cap) = slot.take() else {
            return false;
        };
        let dir = self.cfg.paths.user_dir.clone();
        let _ = sevenplus::ensure_user_dir(&dir);
        match userdir::write_file(&dir, &cap.name, &cap.body) {
            Ok(()) => {
                let key = cap.name.to_ascii_lowercase();
                if !self.user_written.iter().any(|n| n == &key) {
                    self.user_written.push(key);
                }
                let msg = format!("Stored {}", cap.name);
                self.status = msg.clone();
                self.send_report(&msg);
                true
            }
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
                true
            }
        }
    }

    fn delete_user_file(&mut self, channel: u8, args: &str) {
        let peer = self.linked_call(channel);
        match mail::classify(peer.as_deref(), args) {
            MailAsk::Own => {
                let Some(call) = self.linked_call(channel) else {
                    self.status = "connect a station first".into();
                    self.send_report("connect a station first");
                    return;
                };
                let dir = mail::mail_dir();
                let _ = mail::ensure_mail_dir(&dir);
                match mail::delete_own(&dir, &call) {
                    Ok(true) => {
                        let msg = format!("Deleted {call}.GPM");
                        self.status = msg.clone();
                        self.send_report(&msg);
                    }
                    Ok(false) => {
                        self.status = "no mail".into();
                        self.send_report("no mail");
                    }
                    Err(err) => {
                        self.status = err.clone();
                        self.send_report(&err);
                    }
                }
                return;
            }
            MailAsk::Foreign => {
                self.status = "only your own mail".into();
                self.send_report("only your own mail");
                return;
            }
            MailAsk::NotMail if args.trim().is_empty() => {
                self.status = "connect a station first".into();
                self.send_report("connect a station first");
                return;
            }
            MailAsk::NotMail => {}
        }
        if args.trim().is_empty() {
            return;
        }
        let name = match userdir::safe_name(args) {
            Ok(name) => name,
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
                return;
            }
        };
        if userdir::is_protected(&name)
            || !self
                .user_written
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name))
        {
            let msg = format!("Only a file stored with //Write can be deleted ({name})");
            self.status = msg.clone();
            self.send_report(&msg);
            return;
        }
        let dir = self.cfg.paths.user_dir.clone();
        match userdir::delete_file(&dir, &name) {
            Ok(()) => {
                self.user_written.retain(|n| !n.eq_ignore_ascii_case(&name));
                let msg = format!("Deleted {name}");
                self.status = msg.clone();
                self.send_report(&msg);
            }
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
            }
        }
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
            let shown = self
                .names
                .name_for(&call, &self.names_qrg())
                .unwrap_or_else(|| call.clone());
            self.status = format!("Name {call}: {shown}");
            self.send_report(&format!("Name: {shown}"));
            return;
        }
        match self.names.set_name(&call, name) {
            Ok(stored) => {
                self.status = format!("NAMES.GP {call} = {stored}");
                self.send_report(&format!("Name stored: {stored}"));
            }
            Err(err) => {
                self.status = err.clone();
                self.send_report(&err);
            }
        }
    }

    /// Channel / spy / mail pads wrapped to the live viewport width.
    fn active_layout_for(&self, columns: usize) -> LayoutJob {
        let font = self.pad_font_px();
        match self.active {
            ActivePane::Spy(n) => {
                let idx = n.saturating_sub(1) as usize;
                self.spy_pads
                    .get(idx)
                    .map(|pad| pad.layout_job_spy(columns, self.spy_fg, self.spy_inv, font))
                    .unwrap_or_default()
            }
            ActivePane::Check => {
                self.bbs_pad
                    .layout_job_channel(columns, self.session.umlaut_for(SLOT_BBS), font)
            }
            ActivePane::List => {
                self.box_pad
                    .layout_job_channel(columns, self.session.umlaut_for(SLOT_LIST), font)
            }
            ActivePane::Channel(n) => self
                .pads
                .get(n as usize)
                .map(|pad| {
                    pad.layout_job_channel_show(
                        columns,
                        self.session.umlaut_for(n),
                        self.session.echo_for(n),
                        Some(self.monitor_info),
                        font,
                    )
                })
                .unwrap_or_default(),
            _ => self.monitor_layout_for(columns),
        }
    }

    /// Monospace point size for Monitor, channel, BBS-Menu, and Box-List.
    fn pad_font_px(&self) -> f32 {
        padlog::pad_font_px(self.cfg.session.pad_font_size)
    }

    /// Monitor pad (`pads[0]`). The full Monitor pane and the channel split
    /// both paint this job — there is no second monitor buffer.
    /// `columns` is how many monospace cells fit in the pane right now.
    fn monitor_layout_for(&self, columns: usize) -> LayoutJob {
        let font = self.pad_font_px();
        self.pads
            .first()
            .map(|pad| {
                pad.layout_job_monitor(
                    columns,
                    self.monitor_fg,
                    self.monitor_inv,
                    self.monitor_info,
                    font,
                )
            })
            .unwrap_or_default()
    }

    fn split_visible(&self) -> bool {
        self.split_on && matches!(self.active, ActivePane::Channel(_))
    }

    fn modal_open(&self) -> bool {
        self.icon_editor.is_some()
            || self.session.show_qrg_dialog
            || self.session.show_connect_dialog
            || self.session.show_mycall_dialog
            || self.session.show_spy_dialog
            || self.show_name_dialog
            || self.show_mheard_spy_confirm
            || self
                .autobin
                .as_ref()
                .is_some_and(|live| live.state.pending_offer().is_some())
    }

    /// Any titled dialog that should use the session-strip grey chrome.
    fn popup_chrome_open(&self) -> bool {
        self.modal_open()
            || self.show_setup
            || self.show_tnc
            || self.find_open
            || self.list_offer.is_some()
            || self.show_connect_names
            || self.show_radio_gate
    }

    fn active_channel(&self) -> Option<u8> {
        match self.active {
            ActivePane::Channel(n) if (1..=self.channels).contains(&n) => Some(n),
            _ => None,
        }
    }

    /// DOS `Zoom`: `1` is off, `2` is on. Off keeps the regular console face.
    fn toggle_zoom(&mut self) {
        self.cfg.session.zoom = if zoom_bold(self.cfg.session.zoom) {
            1
        } else {
            2
        };
    }

    /// Swap the embedded JetBrains Mono face when Zoom changes.
    /// Menu text stays the system UI font.
    fn sync_console_font(&mut self, ctx: &Context) {
        let bold = zoom_bold(self.cfg.session.zoom);
        if bold == self.console_bold {
            return;
        }
        self.console_bold = bold;
        install_console_font(ctx, bold);
        ctx.request_repaint();
    }

    fn open_setup(&mut self) {
        self.show_setup = true;
        self.setup_pane = self.active;
        self.setup_cursor = 0;
        self.setup_view = SetupView::Main;
        self.show_tnc = false;
        self.mheard_clean_opened = self.cfg.session.mheard_clean_days;
    }

    fn open_icon_editor(&mut self) {
        if self.icon_editor.is_some() {
            return;
        }
        self.setup_icon_pick = false;
        let path = resolve_icon_path(&self.cfg.session.icon_file);
        match IconEdit::open(path) {
            Ok(edit) => {
                self.status = format!("Editing {}", edit.file_name());
                self.icon_editor = Some(edit);
            }
            Err(err) => self.status = format!("Icon editor: {err}"),
        }
    }

    fn close_setup(&mut self) {
        self.show_setup = false;
        self.setup_color_pick = None;
        self.setup_icon_pick = false;
        self.setup_view = SetupView::Main;
        if let Err(e) = self.cfg.persist_setup(&self.config_path) {
            self.status = format!("setup: {e}");
        } else {
            self.status = "setup saved".into();
        }
        self.persist_session_chrome();
        // The day count is only a draft while the menu is open. Apply it once,
        // after Apply or the window closes, so stepping 0 → 14 does not purge
        // the list at 1, then 2, and so on.
        let days = self.cfg.session.mheard_clean_days;
        if days != self.mheard_clean_opened {
            self.mheard_clean_opened = days;
            self.mheard_clean_at = Some(Instant::now());
            if days > 0 {
                self.apply_mheard_clean(days);
            }
        }
    }

    fn apply_palette(&mut self) {
        let c = &self.cfg.colors;
        self.monitor_bg = egui_color(&c.monitor_background);
        self.monitor_fg = egui_color(&c.monitor_foreground);
        self.monitor_inv = egui_color(&c.monitor_invers_attr);
        self.monitor_info = egui_color(&c.monitor_infoerror);
        self.clock_bg = egui_color(&c.clock_background);
        self.state_bg = egui_color(&c.state_background);
        self.state_fg = egui_color(&c.state_foreground);
        self.input_bg = egui_color(&c.input_background);
        self.input_fg = egui_color(&c.input_foreground);
        self.menu_bg = egui_color(&c.menu_background);
        self.menu_fg = egui_color(&c.menu_foreground);
        self.editor_bg = egui_color(&c.editor_background);
        self.editor_fg = egui_color(&c.editor_foreground);
        self.check_bg = egui_color(&c.check_background);
        self.list_bg = egui_color(&c.list_background);
        self.mh_bg = egui_color(&c.mh_background);
        self.mh_fg = egui_color(&c.mh_foreground);
        self.mh_inv = egui_color(&c.mh_invers_attr);
        self.spy_bg = egui_color(&c.spy_background);
        self.spy_fg = egui_color(&c.spy_foreground);
        self.spy_inv = egui_color(&c.spy_invers_attr);
        self.map_bg = egui_color(&c.map_background);
        self.map_fg = egui_color(&c.map_foreground);
        self.map_grid = egui_color(&c.map_grid);
        self.map_qth = egui_color(&c.map_qth);
        self.map_heard = egui_color(&c.map_heard);
        self.connect_bg = egui_color(&c.connect_background);
        self.connect_fg = egui_color(&c.connect_foreground);
    }

    fn color_triple(&self, slot: ColorSlot) -> [String; 3] {
        let c = &self.cfg.colors;
        match slot {
            ColorSlot::Input => [
                c.input_background.clone(),
                c.input_foreground.clone(),
                c.input_invers_attr.clone(),
            ],
            ColorSlot::State => [
                c.state_background.clone(),
                c.state_foreground.clone(),
                c.state_invers_attr.clone(),
            ],
            ColorSlot::Menu => [
                c.menu_background.clone(),
                c.menu_foreground.clone(),
                c.menu_invers_attr.clone(),
            ],
            ColorSlot::Clock => [c.clock_background.clone(), String::new(), String::new()],
            ColorSlot::Monitor => [
                c.monitor_background.clone(),
                c.monitor_foreground.clone(),
                c.monitor_invers_attr.clone(),
            ],
            ColorSlot::Channel => [
                c.connect_background.clone(),
                c.connect_foreground.clone(),
                c.connect_invers_attr.clone(),
            ],
            ColorSlot::Editor => [
                c.editor_background.clone(),
                c.editor_foreground.clone(),
                c.editor_invers_attr.clone(),
            ],
            ColorSlot::Check => [
                c.check_background.clone(),
                c.check_foreground.clone(),
                c.check_invers_attr.clone(),
            ],
            ColorSlot::List => [
                c.list_background.clone(),
                c.list_foreground.clone(),
                c.list_invers_attr.clone(),
            ],
            ColorSlot::Mh => [
                c.mh_background.clone(),
                c.mh_foreground.clone(),
                c.mh_invers_attr.clone(),
            ],
            ColorSlot::Spy => [
                c.spy_background.clone(),
                c.spy_foreground.clone(),
                c.spy_invers_attr.clone(),
            ],
            ColorSlot::Map => [
                c.map_background.clone(),
                c.map_foreground.clone(),
                c.map_grid.clone(),
            ],
        }
    }

    fn set_color_triple(&mut self, slot: ColorSlot, v: [String; 3]) {
        let c = &mut self.cfg.colors;
        match slot {
            ColorSlot::Input => {
                c.input_background = v[0].clone();
                c.input_foreground = v[1].clone();
                c.input_invers_attr = v[2].clone();
            }
            ColorSlot::State => {
                c.state_background = v[0].clone();
                c.state_foreground = v[1].clone();
                c.state_invers_attr = v[2].clone();
            }
            ColorSlot::Menu => {
                c.menu_background = v[0].clone();
                c.menu_foreground = v[1].clone();
                c.menu_invers_attr = v[2].clone();
            }
            ColorSlot::Clock => {
                c.clock_background = v[0].clone();
            }
            ColorSlot::Monitor => {
                c.monitor_background = v[0].clone();
                c.monitor_foreground = v[1].clone();
                c.monitor_invers_attr = v[2].clone();
            }
            ColorSlot::Channel => {
                c.connect_background = v[0].clone();
                c.connect_foreground = v[1].clone();
                c.connect_invers_attr = v[2].clone();
            }
            ColorSlot::Editor => {
                c.editor_background = v[0].clone();
                c.editor_foreground = v[1].clone();
                c.editor_invers_attr = v[2].clone();
            }
            ColorSlot::Check => {
                c.check_background = v[0].clone();
                c.check_foreground = v[1].clone();
                c.check_invers_attr = v[2].clone();
            }
            ColorSlot::List => {
                c.list_background = v[0].clone();
                c.list_foreground = v[1].clone();
                c.list_invers_attr = v[2].clone();
            }
            ColorSlot::Mh => {
                c.mh_background = v[0].clone();
                c.mh_foreground = v[1].clone();
                c.mh_invers_attr = v[2].clone();
            }
            ColorSlot::Spy => {
                c.spy_background = v[0].clone();
                c.spy_foreground = v[1].clone();
                c.spy_invers_attr = v[2].clone();
            }
            ColorSlot::Map => {
                c.map_background = v[0].clone();
                c.map_foreground = v[1].clone();
                c.map_grid = v[2].clone();
            }
        }
        self.apply_palette();
    }

    fn apply_color_choice(&mut self, slot: ColorSlot, draft: [String; 3], info: Option<String>) {
        if let Some(name) = info {
            self.cfg.colors.monitor_infoerror = name;
        }
        self.set_color_triple(slot, draft);
    }

    fn setup_value(&self, id: RowId) -> String {
        let s = &self.cfg.session;
        let pane = self.setup_pane;
        match id {
            RowId::Remote => on_off(self.session.remote_lit(pane)).into(),
            RowId::Backup => on_off(s.backup).into(),
            RowId::Echo => on_off(self.session.echo_lit(pane)).into(),
            RowId::RxClick => rx_click_plays(s.rx_click).to_string(),
            RowId::ConnectBell => connect_bell_plays(s.connect_bell).to_string(),
            RowId::Umlaut => on_off(self.session.umlaut_lit(pane)).into(),
            RowId::InputColumns => s.input_columns.to_string(),
            RowId::InsertMon => on_off(s.insert_mon_line).into(),
            RowId::Softscroll => on_off(self.session.softscroll_lit(pane)).into(),
            RowId::Language => s.language.clone(),
            RowId::Zoom => on_off(zoom_bold(s.zoom)).into(),
            RowId::FontSize => format!("{}", self.pad_font_px() as u32),
            RowId::Icons => icon_file_label(&s.icon_file),
            RowId::PacketLen => self.cfg.buffers.packet_payload.to_string(),
            RowId::AutoSave => on_off(self.session.seven_plus_lit(pane)).into(),
            RowId::WordWrap => s.editor_wrap.to_string(),
            RowId::EditorBak => on_off(s.editor_bak).into(),
            RowId::RefChannel => {
                if matches!(self.setup_pane, ActivePane::List) {
                    s.list_channel.to_string()
                } else {
                    s.bbs_channel.to_string()
                }
            }
            RowId::FileDrawer => s.file_drawer.clone(),
            RowId::AutoSpy => join_csv_phrases(&s.auto_spy),
            RowId::SpyAutoIgnore => join_csv_phrases(&s.no_auto_spy),
            RowId::MaxSpy => self.spy_limit().to_string(),
            RowId::MheardClean => self.cfg.session.mheard_clean_days.to_string(),
            RowId::MheardClear => String::new(),
            RowId::Locator => {
                if self.cfg.station.locator.is_empty() {
                    "(none)".into()
                } else {
                    self.cfg.station.locator.clone()
                }
            }
            _ => String::new(),
        }
    }

    fn nudge_setup(&mut self, id: RowId, step: i32) {
        match id {
            RowId::Remote => {
                self.session.toggle_remote(self.setup_pane);
            }
            RowId::Backup => self.cfg.session.backup = !self.cfg.session.backup,
            RowId::Echo => {
                self.session.toggle_echo(self.setup_pane);
            }
            RowId::RxClick => {
                let n = (rx_click_plays(self.cfg.session.rx_click) as i32 + step).rem_euclid(3);
                self.cfg.session.rx_click = n as u32;
            }
            RowId::ConnectBell => {
                let n =
                    (connect_bell_plays(self.cfg.session.connect_bell) as i32 + step).rem_euclid(5);
                self.cfg.session.connect_bell = n as u32;
            }
            RowId::Umlaut => {
                self.session.toggle_umlaut(self.setup_pane);
            }
            RowId::InputColumns => {
                let n = self.cfg.session.input_columns as i32 + step;
                self.cfg.session.input_columns = n.clamp(20, 255) as u32;
            }
            RowId::InsertMon => {
                self.cfg.session.insert_mon_line = !self.cfg.session.insert_mon_line
            }
            RowId::Softscroll => {
                self.session.toggle_softscroll(self.setup_pane);
            }
            RowId::Language => {
                self.cfg.session.language =
                    setup::next_name(setup::LANGUAGES, &self.cfg.session.language, step);
                crate::i18n::set_language(&self.cfg.session.language);
            }
            RowId::Zoom => self.toggle_zoom(),
            RowId::FontSize => {
                let current = crate::config::clamp_pad_font(self.cfg.session.pad_font_size) as i32;
                let n = (current + step).clamp(10, 28);
                self.cfg.session.pad_font_size = n as u32;
            }
            RowId::PacketLen => {
                let n = self.cfg.buffers.packet_payload as i32 + step;
                self.cfg.buffers.packet_payload = n.clamp(16, 256) as u32;
                if self.software_ax25() {
                    let n = self.cfg.buffers.packet_payload;
                    let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
                    match store_kiss_paclen(&path, n as u16) {
                        Ok(())
                            if self.serial_open
                                && self.cfg.tnc.protocol_kind() == HostProtocol::Kiss
                                && !self.cfg.tnc.kiss_use_tcp() =>
                        {
                            self.status = format!(
                                "PACLEN {n} — payload {n} bytes (radio command on next KISS enter)"
                            );
                        }
                        Ok(()) => {}
                        Err(err) => self.status = err,
                    }
                }
            }
            RowId::AutoSave => self.set_seven_plus_pane(self.setup_pane, None),
            RowId::WordWrap => {
                let n = self.cfg.session.editor_wrap as i32 + step;
                self.cfg.session.editor_wrap = n.clamp(20, 255) as u32;
            }
            RowId::EditorBak => self.cfg.session.editor_bak = !self.cfg.session.editor_bak,
            RowId::RefChannel => {
                let ch = if matches!(self.setup_pane, ActivePane::List) {
                    &mut self.cfg.session.list_channel
                } else {
                    &mut self.cfg.session.bbs_channel
                };
                let n = *ch as i32 + step;
                *ch = n.clamp(1, i32::from(self.channels)) as u32;
            }
            RowId::Beacon => {
                let n = self.cfg.beacon.period_minutes as i32 + step;
                self.cfg.beacon.period_minutes = n.clamp(0, BEACON_MINUTES_MAX as i32) as u32;
            }
            RowId::MaxSpy => {
                let n = self.cfg.session.max_spy as i32 + step;
                self.cfg.session.max_spy = n.clamp(1, 8) as u32;
                let limit = self.spy_limit();
                if let ActivePane::Spy(slot) = self.active {
                    if slot > limit {
                        self.select(ActivePane::Monitor);
                    }
                }
                if self.session.spy_slot.is_some_and(|slot| slot > limit) {
                    self.session.spy_slot = None;
                }
            }
            RowId::MheardClean => {
                let n = self.cfg.session.mheard_clean_days as i32 + step;
                self.cfg.session.mheard_clean_days =
                    n.clamp(0, super::mheard::CLEAN_DAYS_MAX as i32) as u32;
            }
            _ => {}
        }
    }

    fn activate_setup_row(&mut self) {
        let rows = setup::rows_for(self.setup_pane);
        let Some(row) = rows.get(self.setup_cursor).copied() else {
            return;
        };
        match row.kind {
            RowKind::Submenu => match row.id {
                RowId::Theme => {
                    self.setup_icon_pick = false;
                    self.setup_view = SetupView::Theme { cursor: 0 };
                }
                RowId::Beacon => self.setup_view = self.beacon_view(),
                RowId::Icons => self.setup_icon_pick = !self.setup_icon_pick,
                RowId::Prompt => {
                    self.setup_view = SetupView::Edit {
                        kind: EditKind::Prompt,
                        text: self.cfg.session.prompt.clone(),
                        parent_beacon: false,
                    };
                }
                RowId::FileDrawer => {
                    self.setup_view = SetupView::Edit {
                        kind: EditKind::FileDrawer,
                        text: self.cfg.session.file_drawer.clone(),
                        parent_beacon: false,
                    };
                }
                RowId::AutoSpy => {
                    self.setup_view = SetupView::Edit {
                        kind: EditKind::AutoSpy,
                        text: join_csv_phrases(&self.cfg.session.auto_spy),
                        parent_beacon: false,
                    };
                }
                RowId::SpyAutoIgnore => {
                    self.setup_view = SetupView::Edit {
                        kind: EditKind::SpyAutoIgnore,
                        text: join_csv_phrases(&self.cfg.session.no_auto_spy),
                        parent_beacon: false,
                    };
                }
                RowId::Locator => {
                    self.setup_view = SetupView::Edit {
                        kind: EditKind::Locator,
                        text: self.cfg.station.locator.clone(),
                        parent_beacon: false,
                    };
                }
                id => {
                    if let Some(slot) = id.color_slot() {
                        self.open_color_slot(slot, 0);
                    }
                }
            },
            RowKind::Toggle => self.nudge_setup(row.id, 1),
            RowKind::Number => self.nudge_setup(row.id, 1),
            RowKind::Action => match row.id {
                RowId::MheardClear => self.clear_mheard_list(),
                _ => self.insert_editor_list(),
            },
            RowKind::Exit => self.close_setup(),
        }
    }

    fn open_color_slot(&mut self, slot: ColorSlot, theme_cursor: usize) {
        let saved = self.color_triple(slot);
        let info = if matches!(slot, ColorSlot::Monitor) {
            Some(self.cfg.colors.monitor_infoerror.clone())
        } else {
            None
        };
        self.setup_color_pick = None;
        self.setup_view = SetupView::Colors {
            slot,
            cursor: 0,
            draft: saved.clone(),
            saved,
            info,
            theme_cursor,
        };
    }

    fn activate_theme_row(&mut self) {
        let SetupView::Theme { cursor } = self.setup_view else {
            return;
        };
        let rows = setup::theme_rows();
        let Some(row) = rows.get(cursor).copied() else {
            return;
        };
        match row.kind {
            RowKind::Submenu => match row.id {
                RowId::Icons => self.setup_icon_pick = !self.setup_icon_pick,
                id => {
                    if let Some(slot) = id.color_slot() {
                        self.setup_icon_pick = false;
                        self.open_color_slot(slot, cursor);
                    }
                }
            },
            RowKind::Toggle | RowKind::Number => self.nudge_setup(row.id, 1),
            RowKind::Action | RowKind::Exit => {}
        }
    }

    fn show_theme(&mut self, cursor: usize) {
        self.setup_color_pick = None;
        self.setup_view = SetupView::Theme { cursor };
    }

    fn note_mail_line(&mut self, channel: u8, text: &str) {
        let line = text.trim();
        if line.is_empty() {
            return;
        }
        let prompt = bbs::is_diebox_prompt(line);
        if channel as u32 == self.cfg.session.bbs_channel {
            if prompt {
                self.flush_mail(false, channel);
            } else if let Some(entry) = bbs::parse_check(line) {
                self.bbs_pending.push(entry);
            }
        }
        if channel as u32 == self.cfg.session.list_channel {
            if prompt {
                self.flush_mail(true, channel);
            } else if let Some(entry) = bbs::parse_list(line) {
                self.box_pending.push(entry);
            }
        }
    }

    fn flush_mail(&mut self, list: bool, channel: u8) {
        let pending = if list {
            std::mem::take(&mut self.box_pending)
        } else {
            std::mem::take(&mut self.bbs_pending)
        };
        if pending.is_empty() {
            return;
        }
        let existing = if list { &self.box_rows } else { &self.bbs_rows };
        if existing.is_empty() {
            self.commit_mail(list, pending, channel);
        } else {
            self.list_offer = Some(ListOffer {
                list,
                rows: pending,
                channel,
            });
            self.status = "new list — replace, append, or interrupt".into();
        }
    }

    fn commit_mail(&mut self, list: bool, rows: Vec<MailEntry>, channel: u8) {
        let n = rows.len();
        let shown: Vec<String> = rows.iter().map(MailEntry::display).collect();
        let pad = if list {
            self.box_from = Some(channel);
            self.box_cursor = 0;
            self.box_rows = rows;
            &mut self.box_pad
        } else {
            self.bbs_from = Some(channel);
            self.bbs_cursor = 0;
            self.bbs_rows = rows;
            &mut self.bbs_pad
        };
        pad.clear();
        for line in shown {
            pad.push(PadKind::Incoming, line);
        }
        self.status = format!(
            "{} list: {n} from channel {channel}",
            if list { "Box" } else { "BBS" }
        );
    }

    fn append_mail(&mut self, list: bool, rows: Vec<MailEntry>, channel: u8) {
        let lines: Vec<String> = rows.iter().map(MailEntry::display).collect();
        if list {
            self.box_from = Some(channel);
            for line in lines {
                self.box_pad.push(PadKind::Incoming, line);
            }
            self.box_rows.extend(rows);
        } else {
            self.bbs_from = Some(channel);
            for line in lines {
                self.bbs_pad.push(PadKind::Incoming, line);
            }
            self.bbs_rows.extend(rows);
        }
    }

    fn drop_mail_for(&mut self, channel: u8) {
        if self.bbs_from == Some(channel) {
            self.bbs_rows.clear();
            self.bbs_pending.clear();
            self.bbs_cursor = 0;
            self.bbs_from = None;
            self.bbs_pad.clear();
        }
        if self.box_from == Some(channel) {
            self.box_rows.clear();
            self.box_pending.clear();
            self.box_cursor = 0;
            self.box_from = None;
            self.box_pad.clear();
        }
    }

    fn mail_rows(&self, list: bool) -> &[MailEntry] {
        if list {
            &self.box_rows
        } else {
            &self.bbs_rows
        }
    }

    fn mail_cursor(&self, list: bool) -> usize {
        if list {
            self.box_cursor
        } else {
            self.bbs_cursor
        }
    }

    fn set_mail_cursor(&mut self, list: bool, cursor: usize) {
        let n = self.mail_rows(list).len();
        let cursor = if n == 0 { 0 } else { cursor % n };
        if list {
            self.box_cursor = cursor;
        } else {
            self.bbs_cursor = cursor;
        }
    }

    fn step_mail_cursor(&mut self, list: bool, step: i32) {
        let n = self.mail_rows(list).len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.mail_cursor(list) as i32;
        self.set_mail_cursor(list, (cur + step).rem_euclid(n) as usize);
    }

    fn toggle_mail_mark(&mut self, list: bool) {
        let cursor = self.mail_cursor(list);
        let row = if list {
            self.box_rows.get_mut(cursor)
        } else {
            self.bbs_rows.get_mut(cursor)
        };
        if let Some(row) = row {
            row.marked = !row.marked;
        }
    }

    fn sort_bbs(&mut self) {
        self.bbs_sort_desc = !self.bbs_sort_desc;
        let desc = self.bbs_sort_desc;
        self.bbs_rows.sort_by(|a, b| {
            let key = a
                .rubric
                .to_ascii_uppercase()
                .cmp(&b.rubric.to_ascii_uppercase())
                .then(a.number.cmp(&b.number));
            if desc {
                key.reverse()
            } else {
                key
            }
        });
        self.bbs_cursor = 0;
        self.status = if desc {
            "BBS list sorted by rubric, descending".into()
        } else {
            "BBS list sorted by rubric".into()
        };
    }

    fn run_find(&mut self) {
        let list = matches!(self.active, ActivePane::List);
        let term = self.find_text.trim().to_string();
        if term.is_empty() {
            self.status = "find needs a word".into();
            return;
        }
        let fresh = !term.eq_ignore_ascii_case(&self.find_last);
        self.find_last = term.clone();
        let n = self.mail_rows(list).len();
        if n == 0 {
            self.status = "find: the list is empty".into();
            return;
        }
        let start = if fresh {
            0
        } else {
            self.mail_cursor(list).saturating_add(1)
        };
        let hit = (0..n)
            .map(|i| (start + i) % n)
            .find(|&i| self.mail_rows(list)[i].matches(&term));
        match hit {
            Some(i) => {
                self.set_mail_cursor(list, i);
                self.status = format!("find: line {}", i + 1);
            }
            None => self.status = format!("find: {term} is not in the list"),
        }
        self.find_open = false;
    }

    fn read_mail(&mut self, list: bool) {
        let channel = if list {
            self.cfg.session.list_channel
        } else {
            self.cfg.session.bbs_channel
        } as u8;
        let drawer = self.cfg.session.file_drawer.clone();
        let cursor = self.mail_cursor(list);
        let cmds: Vec<String> = {
            let rows = self.mail_rows(list);
            let marked: Vec<String> = rows
                .iter()
                .filter(|e| e.marked)
                .map(|e| e.read_command(list, &drawer))
                .collect();
            if !marked.is_empty() {
                marked
            } else {
                rows.get(cursor)
                    .map(|e| e.read_command(list, &drawer))
                    .into_iter()
                    .collect()
            }
        };
        if cmds.is_empty() {
            self.status = "no list line".into();
            return;
        }
        for cmd in &cmds {
            self.send_channel_command(channel, cmd);
        }
        self.status = cmds.join("  ");
    }

    fn send_channel_command(&mut self, channel: u8, line: &str) {
        if !(1..=self.channels).contains(&channel) {
            self.status = format!("channel {channel} is not open");
            return;
        }
        if !self.channel_linked(channel) {
            self.status = format!("channel {channel} is not connected");
            return;
        }
        if self.software_ax25() {
            if self.send_channel_packet(channel, line.as_bytes()) {
                if let Some(pad) = self.pads.get_mut(channel as usize) {
                    pad.push(PadKind::Own, line.to_string());
                }
            }
        } else {
            self.tx_modem_line(channel, line);
        }
    }

    /// `//n` / `//n text` / `//0` — cross-channel chat mode.
    fn handle_cross_channel(&mut self, from: u8, args: &str) {
        let Some((target, text)) = split_cross_channel(args) else {
            self.status = "Invalid channel number.".into();
            self.send_report("Invalid channel number.");
            return;
        };
        if target == 0 {
            if self.chat_target(from).is_none() {
                self.status = "not in chat mode".into();
                self.send_report("not in chat mode");
                return;
            }
            self.leave_chat_mode(from, true);
            return;
        }
        if from < 1 || !(1..=self.channels).contains(&target) || target == from {
            self.status = "Invalid channel number.".into();
            self.send_report("Invalid channel number.");
            return;
        }
        if !self.channel_linked(target) {
            self.status = "Channel not connected.".into();
            self.send_report("Channel not connected.");
            return;
        }
        if !self.channel_allows_chat(target) {
            self.status = "Messages are only allowed to TERMs.".into();
            self.send_report("Messages are only allowed to TERMs.");
            return;
        }
        if self.channel_transfer_busy(from) || self.channel_transfer_busy(target) {
            self.status = "Transmitting file, message can't be sent.".into();
            self.send_report("Transmitting file, message can't be sent.");
            return;
        }

        if text.is_empty() {
            self.enter_chat_mode(from, target);
        } else {
            let wire = self.chat_line_for(from, text, true);
            self.chat_bridging = true;
            self.tx_chat_line(target, &wire);
            self.chat_bridging = false;
            self.status = format!("//{target} → ch{target}");
        }
    }

    fn enter_chat_mode(&mut self, from: u8, target: u8) {
        if self.chat_target(from) == Some(target) {
            self.status = format!("Chat mode set to Channel {target}. Terminate with //0");
            self.send_report(&format!(
                "Chat mode set to Channel {target}. Terminate with //0"
            ));
            return;
        }
        if self.chat_target(from).is_some() {
            self.leave_chat_mode(from, true);
        }
        if let Some(slot) = self.channel_chat.get_mut(from as usize) {
            *slot = Some(target);
        }
        self.status = format!("Chat mode set to Channel {target}. Terminate with //0");
        self.send_report(&format!(
            "Chat mode set to Channel {target}. Terminate with //0"
        ));
        let label = self.station_label(from);
        let joined = format!("{label} Joined your channel!");
        self.announce_chat_room(target, from, &joined);
    }

    /// Drop chat mode. `notify` tells the joiner and the room.
    fn leave_chat_mode(&mut self, from: u8, notify: bool) {
        let Some(target) = self.chat_target(from) else {
            return;
        };
        if let Some(slot) = self.channel_chat.get_mut(from as usize) {
            *slot = None;
        }
        if !notify {
            return;
        }
        let label = self.station_label(from);
        if self.channel_linked(from) {
            let prev = self.active;
            self.active = ActivePane::Channel(from);
            self.send_report("Chat mode terminated.");
            self.active = prev;
        }
        if self.channel_linked(target) {
            let left = format!("{label} Left your channel!");
            self.announce_chat_room(target, from, &left);
        }
        self.status = "Chat mode terminated.".into();
    }

    /// Link drop on `channel`: leave if joining, and kick anyone who joined it.
    fn drop_chat_on_disconnect(&mut self, channel: u8) {
        if self.chat_target(channel).is_some() {
            // Target may still be up — tell the room this station left.
            self.leave_chat_mode(channel, true);
        }
        let joiners: Vec<u8> = (1..=self.channels)
            .filter(|&ch| self.chat_target(ch) == Some(channel))
            .collect();
        for joiner in joiners {
            if let Some(slot) = self.channel_chat.get_mut(joiner as usize) {
                *slot = None;
            }
            if self.channel_linked(joiner) {
                let prev = self.active;
                self.active = ActivePane::Channel(joiner);
                self.send_report("Chat mode terminated.");
                self.active = prev;
            }
        }
    }

    /// Tell the room host and every other joiner (not `skip`).
    fn announce_chat_room(&mut self, room: u8, skip: u8, message: &str) {
        self.chat_bridging = true;
        if room != skip && self.channel_linked(room) {
            let prev = self.active;
            self.active = ActivePane::Channel(room);
            self.send_report(message);
            self.active = prev;
        }
        for ch in 1..=self.channels {
            if ch == skip || ch == room {
                continue;
            }
            if self.chat_target(ch) == Some(room) && self.channel_linked(ch) {
                let prev = self.active;
                self.active = ActivePane::Channel(ch);
                self.send_report(message);
                self.active = prev;
            }
        }
        self.chat_bridging = false;
    }

    fn chat_target(&self, channel: u8) -> Option<u8> {
        self.channel_chat.get(channel as usize).copied().flatten()
    }

    /// CALL, or `CALL (Name)` when NAMES.GP knows them.
    fn station_label(&self, channel: u8) -> String {
        let call = self
            .linked_call(channel)
            .unwrap_or_else(|| self.effective_mycall(channel));
        match self.names.name_for(&call, &self.names_qrg()) {
            Some(name) if !name.is_empty() => format!("{call} ({name})"),
            _ => call,
        }
    }

    fn channel_allows_chat(&self, channel: u8) -> bool {
        let Some(peer) = self.linked_call(channel) else {
            return false;
        };
        match self.names.kind_of(&peer, &self.names_qrg()) {
            None | Some(StationKind::Terminal) => true,
            Some(_) => false,
        }
    }

    fn channel_transfer_busy(&self, channel: u8) -> bool {
        if self.seven_capturing(channel) {
            return true;
        }
        self.autobin.as_ref().is_some_and(|live| {
            live.channel == channel
                && (live.state.tx_active()
                    || live.state.recv_progress().is_some()
                    || live.state.pending_offer().is_some())
        })
    }

    /// `from_peer`: line came from the linked station (Incoming). Else sysop typed it.
    fn chat_line_for(&self, from: u8, text: &str, from_peer: bool) -> String {
        let call = if from_peer {
            self.linked_call(from)
                .unwrap_or_else(|| self.effective_mycall(from))
        } else {
            self.effective_mycall(from)
        };
        format!("{call}: {text}")
    }

    /// Bridge chat both ways: joiner ↔ room, and joiner ↔ other joiners.
    fn relay_chat_from(&mut self, from: u8, text: &str, from_peer: bool) {
        if self.chat_bridging || from < 1 {
            return;
        }
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            return;
        }
        let mut dests = Vec::new();
        if let Some(room) = self.chat_target(from) {
            if room != from {
                dests.push(room);
            }
            for ch in 1..=self.channels {
                if ch != from && self.chat_target(ch) == Some(room) {
                    dests.push(ch);
                }
            }
        }
        for ch in 1..=self.channels {
            if self.chat_target(ch) == Some(from) {
                dests.push(ch);
            }
        }
        dests.sort_unstable();
        dests.dedup();
        dests.retain(|&ch| ch != from && self.channel_linked(ch));
        if dests.is_empty() {
            return;
        }
        let wire = self.chat_line_for(from, text, from_peer);
        self.chat_bridging = true;
        for ch in dests {
            self.tx_chat_line(ch, &wire);
        }
        self.chat_bridging = false;
    }

    fn tx_chat_line(&mut self, channel: u8, line: &str) {
        if !self.channel_linked(channel) {
            return;
        }
        let mut raw = line.to_string();
        if !raw.ends_with('\r') {
            raw.push('\r');
        }
        if self.software_ax25() {
            if self.send_channel_packet(channel, raw.as_bytes()) {
                if let Some(pad) = self.pads.get_mut(channel as usize) {
                    pad.push(PadKind::Own, line.to_string());
                }
            }
        } else {
            self.tx_modem_line(channel, line);
        }
    }

    fn insert_editor_list(&mut self) {
        let list = matches!(self.setup_pane, ActivePane::List);
        let text = self.editor.text.clone();
        if text.trim().is_empty() {
            self.status = "editor is empty".into();
            return;
        }
        let mut found = Vec::new();
        for line in text.lines() {
            if let Some(entry) = if list {
                bbs::parse_list(line)
            } else {
                bbs::parse_check(line)
            } {
                found.push(entry);
            }
        }
        if found.is_empty() {
            self.status = "editor has no DieBox list lines".into();
            return;
        }
        let n = found.len();
        let channel = if list {
            self.cfg.session.list_channel
        } else {
            self.cfg.session.bbs_channel
        } as u8;
        self.append_mail(list, found, channel);
        self.status = format!("appended {n} list lines");
    }

    /// Wipe the MHeard list and the file it is stored in.
    fn clear_mheard_list(&mut self) {
        let n = self.mheard.listed_len();
        if n == 0 {
            self.status = "MHeard list is empty".into();
            return;
        }
        self.mheard_selected = None;
        self.mheard_spy_pick = false;
        self.show_mheard_spy_confirm = false;
        self.mheard_spy_pending.clear();
        match self.mheard.clear() {
            Ok(()) => self.status = format!("MHeard list cleared ({n})"),
            Err(e) => self.status = e,
        }
    }

    /// Auto Clean runs about once an hour, and only while a day count is set.
    /// Setup can show a day count that is not committed yet, so this waits.
    fn maybe_clean_mheard(&mut self) {
        if self.show_setup {
            return;
        }
        let days = self.cfg.session.mheard_clean_days;
        if days == 0 {
            return;
        }
        let due = self
            .mheard_clean_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(60 * 60));
        if !due {
            return;
        }
        self.mheard_clean_at = Some(Instant::now());
        self.apply_mheard_clean(days);
    }

    fn apply_mheard_clean(&mut self, days: u32) {
        let kept = self
            .mheard_selected
            .and_then(|i| self.mheard.entries.get(i).map(|e| e.call.clone()));
        let now_ms = Local::now().timestamp_millis();
        let removed = self.mheard.drop_older_than(days, now_ms);
        if removed == 0 {
            return;
        }
        self.mheard_selected = kept.and_then(|call| {
            self.mheard
                .entries
                .iter()
                .position(|e| e.call.eq_ignore_ascii_case(&call))
        });
        self.status = format!("MHeard auto clean removed {removed}");
    }

    fn commit_setup_edit(&mut self, kind: EditKind, text: String, parent_beacon: bool) {
        match kind {
            EditKind::Prompt => self.cfg.session.prompt = text,
            EditKind::FileDrawer => self.cfg.session.file_drawer = text,
            EditKind::BeaconAddress => self.cfg.beacon.address = text,
            EditKind::BeaconText => self.cfg.beacon.text = text,
            EditKind::AutoSpy => {
                self.cfg.session.auto_spy = split_csv_phrases(&text);
            }
            EditKind::SpyAutoIgnore => {
                self.cfg.session.no_auto_spy = split_csv_phrases(&text);
            }
            EditKind::Locator => {
                let t = text.trim();
                if t.is_empty() {
                    self.cfg.station.locator.clear();
                    self.status = "Own locator cleared".into();
                } else if let Some(loc) = crate::locator::normalize_locator(t) {
                    self.cfg.station.locator = loc.clone();
                    self.map_view.fit_locator(&loc);
                    self.status = format!("Own locator {loc}");
                } else {
                    self.status =
                        "Locator must be 6 or 8 characters (e.g. JO32KX / JO32MW49)".into();
                }
            }
        }
        self.setup_view = if parent_beacon {
            self.beacon_view()
        } else {
            SetupView::Main
        };
    }

    fn beacon_view(&self) -> SetupView {
        SetupView::Beacon {
            cursor: 0,
            period: self.cfg.beacon.period_minutes.to_string(),
        }
    }

    fn poll_setup_keys(&mut self, ctx: &Context) {
        let none = egui::Modifiers::NONE;
        if !matches!(
            self.setup_view,
            SetupView::Edit { .. } | SetupView::Beacon { .. }
        ) {
            ctx.memory_mut(|m| m.stop_text_input());
        }
        if matches!(self.setup_view, SetupView::Edit { .. }) {
            if eat_key(ctx, none, egui::Key::Escape) {
                self.setup_view = match &self.setup_view {
                    SetupView::Edit {
                        parent_beacon: true,
                        ..
                    } => self.beacon_view(),
                    _ => SetupView::Main,
                };
            }
            if eat_key(ctx, none, egui::Key::Enter) {
                if let SetupView::Edit {
                    kind,
                    text,
                    parent_beacon,
                } = self.setup_view.clone()
                {
                    self.commit_setup_edit(kind, text, parent_beacon);
                }
            }
            return;
        }
        if eat_key(ctx, none, egui::Key::Escape) {
            match &self.setup_view {
                SetupView::Main => self.close_setup(),
                SetupView::Theme { .. } => {
                    self.setup_icon_pick = false;
                    self.setup_view = SetupView::Main;
                }
                SetupView::Colors { theme_cursor, .. } => {
                    let cursor = *theme_cursor;
                    self.show_theme(cursor);
                }
                _ => self.setup_view = SetupView::Main,
            }
            return;
        }
        let rows = setup::rows_for(self.setup_pane);
        let n = rows.len().max(1);
        if eat_key(ctx, none, egui::Key::ArrowUp) {
            self.step_setup_cursor(-1, n);
        }
        if eat_key(ctx, none, egui::Key::ArrowDown) {
            self.step_setup_cursor(1, n);
        }
        // The beacon box types its period and its text, so the caret keeps the arrows.
        if !matches!(self.setup_view, SetupView::Beacon { .. }) {
            if eat_key(ctx, none, egui::Key::ArrowLeft) {
                self.nudge_open_value(-1);
            }
            if eat_key(ctx, none, egui::Key::ArrowRight) {
                self.nudge_open_value(1);
            }
            if eat_key(ctx, none, egui::Key::Enter) {
                self.activate_open_row();
            }
        }
    }

    fn step_setup_cursor(&mut self, step: i32, len: usize) {
        match &mut self.setup_view {
            SetupView::Colors {
                cursor, slot, info, ..
            } => {
                let len = color_choice_count(*slot, info.is_some()) + 1;
                *cursor = (*cursor as i32 + step).rem_euclid(len as i32) as usize;
            }
            SetupView::Beacon { cursor, .. } => {
                *cursor = (*cursor as i32 + step).rem_euclid(4) as usize;
            }
            SetupView::Theme { cursor } => {
                let n = setup::theme_rows().len().max(1) as i32;
                *cursor = (*cursor as i32 + step).rem_euclid(n) as usize;
            }
            SetupView::Main => {
                self.setup_cursor =
                    (self.setup_cursor as i32 + step).rem_euclid(len as i32) as usize;
            }
            SetupView::Edit { .. } => {}
        }
    }

    fn nudge_open_value(&mut self, step: i32) {
        match &mut self.setup_view {
            SetupView::Colors {
                cursor,
                draft,
                slot,
                info,
                ..
            } if *cursor < 3 && *cursor < color_choice_count(*slot, info.is_some()) => {
                draft[*cursor] = setup::next_name(setup::VGA_NAMES, &draft[*cursor], step);
            }
            SetupView::Colors { cursor, info, .. } if *cursor == 3 => {
                if let Some(name) = info {
                    *name = setup::next_name(setup::VGA_NAMES, name, step);
                }
            }
            SetupView::Main => {
                let rows = setup::rows_for(self.setup_pane);
                if let Some(row) = rows.get(self.setup_cursor).copied() {
                    if matches!(row.kind, RowKind::Toggle | RowKind::Number) {
                        self.nudge_setup(row.id, step);
                    }
                }
            }
            SetupView::Theme { cursor } => {
                let rows = setup::theme_rows();
                if let Some(row) = rows.get(*cursor).copied() {
                    if matches!(row.kind, RowKind::Toggle | RowKind::Number) {
                        self.nudge_setup(row.id, step);
                    }
                }
            }
            _ => {}
        }
    }

    fn activate_open_row(&mut self) {
        match self.setup_view.clone() {
            SetupView::Main => self.activate_setup_row(),
            SetupView::Theme { .. } => self.activate_theme_row(),
            SetupView::Colors {
                slot,
                cursor,
                draft,
                info,
                theme_cursor,
                ..
            } => {
                let apply_at = color_choice_count(slot, info.is_some());
                if cursor >= apply_at {
                    self.apply_color_choice(slot, draft, info);
                    self.show_theme(theme_cursor);
                }
            }
            SetupView::Beacon { .. } => {}
            SetupView::Edit { .. } => {}
        }
    }

    fn paint_setup(&mut self, ctx: &Context) {
        if !self.show_setup {
            return;
        }
        let view = self.setup_view.clone();
        match view {
            SetupView::Edit {
                kind,
                mut text,
                parent_beacon,
            } => {
                let title = match kind {
                    EditKind::Prompt => "Remote control prompt",
                    EditKind::BeaconAddress => "Beacon address",
                    EditKind::BeaconText => "Beacon text",
                    EditKind::FileDrawer => "File drawer",
                    EditKind::AutoSpy => "Auto Spy",
                    EditKind::SpyAutoIgnore => "Spy Auto Ignore",
                    EditKind::Locator => "Own locator (Maidenhead)",
                };
                let mut open = true;
                let mut applied = false;
                centered_popup(title, egui::Id::new("gp_setup_edit"), &mut open).show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    let edit =
                        retro::field(ui, &mut text, ui.available_width(), "", menu_font(17.0));
                    edit.request_focus();
                    if retro::apply(ui, false, menu_font(20.0)) {
                        applied = true;
                    }
                });
                if applied {
                    self.commit_setup_edit(kind, text, parent_beacon);
                } else if !open {
                    self.setup_view = if parent_beacon {
                        self.beacon_view()
                    } else {
                        SetupView::Main
                    };
                } else if let SetupView::Edit { text: slot, .. } = &mut self.setup_view {
                    *slot = text;
                }
            }
            SetupView::Colors {
                slot,
                cursor,
                draft,
                info,
                theme_cursor,
                ..
            } => {
                let title = slot.title();
                let labels = ["Background", "Foreground", "Inverse"];
                let shown = if matches!(slot, ColorSlot::Clock) {
                    1
                } else {
                    3
                };
                let apply_at = color_choice_count(slot, info.is_some());
                let mut open = true;
                let mut applied = false;
                centered_popup(title, egui::Id::new("gp_setup_colors"), &mut open).show(
                    ctx,
                    |ui| {
                        ui.set_min_width(retro::ROW_W);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                for (i, label) in labels.iter().take(shown).enumerate() {
                                    let value = draft[i].to_ascii_uppercase();
                                    let resp = retro::entry(
                                        ui,
                                        label,
                                        &value,
                                        retro::TAB_OPEN,
                                        i == cursor,
                                        300.0,
                                        menu_font(17.0),
                                    );
                                    if resp.clicked() && !resp.double_clicked() {
                                        if let SetupView::Colors { cursor, .. } =
                                            &mut self.setup_view
                                        {
                                            *cursor = i;
                                        }
                                        self.setup_color_pick = if self.setup_color_pick == Some(i)
                                        {
                                            None
                                        } else {
                                            Some(i)
                                        };
                                    }
                                }
                                if let Some(name) = &info {
                                    let value = name.to_ascii_uppercase();
                                    let resp = retro::entry(
                                        ui,
                                        "Info / Error",
                                        &value,
                                        retro::TAB_OPEN,
                                        cursor == 3,
                                        300.0,
                                        menu_font(17.0),
                                    );
                                    if resp.clicked() && !resp.double_clicked() {
                                        if let SetupView::Colors { cursor, .. } =
                                            &mut self.setup_view
                                        {
                                            *cursor = 3;
                                        }
                                        self.setup_color_pick = if self.setup_color_pick == Some(3)
                                        {
                                            None
                                        } else {
                                            Some(3)
                                        };
                                    }
                                }
                            });
                            let bg = egui_color(&draft[0]);
                            let fg = egui_color(&draft[1]);
                            let inv = egui_color(&draft[2]);
                            ui.vertical(|ui| {
                                if matches!(slot, ColorSlot::Clock) {
                                    color_sample(ui, "Clock", Color32::WHITE, bg);
                                } else if let Some(name) = &info {
                                    color_sample(ui, "fm SRC to DEST", fg, bg);
                                    color_sample(ui, "payload", inv, bg);
                                    color_sample(ui, "Info / Error", egui_color(name), bg);
                                } else {
                                    color_sample(ui, "Normal text", fg, bg);
                                    color_sample(ui, "Inverted text", bg, inv);
                                }
                            });
                        });
                        if retro::apply(ui, cursor == apply_at, menu_font(20.0)) {
                            applied = true;
                        }
                    },
                );
                if applied {
                    self.apply_color_choice(slot, draft, info);
                    self.show_theme(theme_cursor);
                } else if !open {
                    self.show_theme(theme_cursor);
                }
            }
            SetupView::Theme { cursor } => {
                let rows = setup::theme_rows();
                let cursor = cursor.min(rows.len().saturating_sub(1));
                let mut open = true;
                let mut back = false;
                centered_popup("Theme Setup", egui::Id::new("gp_setup_theme"), &mut open).show(
                    ctx,
                    |ui| {
                        ui.set_min_width(retro::ROW_W);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for (i, row) in rows.iter().enumerate() {
                            let value = self.setup_value(row.id);
                            let label = row.id.label();
                            let tab = match row.kind {
                                RowKind::Submenu | RowKind::Action => retro::TAB_OPEN,
                                _ => retro::TAB_VALUE,
                            };
                            let resp = retro::entry(
                                ui,
                                &label,
                                &value,
                                tab,
                                i == cursor,
                                retro::ROW_W,
                                menu_font(17.0),
                            );
                            if resp.clicked() && !resp.double_clicked() {
                                if let SetupView::Theme { cursor } = &mut self.setup_view {
                                    *cursor = i;
                                }
                                self.activate_theme_row();
                            }
                        }
                        if retro::apply(ui, false, menu_font(20.0)) {
                            back = true;
                        }
                    },
                );
                if back || !open {
                    self.setup_icon_pick = false;
                    self.setup_view = SetupView::Main;
                }
            }
            SetupView::Beacon { mut period, .. } => {
                let mut open = true;
                let mut applied = false;
                let mut edit_address = false;
                let mut pick_file = false;
                centered_popup(
                    "Beacon parameters",
                    egui::Id::new("gp_setup_beacon"),
                    &mut open,
                )
                .show(ctx, |ui| {
                    ui.set_min_width(retro::ROW_W);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let addr = self.cfg.beacon.address.clone();
                    let resp = retro::entry(
                        ui,
                        "Beacon address",
                        &addr,
                        retro::TAB_OPEN,
                        false,
                        retro::ROW_W,
                        menu_font(17.0),
                    );
                    if resp.clicked() && !resp.double_clicked() {
                        edit_address = true;
                    }
                    let file_mode = self.cfg.beacon.uses_file();
                    let source = if file_mode { "File" } else { "Text" };
                    let resp = retro::entry(
                        ui,
                        "Beacon from",
                        source,
                        retro::TAB_VALUE,
                        false,
                        retro::ROW_W,
                        menu_font(17.0),
                    );
                    if resp.clicked() && !resp.double_clicked() {
                        self.cfg.beacon.set_uses_file(!file_mode);
                    }
                    if self.cfg.beacon.uses_file() {
                        let name = beacon_file_label(&self.cfg.beacon.file);
                        let resp = retro::entry(
                            ui,
                            "Beacon file",
                            &name,
                            retro::TAB_OPEN,
                            false,
                            retro::ROW_W,
                            menu_font(17.0),
                        );
                        if resp.clicked() && !resp.double_clicked() {
                            pick_file = true;
                        }
                    } else {
                        ui.label(
                            RichText::new("Beacon text")
                                .font(menu_font(16.0))
                                .color(Color32::BLACK),
                        );
                        let mut text = self.cfg.beacon.text.clone();
                        retro::field(
                            ui,
                            &mut text,
                            retro::ROW_W,
                            "up to 81 characters",
                            menu_font(17.0),
                        );
                        self.cfg.beacon.text = clip_beacon(&text, BEACON_TEXT_MAX);
                    }
                    ui.label(
                        RichText::new("Period (minutes, 0 = off, 1440 = once a day)")
                            .font(menu_font(16.0))
                            .color(Color32::BLACK),
                    );
                    retro::field(ui, &mut period, retro::ROW_W, "minutes", menu_font(17.0));
                    if retro::apply(ui, false, menu_font(20.0)) {
                        applied = true;
                    }
                });
                store_beacon_period(&mut self.cfg.beacon, &mut period);
                if pick_file {
                    self.pick_beacon_file();
                }
                if edit_address {
                    let text = self.cfg.beacon.address.clone();
                    self.setup_view = SetupView::Edit {
                        kind: EditKind::BeaconAddress,
                        text,
                        parent_beacon: true,
                    };
                } else if applied || !open {
                    self.setup_view = SetupView::Main;
                } else if let SetupView::Beacon { period: slot, .. } = &mut self.setup_view {
                    *slot = period;
                }
            }
            SetupView::Main => {
                let rows = setup::rows_for(self.setup_pane);
                let cursor = self.setup_cursor.min(rows.len().saturating_sub(1));
                let mut open = true;
                let mut applied = false;
                let on_exit = rows
                    .get(cursor)
                    .is_some_and(|row| row.kind == RowKind::Exit);
                centered_popup(
                    "Program parameters",
                    egui::Id::new("gp_setup_main"),
                    &mut open,
                )
                .show(ctx, |ui| {
                    ui.set_min_width(retro::ROW_W);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (i, row) in rows.iter().enumerate() {
                        if row.kind == RowKind::Exit {
                            continue;
                        }
                        let value = self.setup_value(row.id);
                        let label = row.id.label();
                        let tab = match row.kind {
                            RowKind::Submenu | RowKind::Action => retro::TAB_OPEN,
                            _ => retro::TAB_VALUE,
                        };
                        let resp = retro::entry(
                            ui,
                            &label,
                            &value,
                            tab,
                            i == cursor,
                            retro::ROW_W,
                            menu_font(17.0),
                        );
                        if resp.clicked() && !resp.double_clicked() {
                            self.setup_cursor = i;
                            self.activate_setup_row();
                        }
                    }
                    if retro::apply(ui, on_exit, menu_font(20.0)) {
                        applied = true;
                    }
                });
                if applied || !open {
                    self.close_setup();
                }
            }
        }
        self.paint_color_names(ctx);
        self.paint_icon_sets(ctx);
    }

    /// Files in `data/ICONS`, plus the stock `data/ICONS.GP`. A click reloads the bar.
    fn paint_icon_sets(&mut self, ctx: &Context) {
        if !self.setup_icon_pick {
            return;
        }
        if !matches!(self.setup_view, SetupView::Theme { .. }) {
            self.setup_icon_pick = false;
            return;
        }
        let current = icon_file_label(&self.cfg.session.icon_file);
        let names = list_icon_files();
        let mut open = true;
        let mut chosen: Option<String> = None;
        let mut edit = false;
        centered_popup("System Icons", egui::Id::new("gp_setup_icons"), &mut open)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                ui.set_min_width(280.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                let width = ui.available_width().max(240.0);
                if retro::button(ui, "Icon editor", width, 30.0, menu_font(18.0)) {
                    edit = true;
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        let width = ui.available_width().max(240.0);
                        for name in &names {
                            let selected = name.eq_ignore_ascii_case(&current);
                            let row = retro::entry(
                                ui,
                                name,
                                "",
                                Color32::TRANSPARENT,
                                selected,
                                width,
                                menu_font(17.0),
                            );
                            if row.clicked() && !row.double_clicked() {
                                chosen = Some(name.clone());
                            }
                        }
                    });
            });
        if edit {
            self.setup_icon_pick = false;
            self.open_icon_editor();
        } else if let Some(name) = chosen {
            self.use_icon_file(ctx, &name);
            self.setup_icon_pick = false;
        } else if !open {
            self.setup_icon_pick = false;
        }
    }

    fn use_icon_file(&mut self, ctx: &Context, name: &str) {
        let path = resolve_icon_path(name);
        let tex = textures_from_icon_file(ctx, &path);
        if tex.is_empty() {
            self.status = format!("Icons not readable ({})", path.display());
            return;
        }
        self.icon_tex = tex;
        self.cfg.session.icon_file = name.to_string();
        self.status = format!("Icons {name}");
    }

    /// Grid on the left, the 17 icons and the VGA palette on the right.
    /// Save writes the library that is on the bar. Apply closes.
    fn paint_icon_editor(&mut self, ctx: &Context) {
        if self.icon_editor.is_none() {
            return;
        }
        let mut open = true;
        let mut close = false;
        let mut reload: Option<PathBuf> = None;
        let mut note: Option<String> = None;
        {
            let edit = self.icon_editor.as_mut().expect("editor open");
            centered_popup("Icon editor", egui::Id::new("gp_icon_editor"), &mut open).show(
                ctx,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        paint_icon_grid(ui, edit);
                        icon_editor_side(ui, edit, &mut close, &mut reload, &mut note);
                    });
                },
            );
        }
        if let Some(path) = reload {
            let tex = textures_from_icon_file(ctx, &path);
            if !tex.is_empty() {
                self.icon_tex = tex;
            }
        }
        if let Some(note) = note {
            self.status = note;
        }
        if close || !open {
            self.icon_editor = None;
        }
    }

    /// Colour name list for Background, Foreground, or Inverse. A click writes it back.
    fn paint_color_names(&mut self, ctx: &Context) {
        let Some(which) = self.setup_color_pick else {
            return;
        };
        let SetupView::Colors { draft, info, .. } = &self.setup_view else {
            self.setup_color_pick = None;
            return;
        };
        let titles = ["Background", "Foreground", "Inverse", "Info / Error"];
        let title = titles.get(which).copied().unwrap_or("Colour");
        let current = if which < 3 {
            draft.get(which).cloned().unwrap_or_default()
        } else {
            info.clone().unwrap_or_default()
        };
        let mut open = true;
        let mut chosen: Option<usize> = None;
        centered_popup(title, egui::Id::new("gp_setup_color_name"), &mut open)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                ui.set_min_width(280.0);
                ui.spacing_mut().item_spacing.y = 3.0;
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        let width = ui.available_width().max(240.0);
                        for (i, name) in setup::VGA_NAMES.iter().enumerate() {
                            let selected = name.eq_ignore_ascii_case(&current);
                            let row = retro::entry(
                                ui,
                                name,
                                "",
                                Color32::TRANSPARENT,
                                selected,
                                width,
                                menu_font(17.0),
                            );
                            if row.clicked() && !row.double_clicked() {
                                chosen = Some(i);
                            }
                        }
                    });
            });
        if let Some(i) = chosen {
            let name = setup::VGA_NAMES.get(i).map(|s| (*s).to_string());
            if let (
                Some(name),
                SetupView::Colors {
                    draft,
                    info,
                    cursor,
                    ..
                },
            ) = (name, &mut self.setup_view)
            {
                if which < 3 {
                    if let Some(slot) = draft.get_mut(which) {
                        *slot = name;
                    }
                } else if let Some(slot) = info {
                    *slot = name;
                }
                *cursor = which;
            }
            self.setup_color_pick = None;
        } else if !open {
            self.setup_color_pick = None;
        }
    }

    fn answer_list_offer(&mut self, answer: ListAnswer) {
        let Some(offer) = self.list_offer.take() else {
            return;
        };
        match answer {
            ListAnswer::Replace => self.commit_mail(offer.list, offer.rows, offer.channel),
            ListAnswer::Append => {
                let n = offer.rows.len();
                self.append_mail(offer.list, offer.rows, offer.channel);
                self.status = format!("appended {n} lines");
            }
            ListAnswer::Interrupt => self.status = "list left as it was".into(),
        }
    }

    fn paint_list_offer(&mut self, ctx: &Context) {
        if self.list_offer.is_none() {
            return;
        }
        let n = self.list_offer.as_ref().map(|o| o.rows.len()).unwrap_or(0);
        let mut answer = None;
        let mut open = true;
        centered_popup(
            "Cancel the old list?",
            egui::Id::new("gp_list_offer"),
            &mut open,
        )
        .show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(
                RichText::new(format!("{n} new lines"))
                    .color(Color32::BLACK)
                    .font(menu_font(18.0)),
            );
            ui.label(
                RichText::new("What should happen to the old list?")
                    .color(Color32::BLACK)
                    .font(menu_font(16.0)),
            );
            if retro::choice(ui, "Yes — replace", menu_font(17.0)) {
                answer = Some(ListAnswer::Replace);
            }
            if retro::choice(ui, "No — append", menu_font(17.0)) {
                answer = Some(ListAnswer::Append);
            }
            if retro::choice(ui, "Interrupt", menu_font(17.0)) {
                answer = Some(ListAnswer::Interrupt);
            }
        });
        if let Some(answer) = answer {
            self.answer_list_offer(answer);
        } else if !open {
            self.answer_list_offer(ListAnswer::Interrupt);
        }
    }

    fn paint_autobin_offer(&mut self, ctx: &Context) {
        let Some((channel, label)) = self.autobin.as_ref().and_then(|live| {
            live.state
                .pending_offer()
                .map(|offer| (live.channel, autobin_offer_label(offer)))
        }) else {
            return;
        };
        let mut answer = None;
        let mut open = true;
        centered_popup("BIN-RX", egui::Id::new("gp_autobin_rx"), &mut open).show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(
                RichText::new(label)
                    .color(Color32::BLACK)
                    .font(menu_font(18.0)),
            );
            ui.label(
                RichText::new("Save this file?")
                    .color(Color32::BLACK)
                    .font(menu_font(16.0)),
            );
            if retro::choice(ui, "SAVE", menu_font(17.0)) {
                answer = Some(true);
            }
            if retro::choice(ui, "No", menu_font(17.0)) {
                answer = Some(false);
            }
        });
        if answer == Some(true) {
            let _ = self.accept_autobin(channel);
        } else if answer == Some(false) || !open {
            self.cancel_autobin();
        }
    }

    fn paint_find(&mut self, ctx: &Context) {
        if !self.find_open {
            return;
        }
        let mut text = self.find_text.clone();
        let mut open = true;
        let mut applied = false;
        centered_popup("Find", egui::Id::new("gp_find"), &mut open).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(RichText::new("Find this word").font(menu_font(16.0)));
            let edit = retro::field(ui, &mut text, ui.available_width(), "", menu_font(17.0));
            edit.request_focus();
            if retro::apply(ui, false, menu_font(20.0)) {
                applied = true;
            }
        });
        self.find_text = text;
        if applied {
            self.run_find();
        } else if !open {
            self.find_open = false;
        }
    }

    fn paint_tnc_command(&mut self, ctx: &Context) {
        if !self.show_tnc {
            return;
        }
        let mut text = self.tnc_cmd.clone();
        let mut open = true;
        let mut applied = false;
        centered_popup("TNC command", egui::Id::new("gp_tnc_cmd"), &mut open).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(RichText::new("Command for the TNC").font(menu_font(16.0)));
            let edit = retro::field(ui, &mut text, ui.available_width(), "", menu_font(17.0));
            edit.request_focus();
            if retro::apply(ui, false, menu_font(20.0)) {
                applied = true;
            }
        });
        self.tnc_cmd = text;
        if applied {
            let cmd = std::mem::take(&mut self.tnc_cmd);
            self.show_tnc = false;
            self.send_tnc_command(&cmd);
        } else if !open {
            self.show_tnc = false;
        }
    }

    fn send_tnc_command(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        if self.take_monitor_command(cmd) {
            return;
        }
        if let Some(MonitorInput::Set(args)) = MonitorFilter::parse_command(cmd) {
            if let Ok(filter) = MonitorFilter::apply(&self.monitor_filter, &args) {
                self.monitor_filter = filter;
                self.monitor_mode = self.monitor_filter.label();
                self.log_event(format!("monitor mode → {}", self.monitor_mode));
            }
        }
        if self.software_ax25() {
            self.send_kiss_esc(cmd);
            return;
        }
        // WA8DED and TFPCX: the line is a hostmode command. The driver applies it.
        self.tx_modem_line(0, cmd);
        self.status = format!("TNC: {cmd}");
        self.last_link_status = self.status.clone();
    }

    /// Esc `W 30` on KISS: send the TAPR frame for that `[params]` letter and
    /// store the new arg on the same line. A bare `W` shows the stored value.
    fn send_kiss_esc(&mut self, cmd: &str) {
        match parse_kiss_esc(cmd) {
            EscKiss::Ignore => {
                let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
                let letters = load_kiss_params(&path)
                    .unwrap_or_default()
                    .iter()
                    .map(|p| format!("{} ({})", p.letter, p.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.status = if letters.is_empty() {
                    format!("KISS has no command channel for {cmd}")
                } else {
                    format!("KISS has no command channel for {cmd}. Esc params: {letters}, PACLEN")
                };
            }
            EscKiss::BadValue => {
                self.status = if cmd.trim().to_ascii_uppercase().starts_with("PACLEN") {
                    "PACLEN is a number from 0 to 256 (0 means 256 bytes)".into()
                } else {
                    "KISS param is a number from 0 to 255".into()
                };
            }
            EscKiss::ShowPaclen => {
                let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
                self.status = match load_kiss_paclen(&path) {
                    Some(n) => format!("PACLEN {n} — payload {} bytes", kiss_payload_len(n)),
                    None => "KISS.INI [params] has no PACLEN".into(),
                };
            }
            EscKiss::SetPaclen(n) => self.set_kiss_paclen(n),
            EscKiss::Show(letter) => {
                let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
                self.status = match load_kiss_params(&path)
                    .and_then(|ps| ps.into_iter().find(|p| p.letter == letter))
                {
                    Some(p) => format!(
                        "{} (Esc {}) = {} — Esc {} and a number 0–255",
                        p.name, p.letter, p.value, p.letter
                    ),
                    None => format!("KISS.INI [params] has no Esc {letter}"),
                };
            }
            EscKiss::Set { letter, value } => self.set_kiss_param(letter, value),
        }
    }

    fn set_kiss_param(&mut self, letter: char, value: u8) {
        let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
        let Some(params) = load_kiss_params(&path) else {
            self.status = "KISS.INI has no [params] section".into();
            return;
        };
        let Some(def) = params.iter().find(|p| p.letter == letter) else {
            self.status = format!("KISS.INI [params] has no Esc {letter}");
            return;
        };
        let name = def.name.clone();
        let kiss_cmd = def.cmd;
        if self.cfg.tnc.protocol_kind() == HostProtocol::Baycom {
            if let Err(e) = crate::tnc::baycom::Settings::default().set_parameter(kiss_cmd, value) {
                self.status = e.to_string();
                return;
            }
        }
        if let Err(err) = store_kiss_param(&path, letter, value) {
            self.status = err;
            return;
        }
        let sent = self.serial_open;
        if sent {
            if let Err(e) = self.cmd_tx.try_send(TncCommand::KissParam {
                cmd: kiss_cmd,
                arg: value,
            }) {
                self.status = format!("{name}={value} stored but not queued: {e}");
                return;
            }
        }
        if let Some(params) = load_kiss_params(&path) {
            let lines = kiss_param_lines(&params);
            let mycall = self.mycall_for_active();
            self.timing_banner = Some(timing_banner_fallback(
                &mycall,
                &lines,
                &self.cfg.tnc.monitor_mode,
            ));
        }
        let driver = if self.cfg.tnc.protocol_kind() == HostProtocol::Baycom {
            "Baycom"
        } else {
            "kiss"
        };
        self.status = if sent {
            format!("{driver} {name}={value} (Esc {letter}, cmd {kiss_cmd})")
        } else {
            format!("{driver} {name}={value} stored — link is not open")
        };
        self.last_link_status = self.status.clone();
        self.log_event(self.status.clone());
    }

    /// Esc `PACLEN n` stores the command and makes every KISS info field that long.
    fn set_kiss_paclen(&mut self, paclen: u16) {
        let path = resolve_kiss_ini(&self.cfg.tnc.kiss_ini);
        if let Err(err) = store_kiss_paclen(&path, paclen) {
            self.status = err;
            return;
        }
        let bytes = kiss_payload_len(paclen);
        self.cfg.buffers.packet_payload = bytes;
        let saved = self.cfg.persist_setup(&self.config_path);
        let radio_later = self.serial_open
            && self.cfg.tnc.protocol_kind() == HostProtocol::Kiss
            && !self.cfg.tnc.kiss_use_tcp();
        self.status = match (saved, radio_later) {
            (Ok(()), false) => format!("PACLEN {paclen} — payload {bytes} bytes"),
            (Ok(()), true) => format!(
                "PACLEN {paclen} — payload {bytes} bytes (radio command on next KISS enter)"
            ),
            (Err(err), _) => format!("PACLEN {paclen} — payload {bytes} bytes ({err})"),
        };
        self.last_link_status = self.status.clone();
        self.log_event(self.status.clone());
    }

    /// Hand the latest beacon setup to the wall-clock thread and show its note.
    ///
    /// Closing setup does not restart the period. The thread is what actually
    /// sends; this only keeps its copy current while the window is painting.
    fn poll_beacon(&mut self) {
        self.beacon_sched.publish(BeaconSnap {
            kiss: self.software_ax25(),
            port: self.cfg.tnc.kiss_tx_port(),
            beacon: self.cfg.beacon.clone(),
            my_call: self.cfg.primary_mycall().to_string(),
            qrg: self.session.qrg_text(),
            version: version::APP_VERSION.into(),
            cmd_tx: self.cmd_tx.clone(),
            mail_dir: mail::mail_dir(),
        });
        if let Some(note) = self.beacon_sched.take_note() {
            self.status = note;
        }
    }

    fn pick_beacon_file(&mut self) {
        let current = PathBuf::from(self.cfg.beacon.file.trim());
        let start = current
            .parent()
            .filter(|p| p.is_dir())
            .map(std::path::Path::to_path_buf)
            .or_else(|| Some(crate::paths::data_dir()));
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Beacon file", &["gp", "GP", "txt", "TXT"])
            .add_filter("All files", &["*"])
            .set_title("Select beacon file");
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            self.cfg.beacon.file = path.display().to_string();
            self.cfg.beacon.set_uses_file(true);
            self.status = format!("Beacon file {name}");
        }
    }

    /// DOS chapter 4 keys. A key with no product yet answers on the status line.
    fn poll_hotkeys(&mut self, ctx: &Context) {
        if self.icon_editor.is_some() {
            return;
        }
        let alt = egui::Modifiers::ALT;
        let none = egui::Modifiers::NONE;
        // ALT-Enter: exclusive fullscreen (title bar off). Works on every screen.
        if eat_key(ctx, alt, egui::Key::Enter) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            self.status = if self.fullscreen {
                "Fullscreen ON (ALT-Enter)".into()
            } else {
                "Fullscreen OFF".into()
            };
            return;
        }
        if self.show_radio_gate {
            if eat_key(ctx, alt, egui::Key::X) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if let Some(id) = ctx.memory(|m| m.focused()) {
                if (0..=10u8).any(|n| id == compose_id_for(n as usize)) {
                    ctx.memory_mut(|m| m.surrender_focus(id));
                }
            }
            return;
        }
        if eat_key(ctx, alt, egui::Key::X) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.list_offer.is_some() {
            if eat_key(ctx, none, egui::Key::Y) {
                self.answer_list_offer(ListAnswer::Replace);
            } else if eat_key(ctx, none, egui::Key::N) {
                self.answer_list_offer(ListAnswer::Append);
            } else if eat_key(ctx, none, egui::Key::Escape) {
                self.answer_list_offer(ListAnswer::Interrupt);
            }
            return;
        }
        if self.find_open {
            if eat_key(ctx, none, egui::Key::Escape) {
                self.find_open = false;
            }
            if eat_key(ctx, none, egui::Key::Enter) {
                self.run_find();
            }
            return;
        }
        if self.show_tnc {
            if eat_key(ctx, none, egui::Key::Escape) {
                self.show_tnc = false;
            }
            if eat_key(ctx, none, egui::Key::Enter) {
                let cmd = std::mem::take(&mut self.tnc_cmd);
                self.show_tnc = false;
                self.send_tnc_command(&cmd);
            }
            return;
        }
        if self.show_setup {
            self.poll_setup_keys(ctx);
            return;
        }
        let ctrl = egui::Modifiers::CTRL;
        let shift = egui::Modifiers::SHIFT;

        if self.show_connect_names && eat_key(ctx, none, egui::Key::Escape) {
            self.show_connect_names = false;
        }
        if self.modal_open() {
            return;
        }
        self.poll_split_keys(ctx);
        self.poll_gpw_key(ctx);

        for n in 1..=12u8 {
            let Some(key) = fn_key(n) else { continue };
            if eat_key(ctx, shift, key) {
                self.send_shift_macro(n);
                return;
            }
        }
        if eat_key(ctx, none, egui::Key::F11) || eat_key(ctx, alt, egui::Key::F1) {
            self.select(ActivePane::Check);
            return;
        }
        if eat_key(ctx, none, egui::Key::F12) || eat_key(ctx, alt, egui::Key::F2) {
            self.select(ActivePane::List);
            return;
        }
        if eat_key(ctx, alt, egui::Key::F4) {
            self.clear_active_pad();
            return;
        }
        if eat_key(ctx, alt, egui::Key::F6) {
            self.status = if self.last_link_status.is_empty() {
                "no link status yet".into()
            } else {
                self.last_link_status.clone()
            };
            return;
        }
        if eat_key(ctx, alt, egui::Key::F7) {
            self.status = if self.last_error.is_empty() {
                "no error yet".into()
            } else {
                self.last_error.clone()
            };
            return;
        }
        for n in 1..=10u8 {
            let Some(key) = fn_key(n) else { continue };
            if eat_key(ctx, none, key) {
                if n <= self.channels {
                    self.select(ActivePane::Channel(n));
                }
                return;
            }
        }

        if eat_key(ctx, alt, egui::Key::M) {
            self.select(ActivePane::Monitor);
            return;
        }
        if eat_key(ctx, alt, egui::Key::H) {
            // DOS chapter 8: MH list.
            self.select(ActivePane::Mh);
            return;
        }
        if eat_key(ctx, alt, egui::Key::W) {
            // World map (MHeard EMPTY slot).
            self.select(ActivePane::Map);
            return;
        }
        if eat_key(ctx, alt, egui::Key::E) {
            self.open_ctext_editor();
            return;
        }
        if eat_key(ctx, alt, egui::Key::O) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.status = "minimized".into();
            return;
        }
        if eat_key(ctx, alt, egui::Key::R) {
            let on = self.session.toggle_softscroll(self.active);
            self.status = softscroll_status(on);
            self.persist_session_chrome();
            return;
        }
        if eat_key(ctx, alt, egui::Key::U) {
            self.open_setup();
            return;
        }
        if eat_key(ctx, alt, egui::Key::Z) {
            self.toggle_zoom();
            let on = zoom_bold(self.cfg.session.zoom);
            self.status = format!("Zoom {}", on_off(on));
            if let Err(e) = self.cfg.persist_setup(&self.config_path) {
                self.status = format!("Zoom: {e}");
            }
            return;
        }
        if eat_key(ctx, alt, egui::Key::G) {
            self.session.hour_gong = !self.session.hour_gong;
            self.status = format!(
                "Hourly gong {}",
                if self.session.hour_gong { "ON" } else { "OFF" }
            );
            self.persist_session_chrome();
            return;
        }
        if eat_key(ctx, alt, egui::Key::Q) {
            self.session.open_qrg_dialog();
            return;
        }
        if eat_key(ctx, alt, egui::Key::C) {
            self.hotkey_connect();
            return;
        }
        if eat_key(ctx, alt, egui::Key::D) {
            if let Some(ch) = self.active_channel() {
                self.disconnect_channel(ch);
            } else {
                self.status = "Disconnect — select a channel".into();
            }
            return;
        }
        if eat_key(ctx, alt, egui::Key::Y) {
            self.hotkey_mycall();
            return;
        }
        if eat_key(ctx, alt, egui::Key::N) {
            self.open_name_dialog();
            return;
        }
        if eat_key(ctx, alt, egui::Key::T) {
            self.hotkey_connect_text();
            return;
        }
        if eat_key(ctx, alt, egui::Key::A) {
            self.status = self.beacon_sched.send_now();
            return;
        }
        if eat_key(ctx, alt, egui::Key::B) {
            self.session.scr_lock = !self.session.scr_lock;
            self.status = format!("ScrLock {}", on_off(self.session.scr_lock));
            self.persist_session_chrome();
            return;
        }
        if eat_key(ctx, alt, egui::Key::S) {
            self.save_active();
            return;
        }
        if eat_key(ctx, alt, egui::Key::F) {
            self.hotkey_file_send();
            return;
        }
        if eat_key(ctx, alt, egui::Key::L) {
            if matches!(self.active, ActivePane::Editor) {
                self.editor_load_dialog();
            } else {
                self.status = "Load — switch to the editor (ALT-E)".into();
            }
            return;
        }

        if eat_key(ctx, ctrl, egui::Key::Enter) {
            self.finish_compose(false);
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::D) {
            let text = self.macro_ctx().date;
            self.insert_buffer_text(&text);
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::T) {
            let text = self.macro_ctx().time;
            self.insert_buffer_text(&text);
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::O) {
            let text = gpi::random_origin_line();
            if text.is_empty() {
                self.status = "ORIGIN.GPI has no line".into();
            } else {
                self.insert_buffer_text(&text);
            }
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::Y) {
            self.cut_edit_line();
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::C) {
            self.copy_edit_line();
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::P) {
            self.paste_edit_line();
            return;
        }
        if eat_key(ctx, ctrl, egui::Key::K) {
            self.status = "Copying marked QSO text is not built yet".into();
            return;
        }

        if matches!(self.active, ActivePane::Check | ActivePane::List) {
            let list = matches!(self.active, ActivePane::List);
            if eat_key(ctx, none, egui::Key::Escape) {
                self.select(ActivePane::Monitor);
                return;
            }
            if eat_key(ctx, none, egui::Key::ArrowUp) {
                self.step_mail_cursor(list, -1);
                return;
            }
            if eat_key(ctx, none, egui::Key::ArrowDown) {
                self.step_mail_cursor(list, 1);
                return;
            }
            if eat_key(ctx, none, egui::Key::Space) {
                self.toggle_mail_mark(list);
                self.step_mail_cursor(list, 1);
                return;
            }
            if eat_key(ctx, none, egui::Key::Enter) {
                self.read_mail(list);
                return;
            }
        }
        // DOS pre-write: Up/Down walk typed lines, Delete drops a recalled line.
        if self.start_screen.is_none() && self.compose_enabled() {
            if eat_key(ctx, none, egui::Key::ArrowUp) {
                self.recall_history(true);
                if let Some(id) = self.compose_focus_id() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
                return;
            }
            if eat_key(ctx, none, egui::Key::ArrowDown) {
                self.recall_history(false);
                if let Some(id) = self.compose_focus_id() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
                return;
            }
            let recalling = self
                .compose_slot_idx()
                .and_then(|s| self.compose.get(s))
                .and_then(|c| c.hist_cursor)
                .is_some();
            if recalling && eat_key(ctx, none, egui::Key::Delete) {
                let _ = self.delete_recalled_history_line();
                return;
            }
            if eat_key(ctx, none, egui::Key::Enter) {
                self.submit();
                return;
            }
        }
        if eat_key(ctx, none, egui::Key::Insert) {
            self.status = "Insert/overwrite is not built yet".into();
            return;
        }
        if eat_key(ctx, none, egui::Key::Escape) {
            self.show_tnc = true;
            self.tnc_cmd.clear();
        }
    }

    fn hotkey_connect(&mut self) {
        match self.active {
            ActivePane::Mh => self.connect_from_mheard(),
            ActivePane::Channel(n) => self.open_connect_dialog_for_channel(n),
            _ => {
                if let Some(ch) = self.first_free_channel() {
                    self.open_connect_dialog_for_channel(ch);
                } else {
                    self.status = "no free channel for Connect".into();
                }
            }
        }
    }

    fn hotkey_mycall(&mut self) {
        let Some(ch) = self.active_channel() else {
            self.status = "Mycall — select a channel".into();
            return;
        };
        if self.channel_linked(ch) {
            self.status = "Mycall — only while disconnected".into();
            return;
        }
        self.open_mycall_dialog();
    }

    fn hotkey_connect_text(&mut self) {
        let Some(ch) = self.active_channel() else {
            self.status = "Connect text — select a channel".into();
            return;
        };
        if !self.channel_linked(ch) {
            self.status = "Connect text — not connected".into();
            return;
        }
        self.send_connect_text(ch);
        self.status = "connect text sent".into();
    }

    fn save_active(&mut self) {
        match self.active {
            ActivePane::Editor => self.editor_save(),
            ActivePane::Channel(n) => {
                if !self.accept_autobin(n) {
                    self.save_screen_transcript();
                }
            }
            ActivePane::Monitor | ActivePane::Check | ActivePane::List | ActivePane::Spy(_) => {
                self.save_screen_transcript()
            }
            _ => self.status = "Save — this window has no transcript".into(),
        }
    }

    fn hotkey_file_send(&mut self) {
        if matches!(self.active, ActivePane::Check | ActivePane::List) {
            self.find_open = true;
            self.find_text.clear();
            return;
        }
        let ready = self
            .active_channel()
            .is_some_and(|ch| self.channel_linked(ch));
        self.status = if ready {
            "File send is not built yet".into()
        } else {
            "File send — connect a station first".into()
        };
    }

    fn send_shift_macro(&mut self, n: u8) {
        if self.active_channel().is_none() {
            self.status = format!("F{n}.GPI — select a channel");
            return;
        }
        let ctx = self.macro_ctx();
        let Some(text) = gpi::load_expanded(&format!("F{n}"), &ctx) else {
            self.status = format!("F{n}.GPI not found");
            return;
        };
        self.send_gpi_text(&text);
        self.status = format!("sent F{n}.GPI");
    }

    fn insert_buffer_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if matches!(self.active, ActivePane::Editor) {
            self.editor.append_on_cursor_line(text);
            return;
        }
        let Some(slot) = self.compose_slot_idx() else {
            return;
        };
        let Some(c) = self.compose.get_mut(slot) else {
            return;
        };
        if !c.input.is_empty() && !c.input.ends_with(' ') && !c.input.ends_with('\n') {
            c.input.push(' ');
        }
        c.input.push_str(text);
        // Multi-line clips are absorbed on the next compose paint / submit.
    }

    fn cut_edit_line(&mut self) {
        if matches!(self.active, ActivePane::Editor) {
            self.line_clip = self.editor.cut_cursor_line();
        } else if let Some(slot) = self.compose_slot_idx() {
            self.line_clip = self
                .compose
                .get_mut(slot)
                .map(|c| std::mem::take(&mut c.input))
                .unwrap_or_default();
        } else {
            return;
        }
        self.status = "line cut".into();
    }

    fn copy_edit_line(&mut self) {
        self.line_clip = if matches!(self.active, ActivePane::Editor) {
            self.editor.copy_cursor_line()
        } else if let Some(slot) = self.compose_slot_idx() {
            self.compose
                .get(slot)
                .map(|c| c.input.clone())
                .unwrap_or_default()
        } else {
            return;
        };
        self.status = "line copied".into();
    }

    fn paste_edit_line(&mut self) {
        if self.line_clip.is_empty() {
            self.status = "nothing to paste".into();
            return;
        }
        let clip = self.line_clip.clone();
        if matches!(self.active, ActivePane::Editor) {
            self.editor.paste_line(&clip);
        } else if self.compose_enabled() {
            self.insert_buffer_text(&clip);
            self.absorb_compose_newlines();
        }
    }

    fn poll_split_keys(&mut self, ctx: &Context) {
        if !matches!(self.active, ActivePane::Channel(_)) {
            return;
        }
        let (toggle, up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::ALT, egui::Key::F8),
                i.consume_key(egui::Modifiers::ALT, egui::Key::F9),
                i.consume_key(egui::Modifiers::ALT, egui::Key::F10),
            )
        });
        if toggle {
            self.toggle_split();
        }
        if up {
            self.step_split(1.0);
        }
        if down {
            self.step_split(-1.0);
        }
    }

    /// Ctrl-B copies the last sysop reply into the compose box.
    fn poll_gpw_key(&mut self, ctx: &Context) {
        let hit = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::B));
        if !hit || self.last_gpw.is_empty() {
            return;
        }
        if let Some(slot) = self.compose_slot_idx() {
            if let Some(c) = self.compose.get_mut(slot) {
                if !c.input.is_empty() && !c.input.ends_with(' ') && !c.input.ends_with('\n') {
                    c.input.push(' ');
                }
                c.input.push_str(&self.last_gpw.clone());
            }
        }
    }

    fn toggle_split(&mut self) {
        if !matches!(self.active, ActivePane::Channel(_)) {
            return;
        }
        self.split_on = !self.split_on;
        self.status = if self.split_on {
            "Splitscreen on — QSO above, monitor below".into()
        } else {
            "Splitscreen off".into()
        };
    }

    /// `dir` > 0 shows more monitor (bar moves up). `dir` < 0 shows more QSO.
    fn step_split(&mut self, dir: f32) {
        if !matches!(self.active, ActivePane::Channel(_)) {
            return;
        }
        if !self.split_on {
            if dir > 0.0 {
                self.split_on = true;
                self.split_lower = self.split_lower.max(0.32);
                self.status = "Splitscreen on".into();
            }
            return;
        }
        self.split_lower = clamp_split_lower(self.split_lower + dir * 0.06);
        self.status = format!("Splitscreen {:.0}% monitor", self.split_lower * 100.0);
    }

    /// `delta_y` is screen pixels, positive downward (more QSO, less monitor).
    fn drag_split(&mut self, delta_y: f32) {
        if !matches!(self.active, ActivePane::Channel(_)) {
            return;
        }
        if !self.split_on {
            if delta_y < 0.0 {
                self.split_on = true;
                self.split_lower = self.split_lower.max(0.32);
            } else {
                return;
            }
        }
        let body = self.split_body_h.max(120.0);
        self.split_lower = clamp_split_lower(self.split_lower - delta_y / body);
        self.status = format!("Splitscreen {:.0}% monitor", self.split_lower * 100.0);
    }

    fn note_split_right_click(&mut self, ui: &Ui, rect: Rect) {
        if !ui.is_enabled()
            || self.icon_editor.is_some()
            || !matches!(self.active, ActivePane::Channel(_))
        {
            return;
        }
        let hit = ui.input(|i| {
            i.pointer.secondary_clicked()
                && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
        });
        if hit {
            self.toggle_split();
        }
    }

    /// Channel: ▲▼ size the split. Monitor: they still nudge the pad.
    /// Spy, BBS-Menu, and Box-List do not use them.
    fn on_scroll_arrow(&mut self, dir: f32) {
        if matches!(self.active, ActivePane::Channel(_)) {
            if self.split_drag_dy.abs() > SPLIT_DRAG_SLOP {
                return;
            }
            self.step_split(dir);
            return;
        }
        if matches!(self.active, ActivePane::Monitor) && !self.session.scr_lock {
            self.session
                .nudge_lines(if dir > 0.0 { -1 } else { 1 }, self.pad_font_px());
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
            ActivePane::Check => {
                self.bbs_rows.clear();
                self.bbs_pending.clear();
                self.bbs_cursor = 0;
                self.bbs_from = None;
                self.bbs_pad.clear();
                self.bbs_pad.save_log(&padlog::bbs_path(&dir));
                self.status = "BBS menu cleared".into();
            }
            ActivePane::List => {
                self.box_rows.clear();
                self.box_pending.clear();
                self.box_cursor = 0;
                self.box_from = None;
                self.box_pad.clear();
                self.box_pad.save_log(&padlog::box_path(&dir));
                self.status = "Box list cleared".into();
            }
            ActivePane::Editor => {
                self.editor.text.clear();
                self.editor.dirty = true;
                self.editor.cursor_line = 1;
                self.editor.cursor_col = 1;
                self.status = "Editor cleared".into();
            }
            ActivePane::Mh => {
                self.status = "MHeard is not cleared from the keyboard".into();
            }
            ActivePane::Map => {
                self.status = "Map is not cleared from the keyboard".into();
            }
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
            ActivePane::Map => "Map".into(),
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
        if kind == PadKind::System {
            return;
        }
        self.consider_auto_spy(src, line);
        let src_up = src.to_ascii_uppercase();
        let calls = self.cfg.session.spy_calls.clone();
        let nnns = self.spy_auto_nnn.clone();
        let mut saved: Vec<(String, Option<u16>)> = Vec::new();
        let limit = self.spy_limit() as usize;
        for (i, call) in calls.iter().enumerate().take(limit) {
            if call.is_empty() || !call.eq_ignore_ascii_case(&src_up) {
                continue;
            }
            if let Some(pad) = self.spy_pads.get_mut(i) {
                pad.push(kind, line.to_string());
            }
            let nnn = nnns.get(i).copied().flatten();
            if !saved
                .iter()
                .any(|(c, n)| c.eq_ignore_ascii_case(call) && *n == nnn)
            {
                saved.push((call.clone(), nnn));
            }
        }
        for (call, nnn) in saved {
            self.append_spy_line(&call, line, nnn);
        }
    }

    /// `AutoSpy=` / `NoAutoSpy=`: case-sensitive substrings of the monitor line.
    /// When a line matches, open a free spy slot on `src` if one is not watching yet.
    fn consider_auto_spy(&mut self, src: &str, line: &str) {
        if !auto_spy_line_matches(
            line,
            &self.cfg.session.auto_spy,
            &self.cfg.session.no_auto_spy,
        ) {
            return;
        }
        let src = src.trim();
        if src.is_empty() {
            return;
        }
        self.cfg.normalize_spy_calls();
        let limit = self.spy_limit() as usize;
        if self
            .cfg
            .session
            .spy_calls
            .iter()
            .take(limit)
            .any(|c| !c.is_empty() && c.eq_ignore_ascii_case(src))
        {
            return;
        }
        match self.assign_spy_first_empty(src) {
            Ok(slot) => match self.bind_auto_spy_file(slot, src) {
                Some(nnn) => {
                    let base = transcript_call(src);
                    self.status = format!("AutoSpy ch{slot} → {base}.{nnn:03}");
                }
                None => {
                    self.status = format!("AutoSpy ch{slot} → {src} (no free .NNN)");
                }
            },
            Err(_) => {
                // All ten slots full — stay quiet; next Kill Spy frees one.
            }
        }
    }

    /// Pick `CALL.NNN` for this AutoSpy slot. Same call reuses the number until
    /// the program exits; a restart takes the next free file on disk (0…999).
    fn bind_auto_spy_file(&mut self, slot: u8, call: &str) -> Option<u16> {
        let base = transcript_call(call);
        if base.is_empty() {
            return None;
        }
        let idx = slot.saturating_sub(1) as usize;
        if let Some(nnn) = self.spy_auto_run.get(&base).copied() {
            if let Some(slot_nnn) = self.spy_auto_nnn.get_mut(idx) {
                *slot_nnn = Some(nnn);
            }
            return Some(nnn);
        }
        let dir = self.cfg.paths.spy_dir.clone();
        if let Err(e) = crate::paths::ensure_dir(&dir) {
            tracing::warn!("spy dir: {e}");
            return None;
        }
        let Some(nnn) = next_spy_nnn(&dir, &base) else {
            return None;
        };
        self.spy_auto_run.insert(base, nnn);
        if let Some(slot_nnn) = self.spy_auto_nnn.get_mut(idx) {
            *slot_nnn = Some(nnn);
        }
        Some(nnn)
    }

    fn clear_spy_auto_slot(&mut self, idx: usize) {
        if let Some(nnn) = self.spy_auto_nnn.get_mut(idx) {
            *nnn = None;
        }
    }
}

/// One GP DOS channel-state row.
///
/// `       Chan   1: NL1STK     <--  NL0MSK`
/// The call column is nine characters. Channel 10 lines up on the colon.
/// A known radio port is added at the end. Without one, the line is unchanged.
fn format_channel_state_line(
    n: u8,
    mycall: &str,
    outgoing: bool,
    peer: &str,
    port: Option<u8>,
) -> String {
    let arrow = if outgoing { "-->" } else { "<--" };
    let mut line = format!("       Chan  {n:>2}: {mycall:<9}  {arrow}  {peer}");
    if let Some(port) = port {
        line.push_str(&format!(" port {port}"));
    }
    line
}

/// Comma-separated phrases for setup display / edit (`fm DB0SGL, ctl UI`).
fn join_csv_phrases(parts: &[String]) -> String {
    parts.join(", ")
}

fn split_csv_phrases(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// DOS AutoSpy: any `auto` phrase is found in the line, and no `ignore` phrase is.
fn auto_spy_line_matches(line: &str, auto: &[String], ignore: &[String]) -> bool {
    if auto.is_empty() {
        return false;
    }
    if !auto
        .iter()
        .any(|p| !p.is_empty() && line.contains(p.as_str()))
    {
        return false;
    }
    !ignore
        .iter()
        .any(|p| !p.is_empty() && line.contains(p.as_str()))
}

fn store_beacon_period(beacon: &mut BeaconConfig, text: &mut String) {
    text.retain(|c| c.is_ascii_digit());
    if text.len() > 4 {
        text.truncate(4);
    }
    let minutes = beacon_minutes(text);
    if text
        .parse::<u32>()
        .ok()
        .is_some_and(|n| n > BEACON_MINUTES_MAX)
    {
        *text = minutes.to_string();
    }
    beacon.period_minutes = minutes;
}

fn beacon_file_label(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "choose a file".into();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn fn_key(n: u8) -> Option<egui::Key> {
    Some(match n {
        1 => egui::Key::F1,
        2 => egui::Key::F2,
        3 => egui::Key::F3,
        4 => egui::Key::F4,
        5 => egui::Key::F5,
        6 => egui::Key::F6,
        7 => egui::Key::F7,
        8 => egui::Key::F8,
        9 => egui::Key::F9,
        10 => egui::Key::F10,
        11 => egui::Key::F11,
        12 => egui::Key::F12,
        _ => return None,
    })
}

/// Hard drop-shadow like classic DOS windows (no soft blur).
fn popup_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: Vec2::new(4.0, 4.0),
        blur: 0.0,
        spread: 0.0,
        color: Color32::from_black_alpha(110),
    }
}

/// Session-strip grey face for every dialog — retro GP, not egui dark mode.
fn popup_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.window_fill = VGA_GRAY;
    v.panel_fill = VGA_GRAY;
    v.window_stroke = Stroke::new(1.0, Color32::BLACK);
    v.window_rounding = egui::Rounding::ZERO;
    v.menu_rounding = egui::Rounding::same(retro::TOP_ROUND);
    v.window_shadow = popup_shadow();
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.override_text_color = Some(Color32::BLACK);
    // Text fields a step lighter than the face; faint rows a step darker.
    v.extreme_bg_color = Color32::from_rgb(220, 220, 220);
    v.faint_bg_color = Color32::from_rgb(150, 150, 150);
    v.code_bg_color = Color32::from_rgb(200, 200, 200);
    let slight = egui::Rounding::same(retro::TOP_ROUND);
    v.widgets.noninteractive.rounding = egui::Rounding::ZERO;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = slight;
    }
    v.widgets.noninteractive.bg_fill = VGA_GRAY;
    v.widgets.noninteractive.weak_bg_fill = VGA_GRAY;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, VGA_DARK_GRAY);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    // Raised button faces — same family as the QRG session strip.
    v.widgets.inactive.bg_fill = Color32::from_rgb(200, 200, 200);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(200, 200, 200);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    v.widgets.hovered.bg_fill = Color32::from_rgb(210, 210, 210);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(210, 210, 210);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    v.widgets.active.bg_fill = Color32::from_rgb(150, 150, 150);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(150, 150, 150);
    v.widgets.open.bg_fill = Color32::from_rgb(200, 200, 200);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(200, 200, 200);
    v.selection.bg_fill = VGA_BLUE;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v
}

fn popup_frame() -> Frame {
    Frame::none()
        .fill(VGA_GRAY)
        .stroke(Stroke::NONE)
        .inner_margin(12.0)
        .rounding(0.0)
        .shadow(popup_shadow())
}

/// White top and left, black bottom and right, on the window itself.
fn paint_menu_border(ctx: &Context, response: &egui::Response) {
    let painter = ctx.layer_painter(response.layer_id);
    retro::border(&painter, response.rect);
}

/// One menu window. `.show` paints the shared 3D rim after the body.
struct MenuPopup<'a> {
    window: egui::Window<'a>,
}

impl<'a> MenuPopup<'a> {
    fn order(mut self, order: egui::Order) -> Self {
        self.window = self.window.order(order);
        self
    }

    fn show<R>(
        self,
        ctx: &Context,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> Option<egui::InnerResponse<Option<R>>> {
        let shown = self.window.show(ctx, add_contents)?;
        paint_menu_border(ctx, &shown.response);
        Some(shown)
    }
}

/// Shared chrome for every popup: named title bar, close X, centered, grey face.
/// The body is painted with `retro` (rows, fields, and Apply).
fn centered_popup<'a>(
    title: impl Into<egui::WidgetText>,
    id: egui::Id,
    open: &'a mut bool,
) -> MenuPopup<'a> {
    MenuPopup {
        window: egui::Window::new(title)
            .id(id)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .frame(popup_frame())
            .open(open),
    }
}

fn name_selector(
    ui: &mut Ui,
    id: &str,
    selected: impl AsRef<str>,
    contents: impl FnOnce(&mut Ui),
) -> egui::Response {
    ui.scope(|ui| {
        let style = ui.style_mut();
        style.visuals.menu_rounding = egui::Rounding::ZERO;
        style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
        style.visuals.window_fill = VGA_GRAY;
        let response = ui
            .push_id(id, |ui| name_select_row(ui, selected.as_ref(), false, true))
            .inner;
        let popup_id = response.id.with("popup");
        if response.clicked()
            || (response.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
        {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        egui::popup::popup_below_widget(
            ui,
            popup_id,
            &response,
            egui::popup::PopupCloseBehavior::CloseOnClick,
            |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, contents);
                retro::border(ui.painter(), ui.min_rect().expand(4.0));
            },
        );
        response
    })
    .inner
}

fn name_select_row(ui: &mut Ui, label: &str, selected: bool, dropdown: bool) -> egui::Response {
    let fg = if selected {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), menu_font(17.0), fg);
    let width = ui.available_width();
    let height = (retro::ROW_H * 0.75).max(galley.mesh_bounds.height() + 4.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let fill = if selected {
        VGA_BLUE
    } else if response.hovered() {
        Color32::from_rgb(214, 214, 214)
    } else {
        VGA_GRAY
    };
    retro::paint(
        ui.painter(),
        rect,
        fill,
        selected || response.is_pointer_button_down_on(),
        0.0,
    );
    let mut text_rect = rect.shrink2(Vec2::new(6.0, 2.0));
    if dropdown {
        text_rect.max.x -= 20.0;
        let arrow_rect = Rect::from_min_max(Pos2::new(rect.right() - 20.0, rect.top()), rect.max);
        retro::paint(
            ui.painter(),
            arrow_rect,
            fill,
            response.is_pointer_button_down_on(),
            0.0,
        );
        let c = arrow_rect.center();
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                c + Vec2::new(-4.0, -2.0),
                c + Vec2::new(4.0, -2.0),
                c + Vec2::new(0.0, 3.0),
            ],
            Color32::BLACK,
            Stroke::NONE,
        ));
    }
    let text_width = galley.size().x;
    // Center the visible glyphs, not the menu font's extra leading.
    let ink = galley.mesh_bounds;
    ui.painter().with_clip_rect(text_rect).galley(
        Pos2::new(
            text_rect.left() - ink.left(),
            text_rect.center().y - ink.center().y,
        ),
        galley,
        fg,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if text_width > text_rect.width() {
        response.on_hover_text(label)
    } else {
        response
    }
}

/// Colour rows in the dialog, not counting Apply.
fn color_choice_count(slot: ColorSlot, info: bool) -> usize {
    match slot {
        ColorSlot::Clock => 1,
        _ if info => 4,
        _ => 3,
    }
}

fn color_sample(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(168.0, 36.0), Sense::hover());
    retro::paint(ui.painter(), rect, bg, false, retro::ROW_ROUND);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        menu_font(16.0),
        fg,
    );
}

fn mods_match(have: egui::Modifiers, want: egui::Modifiers) -> bool {
    have.alt == want.alt
        && have.shift == want.shift
        && have.ctrl == want.ctrl
        && have.command == want.command
}

/// Consume `key` only when the modifier set matches. Shift-F is not F.
fn eat_key(ctx: &Context, mods: egui::Modifiers, key: egui::Key) -> bool {
    let pressed = ctx.input(|i| i.key_pressed(key) && mods_match(i.modifiers, mods));
    if pressed {
        ctx.input_mut(|i| {
            i.consume_key(mods, key);
        });
    }
    pressed
}

impl eframe::App for GpGuiApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.external_run.clear();
        self.flush_window();
        if self.cfg.session.backup {
            self.save_pad_logs();
        }
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.touch_sysop_activity(ctx);
        self.sync_console_font(ctx);
        self.track_window(ctx);
        self.poll_external();
        self.drain_tnc();
        self.poll_qrt_disconnects();
        self.maybe_hour_gong();
        self.maybe_clean_mheard();
        self.poll_beacon();
        let now = Instant::now();
        // Each wake rebuilds the monitor galley. Stay asleep unless something
        // on screen is actually changing.
        let repaint = if self.transfer_row_open() {
            Duration::from_millis(100)
        } else if let Some(left) = self
            .modems
            .iter()
            .filter_map(|face| face.lamp_remaining(now))
            .min()
        {
            // TX/RX glass is on or off. Wake when it should go dark.
            left.max(Duration::from_millis(50))
        } else if self.external_run.iter().any(|run| run.occupied()) {
            Duration::from_millis(50)
        } else {
            // Half a second so a packet, the COM glass, and a new-chat flash
            // are not held for the clock. The hand still moves on the second.
            idle_monitor_repaint(Local::now().timestamp_subsec_millis())
        };
        ctx.request_repaint_after(repaint);
        self.poll_hotkeys(ctx);

        // Dialogs share the QRG/session-bar grey; the main chrome paints its own colours.
        if self.popup_chrome_open() {
            ctx.set_visuals(popup_visuals());
            apply_popup_text_style(ctx, true);
        } else {
            ctx.set_visuals(egui::Visuals::dark());
            apply_popup_text_style(ctx, false);
        }

        // Panels own reserved bands so CentralPanel (RX) cannot paint over them.
        // Each art pixel is a 2×2 block, so the one-pixel lettering stays even.
        let ppp = ctx.pixels_per_point();
        let icons_h = icon_strip_height(ppp);
        let min_w = icon_row_min_width(ppp, ICON_TOOLBAR_SLOTS);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
            min_w, 700.0,
        )));
        let session_h = if self.show_session_bar() { 26.0 } else { 0.0 };
        // Idle: 2 magenta rows beside the clock. Transfer adds a full-width
        // strip under the clock (thin path/bytes/status + taller bar/times).
        let status_row_h = 24.0_f32;
        let status_idle_h = status_row_h * 2.0;
        let path_h = 18.0_f32;
        // Two time cells beside the bar; each +3px vs a 15px half-row.
        let time_cell_h = 18.0_f32;
        let bar_h = time_cell_h * 2.0;
        let transfer_extra_h = if self.transfer_row_open() {
            path_h + bar_h
        } else {
            0.0
        };
        let status_h = status_idle_h + transfer_extra_h;
        // Channel / BBS / Box row is 20% shorter than the old 72px grid.
        let channels_h = 72.0_f32 * 0.8;
        // Slim column — dial stays centered with little side air.
        let clock_w = 156.0_f32;
        // Header ends at the bottom of the "GP V…" badge, not in the green under it.
        // Same badge math as `draw_analog_clock` (width-sized dial, then the two badges).
        let badge_h = 18.0_f32;
        let badge_gap = 4.0_f32;
        let radius = ((clock_w - 12.0) * 0.5 - 1.0).clamp(36.0, 70.0);
        // Same kind of air the dial has under the top border.
        let under_badge = 8.0_f32;
        let clock_bottom = 12.0 + radius * 2.0 + 8.0 + badge_h * 2.0 + badge_gap + under_badge;
        // Clock height is locked to the idle 2-row status — transfer grows under it.
        let prewrite_h = (clock_bottom - channels_h - status_idle_h).max(80.0);
        let top_h = prewrite_h + channels_h + status_h;

        egui::TopBottomPanel::top("gp_top")
            .exact_height(top_h)
            .show_separator_line(false)
            .frame(Frame::none().fill(self.menu_bg))
            .show(ctx, |ui| {
                if self.icon_editor.is_some() {
                    ui.disable();
                }
                self.top_stack_with_clock(
                    ui,
                    ui.available_width(),
                    prewrite_h,
                    channels_h,
                    status_idle_h,
                    transfer_extra_h,
                    path_h,
                    bar_h,
                    clock_w,
                );
            });

        egui::TopBottomPanel::bottom("gp_icons")
            .exact_height(icons_h)
            .show_separator_line(false)
            .frame(Frame::none().fill(self.menu_bg))
            .show(ctx, |ui| {
                if self.icon_editor.is_some() {
                    ui.disable();
                }
                let h = ui.available_height();
                let w = ui.available_width();
                self.icon_strip(ui, w, h);
            });

        // Body is everything between the top stack and the session strip.
        // The monitor, when split, is taken out of that body and drawn under
        // the session bar — the same order as the DOS channel screenshot.
        let body_h = (ctx.screen_rect().height() - top_h - icons_h - session_h).max(0.0);
        self.split_body_h = body_h;
        let monitor_h = if self.split_visible() {
            split_monitor_height(body_h, self.split_lower)
        } else {
            0.0
        };
        if monitor_h > 0.0 {
            let mon_bg = self.monitor_bg;
            let stick = !self.session.scr_lock;
            let smooth = self.session.softscroll_for(SLOT_MONITOR);
            let show_bar = self.session.scrollbar;
            egui::TopBottomPanel::bottom("gp_split_monitor")
                .exact_height(monitor_h)
                .show_separator_line(false)
                .frame(Frame::none().fill(mon_bg))
                .show(ctx, |ui| {
                    if self.icon_editor.is_some() {
                        ui.disable();
                    }
                    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                    paint_text_pane(
                        ui,
                        rect,
                        mon_bg,
                        "rx_split_monitor",
                        stick,
                        smooth,
                        pane_scroll_owner(ActivePane::Monitor),
                        show_bar,
                        0.0,
                        false,
                        self.pad_font_px(),
                        |ui| {
                            let cols = text_columns(ui, self.pad_font_px());
                            self.monitor_layout_for(cols)
                        },
                    );
                    self.note_split_right_click(ui, rect);
                });
        }

        if self.show_session_bar() {
            egui::TopBottomPanel::bottom("gp_session")
                .exact_height(session_h)
                .show_separator_line(false)
                .frame(Frame::none().fill(VGA_GRAY))
                .show(ctx, |ui| {
                    if self.icon_editor.is_some() {
                        ui.disable();
                    }
                    self.session_bar(ui, ui.available_width(), ui.available_height());
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::none().fill(VGA_BLUE))
            .show(ctx, |ui| {
                if self.icon_editor.is_some() {
                    ui.disable();
                }
                let panel = ui.max_rect();
                self.rx_pane(ui, panel.width(), panel.height());
                if self.icon_editor.is_some() {
                    return;
                }
                self.draw_run_terminal(ui, panel);
                self.qrg_dialog(ctx);
                self.mycall_dialog(ctx);
                self.spy_dialog(ctx);
                self.mheard_spy_confirm_dialog(ctx);
                self.connect_dialog(ctx);
                self.connect_names_window(ctx);
                self.name_dialog(ctx);
            });
        if self.icon_editor.is_none() {
            self.paint_setup(ctx);
        }
        let icon_editor_open = self.icon_editor.is_some();
        self.paint_icon_editor(ctx);
        if icon_editor_open {
            self.split_dragging = false;
            self.split_drag_dy = 0.0;
            self.sync_console_font(ctx);
            return;
        }
        self.paint_tnc_command(ctx);
        self.paint_list_offer(ctx);
        self.paint_autobin_offer(ctx);
        self.paint_find(ctx);
        self.paint_start_screen(ctx);
        if !matches!(self.active, ActivePane::Channel(_)) {
            self.split_dragging = false;
            self.split_drag_dy = 0.0;
        } else if self.split_dragging && self.split_drag_dy.abs() > SPLIT_DRAG_SLOP {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
            ctx.request_repaint();
        }
        // A Zoom click or ALT-Z lands during this frame, after the opening sync.
        self.sync_console_font(ctx);
    }
}

impl GpGuiApp {
    /// Logo up top, radio card under it. Use this opens the link and drops both.
    fn paint_start_screen(&mut self, ctx: &Context) {
        if !self.show_radio_gate {
            return;
        }
        self.refresh_radio_ports();
        let before = self.radio_form_key();
        let screen = ctx.screen_rect();
        let tfpcx = self.radio_protocol == HostProtocol::Tfpcx;
        // The short card hangs under the picture and stays clear of the credits.
        // TFPCX adds the modem columns, so the picture gives up that extra height
        // and the card moves up with it. The other three protocols keep the
        // original anchor.
        let card_h = if tfpcx {
            self.start_card_height.max(START_TFPCX_RESERVE)
        } else {
            START_CARD_RESERVE
        };
        let logo_bottom = self.paint_start_logo(ctx, screen, card_h);
        let mut use_this = false;
        let wide = tfpcx;
        let width = if wide { 760.0 } else { 460.0 };
        let natural = (logo_bottom + 12.0).min((screen.bottom() - 28.0).max(screen.top() + 8.0));
        let y = if tfpcx {
            let above_credits = screen.bottom() - START_CREDITS_H - START_CARD_CREDITS_GAP - card_h;
            natural.min(above_credits).max(screen.top() + 8.0)
        } else {
            natural
        };
        let mut open = true;
        let shown = egui::Window::new("Radio setup")
            .id(egui::Id::new("gp_radio_setup"))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(Pos2::new(screen.center().x, y))
            .default_width(width)
            .min_width(width)
            .max_width(width)
            .frame(popup_frame())
            .open(&mut open)
            .show(ctx, |ui| {
                use_this = self.radio_setup_form(ui);
            });
        if let Some(shown) = &shown {
            paint_menu_border(ctx, &shown.response);
            if tfpcx {
                self.start_card_height = shown.response.rect.height();
            }
        }
        if !open {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.radio_form_key() != before {
            let saved = self.store_radio_form();
            self.note_radio_save(saved);
        }
        if use_this {
            let saved = self.store_radio_form();
            self.note_radio_save(saved);
            if self.radio_error.is_empty() {
                if let Some(msg) = locator_blocks_start(&self.radio_locator) {
                    self.radio_error = msg;
                } else {
                    self.open_chosen_radio();
                }
            }
        }
        if self.show_radio_gate {
            self.paint_radio_pick(ctx);
            egui::Area::new(egui::Id::new("gp_start_credits"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .interactable(false)
                .show(ctx, |ui| {
                    paint_start_credits(ui.painter(), screen);
                });
        }
    }

    /// Monitor-blue splash with the welcome picture moved up, leaving room for the card.
    fn paint_start_logo(&self, ctx: &Context, screen: Rect, reserve: f32) -> f32 {
        let margin = 16.0;
        let mut logo_bottom = screen.top() + margin;
        egui::Area::new(egui::Id::new("gp_start_screen"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                let _ = ui.interact(screen, ui.id().with("start_hit"), Sense::click_and_drag());
                ui.painter().rect_filled(screen, 0.0, self.monitor_bg);
                if let Some(tex) = self.start_screen.as_ref() {
                    let ppp = ctx.pixels_per_point().max(1.0);
                    let tex_size = tex.size_vec2() / ppp;
                    if tex_size.x > 0.0 && tex_size.y > 0.0 {
                        let max_h =
                            (screen.height() - margin * 2.0 - 12.0 - reserve - START_CREDITS_H)
                                .max(72.0);
                        let max_w = (screen.width() - margin * 2.0).max(72.0);
                        let scale = (max_w / tex_size.x).min(max_h / tex_size.y).min(1.0);
                        let draw = tex_size * scale;
                        let img = Rect::from_min_size(
                            Pos2::new(screen.center().x - draw.x * 0.5, screen.top() + margin),
                            draw,
                        );
                        ui.painter().image(
                            tex.id(),
                            img,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        logo_bottom = img.bottom();
                    }
                }
            });
        logo_bottom
    }

    fn tfpcx_link_rows(&mut self, ui: &mut Ui, form_w: f32) {
        let col_w = ((form_w - 16.0) / 3.0).max(200.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for index in 0..self.radio_links.len().min(3) {
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    self.tfpcx_link_column(ui, index, col_w);
                });
            }
        });
    }

    fn tfpcx_link_column(&mut self, ui: &mut Ui, index: usize, row_w: f32) {
        let font = menu_font(15.0);
        let ports = self.radio_ports.clone();
        ui.label(
            RichText::new(format!("Modem {}", index + 1))
                .color(Color32::BLACK)
                .font(menu_font(16.0)),
        );
        let kind = link_kind_label(&self.radio_links[index].protocol);
        if menu_open(ui, "Type", kind, row_w) {
            self.cycle_link_kind(index);
        }
        if self.radio_links[index].protocol == "off" {
            return;
        }
        let kiss = self.radio_links[index].protocol == "kiss";
        if kiss {
            let how = if self.radio_links[index].tcp {
                "TCP"
            } else {
                "Serial"
            };
            if menu_open(ui, "Link", how, row_w) {
                self.radio_link = Some(index);
                self.toggle_radio_pick(RadioPick::Connection);
            }
        }
        let tcp = kiss && self.radio_links[index].tcp;
        if tcp {
            ui.label(
                RichText::new("IP address")
                    .color(Color32::BLACK)
                    .font(font.clone()),
            );
            retro::field(
                ui,
                &mut self.radio_links[index].host,
                row_w,
                "",
                font.clone(),
            );
            ui.label(
                RichText::new("TCP port")
                    .color(Color32::BLACK)
                    .font(font.clone()),
            );
            retro::field(
                ui,
                &mut self.radio_links[index].tcp_port,
                row_w,
                "",
                font.clone(),
            );
        } else {
            let selected = self.radio_links[index].port_name.clone();
            let port_value = if ports.is_empty() {
                "No port found".to_string()
            } else {
                port_menu_label(&ports, &selected)
            };
            if menu_open(ui, "Port", &port_value, row_w) && !ports.is_empty() {
                self.radio_link = Some(index);
                self.toggle_radio_pick(RadioPick::Port);
            }
            let baud = self.radio_links[index].baud.to_string();
            if menu_open(ui, "Baud", &baud, row_w) {
                self.radio_link = Some(index);
                self.toggle_radio_pick(RadioPick::Baud);
            }
        }
        let chosen = self.radio_links[index].ports.clone();
        ui.label(
            RichText::new("Ports")
                .color(Color32::BLACK)
                .font(font.clone()),
        );
        paint_port_grid(ui, &chosen, row_w, &font, |port| {
            self.toggle_link_port(index, port);
        });
    }

    fn cycle_link_kind(&mut self, index: usize) {
        let next = match self.radio_links[index].protocol.as_str() {
            "off" => "kiss",
            "kiss" => "wa8ded",
            _ => "off",
        };
        self.radio_links[index].protocol = next.into();
        if next == "wa8ded" {
            self.radio_links[index].tcp = false;
            let keep = self.radio_links[index].ports.first().copied();
            self.radio_links[index].ports.clear();
            if let Some(port) = keep.or_else(|| self.free_port()) {
                self.radio_links[index].ports = vec![port];
            }
        } else if next == "kiss" && self.radio_links[index].ports.is_empty() {
            for _ in 0..2 {
                if let Some(port) = self.free_port() {
                    self.radio_links[index].ports.push(port);
                }
            }
        }
        self.radio_error.clear();
    }

    fn toggle_link_port(&mut self, index: usize, port: u8) {
        let kiss = self.radio_links[index].protocol == "kiss";
        if self.radio_links[index].ports.contains(&port) {
            self.radio_links[index].ports.retain(|have| *have != port);
            self.radio_error.clear();
            return;
        }
        if self.port_taken(index, port) {
            self.radio_error = format!("Port {port} is already used");
            return;
        }
        self.radio_error.clear();
        if kiss {
            self.radio_links[index].ports.push(port);
            self.radio_links[index].ports.sort_unstable();
        } else {
            self.radio_links[index].ports = vec![port];
        }
    }

    fn port_taken(&self, index: usize, port: u8) -> bool {
        self.radio_links.iter().enumerate().any(|(other, link)| {
            other != index && link.protocol != "off" && link.ports.contains(&port)
        })
    }

    fn free_port(&self) -> Option<u8> {
        (0..=crate::config::TncConfig::TFPCX_MAX_PORT)
            .find(|port| !self.port_taken(usize::MAX, *port))
    }

    fn radio_setup_form(&mut self, ui: &mut Ui) -> bool {
        let tfpcx = self.radio_protocol == HostProtocol::Tfpcx;
        let form_w = if tfpcx { 740.0 } else { retro::ROW_W };
        ui.set_width(form_w);
        ui.spacing_mut().item_spacing.y = 4.0;
        let row_w = form_w;
        let font = menu_font(17.0);
        let ports = self.radio_ports.clone();

        clip_locator_edit(&mut self.radio_locator);
        let gap = 8.0;
        let label_font = menu_font(16.0);
        let loc_text = ui.fonts(|fonts| {
            fonts
                .layout_no_wrap("JO32MW49".to_string(), font.clone(), Color32::BLACK)
                .size()
                .x
        });
        // Well inset plus the text edit's side margins, so eight characters fit.
        let loc_w = loc_text + 24.0;
        let loc_label = ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(
                    "QTH Locator".to_string(),
                    label_font.clone(),
                    Color32::BLACK,
                )
                .size()
                .x
        });
        let loc_col = loc_label.max(loc_w);
        let call_w = (row_w - gap - loc_col).max(120.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.vertical(|ui| {
                ui.set_width(call_w);
                ui.label(
                    RichText::new("Call sign")
                        .color(Color32::BLACK)
                        .font(label_font.clone()),
                );
                retro::field(ui, &mut self.radio_call, call_w, "", font.clone());
            });
            ui.vertical(|ui| {
                ui.set_width(loc_col);
                ui.label(
                    RichText::new("QTH Locator")
                        .color(Color32::BLACK)
                        .font(label_font),
                );
                retro::field_fixed(
                    ui,
                    &mut self.radio_locator,
                    loc_w,
                    "JO32KX",
                    Color32::from_rgb(168, 168, 168),
                    8,
                    font.clone(),
                );
            });
        });
        clip_locator_edit(&mut self.radio_locator);
        if let Some(msg) = locator_field_warning(&self.radio_locator) {
            ui.label(
                RichText::new(msg)
                    .color(Color32::from_rgb(140, 0, 0))
                    .font(menu_font(16.0)),
            );
        }

        let protocol = protocol_label(self.radio_protocol).to_string();
        if menu_open(ui, "Protocol", &protocol, row_w) {
            self.toggle_radio_pick(RadioPick::Protocol);
        }

        if self.radio_protocol == HostProtocol::Tfpcx {
            self.tfpcx_link_rows(ui, form_w);
        } else if self.radio_protocol == HostProtocol::Kiss {
            let how = if self.radio_tcp { "TCP" } else { "Serial" };
            if menu_open(ui, "Connection", how, row_w) {
                self.radio_link = None;
                self.toggle_radio_pick(RadioPick::Connection);
            }
        }

        let serial = self.radio_protocol != HostProtocol::Tfpcx
            && (self.radio_protocol != HostProtocol::Kiss || !self.radio_tcp);
        if serial {
            let port_value = if ports.is_empty() {
                "No port found".to_string()
            } else {
                port_menu_label(&ports, &self.radio_port)
            };
            if menu_open(ui, "Port", &port_value, row_w) && !ports.is_empty() {
                self.radio_link = None;
                self.toggle_radio_pick(RadioPick::Port);
            }
            let baud = self.radio_baud.to_string();
            let label = if self.radio_protocol == HostProtocol::Baycom {
                "Radio bit rate"
            } else {
                "Baud to TNC"
            };
            if menu_open(ui, label, &baud, row_w) {
                self.radio_link = None;
                self.toggle_radio_pick(RadioPick::Baud);
            }
        } else if self.radio_protocol == HostProtocol::Kiss {
            ui.label(
                RichText::new("IP address")
                    .color(Color32::BLACK)
                    .font(menu_font(16.0)),
            );
            retro::field(ui, &mut self.radio_host, row_w, "", font.clone());
            ui.label(
                RichText::new("TCP port")
                    .color(Color32::BLACK)
                    .font(menu_font(16.0)),
            );
            retro::field(ui, &mut self.radio_tcp_port, row_w, "", font);
        }

        let warn = Color32::from_rgb(140, 0, 0);
        if self.radio_protocol == HostProtocol::Baycom {
            ui.label(
                RichText::new(crate::tnc::baycom::EXPERIMENTAL_WARNING)
                    .color(warn)
                    .font(menu_font(16.0)),
            );
        }
        if serial && self.radio_live_ports == 0 {
            ui.label(
                RichText::new(
                    "No serial port found yet. Plug in the radio and it will show up here.",
                )
                .color(warn)
                .font(menu_font(16.0)),
            );
        } else if serial && self.radio_port.is_empty() && !self.radio_saved_port.is_empty() {
            let still_there = self
                .radio_ports
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(&self.radio_saved_port));
            let msg = if still_there {
                format!("{} changed. Choose the port again.", self.radio_saved_port)
            } else {
                format!(
                    "{} is not connected. Choose a port that is.",
                    self.radio_saved_port
                )
            };
            ui.label(RichText::new(msg).color(warn).font(menu_font(16.0)));
        }
        if !self.radio_error.is_empty() {
            ui.label(
                RichText::new(&self.radio_error)
                    .color(Color32::from_rgb(160, 0, 0))
                    .font(menu_font(16.0)),
            );
        }
        retro::apply(ui, false, menu_font(20.0))
    }

    fn refresh_radio_ports(&mut self) {
        if self
            .radio_ports_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.radio_ports_at = Some(Instant::now());
        let mut live = detect_serial_ports();
        if self.radio_protocol == HostProtocol::Baycom {
            match crate::tnc::detect_baycom_ports() {
                Ok(ports) => live.extend(ports),
                Err(e) => {
                    self.radio_error = format!("{e}; see docs/BAYCOM.md for driver setup");
                }
            }
        }
        self.radio_live_ports = live.len();
        self.radio_ports = live;
        self.reconcile_saved_port();
    }

    /// Keep the last saved port when that same device is still present.
    /// A missing port, or a different device on the same name, is not selected.
    fn reconcile_saved_port(&mut self) {
        if let Some(current) = self
            .radio_ports
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&self.radio_port))
        {
            let saved_here = current.name.eq_ignore_ascii_case(&self.radio_saved_port);
            if saved_here && !port_identity_matches(&self.radio_saved_detail, &current.detail) {
                self.radio_port.clear();
            }
            return;
        }
        if let Some(saved) = self
            .radio_ports
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&self.radio_saved_port))
        {
            if port_identity_matches(&self.radio_saved_detail, &saved.detail) {
                self.radio_port = saved.name.clone();
                return;
            }
        }
        self.radio_port.clear();
    }

    fn radio_form_key(
        &self,
    ) -> (
        String,
        String,
        HostProtocol,
        bool,
        String,
        u32,
        String,
        String,
        String,
    ) {
        (
            self.radio_call.clone(),
            self.radio_locator.clone(),
            self.radio_protocol,
            self.radio_tcp,
            self.radio_port.clone(),
            self.radio_baud,
            self.radio_host.clone(),
            self.radio_tcp_port.clone(),
            self.radio_links
                .iter()
                .map(|link| {
                    format!(
                        "{}:{:?}:{}:{}:{}:{}:{}",
                        link.protocol,
                        link.ports,
                        link.tcp,
                        link.port_name,
                        link.baud,
                        link.host,
                        link.tcp_port
                    )
                })
                .collect(),
        )
    }

    fn note_radio_save(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.radio_error.clear(),
            Err(e) => self.radio_error = e,
        }
    }

    /// Write `station.locator` when the field is empty or a finished Maidenhead value.
    /// A half-typed grid stays on the card and is not written yet.
    fn sync_radio_locator(&mut self) -> Result<(), String> {
        let raw = self.radio_locator.trim();
        let next = if raw.is_empty() {
            String::new()
        } else if let Some(loc) = crate::locator::normalize_locator(raw) {
            loc
        } else {
            return Ok(());
        };
        if next == self.cfg.station.locator {
            return Ok(());
        }
        self.cfg.station.locator = next.clone();
        self.cfg.persist_locator(&self.config_path)?;
        if !next.is_empty() {
            self.radio_locator = next.clone();
            self.map_view.fit_locator(&next);
        }
        Ok(())
    }

    /// Copy the card into `cfg` and rewrite Config.toml. Invalid fields are left unsaved.
    fn store_radio_form(&mut self) -> Result<(), String> {
        let call = normalize_mycall(self.radio_call.trim())?;
        self.sync_radio_locator()?;
        self.apply_radio_form_to_cfg()?;
        let call_changed = !call.eq_ignore_ascii_case(self.cfg.primary_mycall());
        if call_changed {
            self.cfg.set_mycall_for_slot(0, &call)?;
            for slot in self.cfg.station.mycall.iter_mut().skip(1) {
                slot.clear();
            }
            self.cfg.persist_mycall(&self.config_path)?;
        }
        self.cfg.persist_radio(&self.config_path)
    }

    fn apply_radio_form_to_cfg(&mut self) -> Result<(), String> {
        if self.radio_protocol == HostProtocol::Baycom && self.radio_baud != 1200 {
            return Err("Baycom supports only a 1200 bit/s radio rate".into());
        }
        let protocol = match self.radio_protocol {
            HostProtocol::Wa8ded => "wa8ded",
            HostProtocol::Tfpcx => "tfpcx",
            HostProtocol::Kiss => "kiss",
            HostProtocol::Baycom => "baycom",
        };
        if self.radio_protocol == HostProtocol::Tfpcx {
            self.cfg.tnc.links = self
                .radio_links
                .iter()
                .map(|link| crate::config::TncLink {
                    protocol: link.protocol.clone(),
                    ports: link.ports.clone(),
                    transport: if link.tcp { "tcp" } else { "serial" }.into(),
                    serial_port: link.port_name.clone(),
                    baud: link.baud,
                    tcp_host: link.host.clone(),
                    tcp_port: link.tcp_port.trim().parse().unwrap_or(0),
                })
                .collect();
            self.cfg.tnc.validated_links()?;
            self.cfg.capture_own_calls();
        }
        let tcp = self.radio_protocol == HostProtocol::Kiss && self.radio_tcp;
        if self.radio_protocol == HostProtocol::Tfpcx {
            // Each link carries its own serial port or TCP address.
        } else if tcp {
            let host = self.radio_host.trim();
            if host.is_empty() {
                return Err("Enter the IP address".into());
            }
            let port: u16 = self
                .radio_tcp_port
                .trim()
                .parse()
                .map_err(|_| "TCP port needs a number (1–65535)".to_string())?;
            if port == 0 {
                return Err("TCP port needs a number (1–65535)".into());
            }
            self.cfg.tnc.kiss_tcp_host = host.to_string();
            self.cfg.tnc.kiss_tcp_port = port;
            self.cfg.tnc.kiss_transport = "tcp".into();
        } else {
            let port_name = self.radio_port.trim().to_string();
            if port_name.is_empty() {
                return Err("Choose a serial port".into());
            }
            if self.radio_baud == 0 {
                return Err("Choose a baud rate".into());
            }
            if let Some(found) = self
                .radio_ports
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(&port_name))
            {
                self.cfg.serial.port = found.name.clone();
                self.cfg.serial.port_detail = found.detail.clone();
                self.radio_saved_port = found.name.clone();
                self.radio_saved_detail = found.detail.clone();
                self.radio_port = found.name.clone();
            } else {
                self.cfg.serial.port = port_name.to_string();
            }
            self.cfg.serial.baud = self.radio_baud;
            if let Some(n) = com_number_from_port(&port_name) {
                self.cfg.serial.com_number = n;
            }
            if self.radio_protocol == HostProtocol::Kiss {
                self.cfg.tnc.kiss_transport = "serial".into();
            }
        }
        self.cfg.tnc.protocol = protocol.into();
        Ok(())
    }

    fn note_modem(&mut self, index: u8, label: String, phase: LinkPhase) {
        let slot = usize::from(index);
        if self.modems.len() <= slot {
            self.modems
                .resize_with(slot + 1, || ModemFace::new(String::new()));
        }
        let was_open = self.serial_open;
        let face = &mut self.modems[slot];
        if !label.is_empty() {
            face.label = label;
        }
        face.phase = phase;
        if phase != LinkPhase::Open {
            face.tx_until = None;
            face.rx_until = None;
        }
        self.serial_open = self.modems.iter().any(|face| face.phase == LinkPhase::Open);
        if self.serial_open && !was_open {
            self.sync_channel_identities();
        }
    }

    fn open_chosen_radio(&mut self) {
        apply_configured_paclen(&mut self.cfg);
        if !self.radio.is_open() {
            let (events, cmd_tx) = self
                .radio
                .open(self.cfg.serial.clone(), self.cfg.tnc.clone());
            self.events = events;
            self.cmd_tx = cmd_tx;
        }
        self.modems = modem_faces(&self.cfg);
        self.serial_open = false;
        let how = self
            .modems
            .iter()
            .map(|face| face.label.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        self.status = format!(
            "Opening {} on {how}",
            protocol_label(self.cfg.tnc.protocol_kind())
        );
        let call = self.cfg.primary_mycall().to_string();
        if !call.eq_ignore_ascii_case(&self.radio_call_loaded) {
            self.pads[0].push(PadKind::System, version::startup_local_call(&call));
        }
        self.show_radio_gate = false;
        self.radio_pick = None;
        self.start_screen = None;
    }

    fn toggle_radio_pick(&mut self, pick: RadioPick) {
        self.radio_pick = if self.radio_pick == Some(pick) {
            None
        } else {
            Some(pick)
        };
    }

    /// Second panel. A click writes the choice back onto the radio card.
    fn paint_radio_pick(&mut self, ctx: &Context) {
        let Some(pick) = self.radio_pick else {
            return;
        };
        let ports = self.radio_ports.clone();
        let title = match pick {
            RadioPick::Protocol => "Protocol",
            RadioPick::Connection => "Connection",
            RadioPick::Port => "Port",
            RadioPick::Baud if self.radio_protocol == HostProtocol::Baycom => "Radio bit rate",
            RadioPick::Baud => "Baud to TNC",
        };
        let options: Vec<String> = match pick {
            RadioPick::Protocol => RADIO_PROTOCOLS
                .into_iter()
                .map(|kind| protocol_label(kind).to_string())
                .collect(),
            RadioPick::Connection => vec!["Serial".into(), "TCP".into()],
            RadioPick::Port => {
                if ports.is_empty() {
                    vec!["No port found".into()]
                } else {
                    ports.iter().map(DetectedPort::menu_label).collect()
                }
            }
            RadioPick::Baud => baud_choices(self.radio_baud, self.radio_protocol)
                .into_iter()
                .map(|n| n.to_string())
                .collect(),
        };
        let link = self.radio_link.and_then(|i| self.radio_links.get(i));
        let current = match pick {
            RadioPick::Protocol => protocol_label(self.radio_protocol).to_string(),
            RadioPick::Connection => {
                let tcp = link.map(|link| link.tcp).unwrap_or(self.radio_tcp);
                if tcp {
                    "TCP".into()
                } else {
                    "Serial".into()
                }
            }
            RadioPick::Port => {
                let selected = link
                    .map(|link| link.port_name.as_str())
                    .unwrap_or(&self.radio_port);
                port_menu_label(&ports, selected)
            }
            RadioPick::Baud => link
                .map(|link| link.baud.to_string())
                .unwrap_or_else(|| self.radio_baud.to_string()),
        };
        let mut open = true;
        let mut chosen: Option<usize> = None;
        centered_popup(title, egui::Id::new("gp_radio_pick"), &mut open)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.spacing_mut().item_spacing.y = 3.0;
                if options.is_empty() {
                    ui.label(RichText::new("Nothing to choose").font(menu_font(16.0)));
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            let width = ui.available_width().max(280.0);
                            for (i, opt) in options.iter().enumerate() {
                                let selected = opt == &current
                                    || (pick == RadioPick::Port
                                        && ports.get(i).is_some_and(|p| {
                                            p.name.eq_ignore_ascii_case(&self.radio_port)
                                        }));
                                let row = retro::entry(
                                    ui,
                                    opt,
                                    "",
                                    Color32::TRANSPARENT,
                                    selected,
                                    width,
                                    menu_font(17.0),
                                );
                                if row.clicked() && !(pick == RadioPick::Port && ports.is_empty()) {
                                    chosen = Some(i);
                                }
                            }
                        });
                }
            });
        if let Some(i) = chosen {
            self.apply_radio_pick(pick, i, &ports);
            self.radio_pick = None;
        } else if !open {
            self.radio_pick = None;
        }
    }

    fn apply_radio_pick(&mut self, pick: RadioPick, index: usize, ports: &[DetectedPort]) {
        match pick {
            RadioPick::Protocol => {
                if let Some(kind) = RADIO_PROTOCOLS.get(index) {
                    self.radio_protocol = *kind;
                    self.radio_ports_at = None;
                    if *kind == HostProtocol::Baycom {
                        self.radio_baud = 1200;
                    }
                }
            }
            RadioPick::Connection => {
                if let Some(link) = self.radio_link.and_then(|i| self.radio_links.get_mut(i)) {
                    link.tcp = index == 1;
                } else {
                    self.radio_tcp = index == 1;
                }
            }
            RadioPick::Port => {
                if let Some(port) = ports.get(index) {
                    if let Some(link) = self.radio_link.and_then(|i| self.radio_links.get_mut(i)) {
                        link.port_name = port.name.clone();
                    } else {
                        self.radio_port = port.name.clone();
                    }
                }
            }
            RadioPick::Baud => {
                let rates = baud_choices(self.radio_baud, self.radio_protocol);
                if let Some(rate) = rates.get(index) {
                    if let Some(link) = self.radio_link.and_then(|i| self.radio_links.get_mut(i)) {
                        link.baud = *rate;
                    } else {
                        self.radio_baud = *rate;
                    }
                }
            }
        }
    }
}

impl GpGuiApp {
    /// Left: purple pre-write → channels → 2-row magenta status.
    /// Right: clock locked to that idle height. Transfer path/bar rows are
    /// full-width under the clock (they do not stretch the clock column).
    fn top_stack_with_clock(
        &mut self,
        ui: &mut Ui,
        width: f32,
        prewrite_h: f32,
        channels_h: f32,
        status_idle_h: f32,
        transfer_extra_h: f32,
        path_h: f32,
        bar_h: f32,
        clock_w: f32,
    ) {
        let height = prewrite_h + channels_h + status_idle_h + transfer_extra_h;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

        // Absolute split so the clock column cannot be pushed off-screen by
        // a greedy left pane (that was clipping the dial on the right).
        let clock_w = clock_w.min(rect.width() * 0.35).max(120.0);
        let left_r = rect.right() - clock_w;
        let status_top = rect.top() + prewrite_h + channels_h;
        let clock_bottom = status_top + status_idle_h;
        let left = Rect::from_min_max(rect.left_top(), Pos2::new(left_r, clock_bottom));
        let pre_r = Rect::from_min_max(rect.left_top(), Pos2::new(left_r, rect.top() + prewrite_h));
        let ch_r = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + prewrite_h),
            Pos2::new(left_r, status_top),
        );
        let st_r = Rect::from_min_max(
            Pos2::new(rect.left(), status_top),
            Pos2::new(left_r, clock_bottom),
        );
        // Full-width strip under the clock (path/bytes/status + bar/times).
        let xfer_r = if transfer_extra_h > 0.0 {
            Some(Rect::from_min_max(
                Pos2::new(rect.left(), clock_bottom),
                Pos2::new(rect.right(), rect.bottom()),
            ))
        } else {
            None
        };
        let clock_rect = Rect::from_min_max(
            Pos2::new(left_r, rect.top()),
            Pos2::new(rect.right(), clock_bottom),
        );

        // Channel chrome uses the menu-window background, same as the icon strip.
        // Clock column stays dark green and does not grow with the transfer strip.
        ui.painter_at(left).rect_filled(left, 0.0, self.menu_bg);
        ui.painter_at(clock_rect)
            .rect_filled(clock_rect, 0.0, self.clock_bg);

        self.draw_prewrite(ui, pre_r);
        self.draw_channel_grid(ui, ch_r);
        if let (Some(xfer), Some(view)) = (xfer_r, self.transfer_view()) {
            self.draw_transfer80_head(ui, st_r);
            self.draw_transfer80_foot(ui, xfer, &view, path_h, bar_h);
        } else {
            self.draw_status_grid(ui, st_r);
        }
        if draw_analog_clock(ui, clock_rect, self.session.hour_gong, self.clock_bg) {
            self.session.hour_gong = !self.session.hour_gong;
            self.status = format!(
                "Hourly gong {}",
                if self.session.hour_gong { "ON" } else { "OFF" }
            );
            self.persist_session_chrome();
            if self.session.hour_gong {
                self.play_hour_chime(Local::now().hour());
            }
        }
    }

    fn compose_focus_id(&self) -> Option<egui::Id> {
        self.compose_slot_idx().map(compose_id_for)
    }

    /// Multi-line paste: send each complete line now; leave a trailing fragment.
    fn absorb_compose_newlines(&mut self) {
        let Some(slot) = self.compose_slot_idx() else {
            return;
        };
        let raw = self
            .compose
            .get(slot)
            .map(|c| c.input.clone())
            .unwrap_or_default();
        if !raw.contains('\n') && !raw.contains('\r') {
            return;
        }
        let normalized = crate::tnc::normalize_newlines(&raw);
        self.dispatch_compose_paste(slot, &normalized);
    }

    /// Delete the history row currently recalled into the compose box.
    fn delete_recalled_history_line(&mut self) -> bool {
        let Some(slot) = self.compose_slot_idx() else {
            return false;
        };
        let Some(hi) = self.compose.get(slot).and_then(|c| c.hist_cursor) else {
            return false;
        };
        let Some(c) = self.compose.get_mut(slot) else {
            return false;
        };
        if hi >= c.history.len() {
            return false;
        }
        c.history.remove(hi);
        if c.history.is_empty() {
            c.hist_cursor = None;
            c.input = std::mem::take(&mut c.hist_stash);
            c.compose_caret_end = true;
            self.status = "history line deleted".into();
            return true;
        }
        let next = hi.min(c.history.len() - 1);
        let line = c.history[next].clone();
        c.hist_cursor = Some(next);
        c.input = line;
        c.compose_caret_end = true;
        self.status = "history line deleted".into();
        true
    }

    fn draw_prewrite(&mut self, ui: &mut Ui, rect: Rect) {
        let enabled = self.compose_enabled();
        let slot = self.compose_slot_idx().unwrap_or(0);
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            // Typing is only on Monitor and a channel: input-window colours.
            // Every other pane uses the menu-window colours and does not take text.
            let (fill, ink) = if enabled {
                (self.input_bg, self.input_fg)
            } else {
                (self.menu_bg, self.menu_fg)
            };
            Frame::none()
                .fill(fill)
                .stroke(Stroke::new(1.0, ink))
                // Thin bottom margin so the compose strip can sit on the purple floor.
                .inner_margin(egui::Margin {
                    left: 4.0,
                    right: 4.0,
                    top: 4.0,
                    bottom: 1.0,
                })
                .show(ui, |ui| {
                    let avail = ui.available_size();
                    ui.set_min_size(avail);
                    ui.set_max_size(avail);
                    // No gap between history and compose — that band looked like a dead line.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if !enabled {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                RichText::new("Input — Monitor or channel only")
                                    .color(ink)
                                    .font(FontId::monospace(15.0)),
                            );
                        });
                        return;
                    }

                    // Flush multi-line paste before layout so history stays one row each.
                    self.absorb_compose_newlines();

                    let compose_font = FontId::monospace(COMPOSE_FONT_SIZE);
                    let row_h = ui.fonts(|f| f.row_height(&compose_font)).max(1.0);
                    let char_w = ui.fonts(|f| f.glyph_width(&compose_font, '_')).max(6.0);
                    let w = ui.available_width();
                    let total_h = ui.available_height();
                    // Height follows wrapped content (1..=COMPOSE_ROWS). Always
                    // reserving 3 rows left empty purple under a short line like "b".
                    let cols = ((w / char_w).floor() as usize).max(1);
                    let chars = self
                        .compose
                        .get(slot)
                        .map(|c| c.input.chars().count())
                        .unwrap_or(0);
                    let wrapped = if chars == 0 {
                        1
                    } else {
                        chars.div_ceil(cols).max(1)
                    };
                    let compose_rows = wrapped.clamp(1, COMPOSE_ROWS);
                    let compose_h = row_h * compose_rows as f32;
                    let hist_h = (total_h - compose_h).max(8.0);
                    let hist_cursor = self.compose.get(slot).and_then(|c| c.hist_cursor);
                    let history: Vec<String> = self
                        .compose
                        .get(slot)
                        .map(|c| {
                            c.history
                                .iter()
                                .map(|l| Self::sanitize_compose_line(l))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut pick: Option<usize> = None;
                    ui.allocate_ui(Vec2::new(w, hist_h), |ui| {
                        ui.set_min_size(Vec2::new(w, hist_h));
                        ui.set_max_size(Vec2::new(w, hist_h));
                        ui.spacing_mut().item_spacing.y = 0.0;
                        egui::ScrollArea::vertical()
                            .max_height(hist_h)
                            .auto_shrink([false, false])
                            .stick_to_bottom(hist_cursor.is_none())
                            .id_salt(("typed_history", slot))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 0.0;
                                for (i, line) in history.iter().enumerate() {
                                    let selected = hist_cursor == Some(i);
                                    let color = if selected {
                                        Color32::from_rgb(255, 255, 85)
                                    } else {
                                        ink
                                    };
                                    let resp = ui.add(
                                        egui::Label::new(
                                            RichText::new(line)
                                                .color(color)
                                                .font(FontId::monospace(14.0)),
                                        )
                                        .sense(Sense::click()),
                                    );
                                    if resp.clicked() {
                                        pick = Some(i);
                                    }
                                    if selected {
                                        resp.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                    });
                    if let Some(i) = pick {
                        self.load_history_line(i);
                    }
                    let focus_id = compose_id_for(slot);
                    // Any click on the purple field focuses the compose line.
                    let in_purple = ui.is_enabled()
                        && ui.input(|i| {
                            i.pointer.any_click()
                                && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
                        });
                    if in_purple {
                        ui.memory_mut(|m| m.request_focus(focus_id));
                    }
                    let caret_end = self
                        .compose
                        .get_mut(slot)
                        .map(|c| {
                            let v = c.compose_caret_end;
                            c.compose_caret_end = false;
                            v
                        })
                        .unwrap_or(false);
                    if caret_end {
                        let len = self
                            .compose
                            .get(slot)
                            .map(|c| c.input.chars().count())
                            .unwrap_or(0);
                        place_compose_caret(ui.ctx(), focus_id, len);
                    }
                    // Hide egui's tall bar caret; we paint a DOS `_` after show().
                    ui.visuals_mut().text_cursor.stroke = Stroke::new(0.0, Color32::TRANSPARENT);
                    // Exact rows×row_h flush to the purple floor (history fills above).
                    let before_ime = self.compose[slot].input.clone();
                    let output = {
                        let input_ref = &mut self.compose[slot].input;
                        ui.allocate_ui_with_layout(
                            Vec2::new(w, compose_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_size(Vec2::new(w, compose_h));
                                ui.set_max_size(Vec2::new(w, compose_h));
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                egui::TextEdit::multiline(input_ref)
                                    .id(focus_id)
                                    .desired_rows(compose_rows)
                                    .desired_width(w)
                                    .frame(false)
                                    .margin(egui::Margin::ZERO)
                                    .text_color(ink)
                                    .font(compose_font)
                                    .show(ui)
                            },
                        )
                        .inner
                    };
                    // `°` and emoji arrive as an IME commit. egui 0.29 keeps that
                    // commit only while the caret is still where IME was enabled,
                    // so typing `4.8` first makes the degree sign vanish.
                    if output.response.has_focus() {
                        let commit = ui.input(|i| dropped_ime_commit(&i.events));
                        if !commit.is_empty() && self.compose[slot].input == before_ime {
                            let cursor = output
                                .cursor_range
                                .map(|r| r.primary.ccursor.index)
                                .unwrap_or_else(|| before_ime.chars().count());
                            let at = insert_at_char(&mut self.compose[slot].input, cursor, &commit);
                            place_compose_caret(ui.ctx(), focus_id, at);
                        }
                    }
                    let changed = output.response.changed();
                    let focused = output.response.has_focus();
                    if focused {
                        if let Some(range) = output.cursor_range {
                            paint_dos_caret(
                                ui,
                                output.galley_pos,
                                output.galley.as_ref(),
                                &range.primary,
                                row_h,
                                char_w,
                                ink,
                            );
                        }
                    }
                    if changed {
                        // Paste of a document lands as newlines; send lines immediately.
                        self.absorb_compose_newlines();
                    }
                    if focused
                        && !self.show_setup
                        && !self.show_tnc
                        && !self.find_open
                        && self.list_offer.is_none()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        let mods = ui.input(|i| i.modifiers);
                        // ALT-Enter is fullscreen; Shift/Ctrl-Enter have their own jobs.
                        if !mods.alt && !mods.shift && !mods.ctrl {
                            if let Some(c) = self.compose.get_mut(slot) {
                                if c.input.ends_with('\n') {
                                    c.input.pop();
                                }
                            }
                            self.submit();
                        }
                    }
                });
        });
    }

    /// `older` is Arrow Up (toward earlier lines). Arrow Down walks back.
    fn recall_history(&mut self, older: bool) {
        let Some(slot) = self.compose_slot_idx() else {
            return;
        };
        let len = self.compose.get(slot).map(|c| c.history.len()).unwrap_or(0);
        let cursor = self.compose.get(slot).and_then(|c| c.hist_cursor);
        match history_step(len, cursor, older) {
            HistMove::Stay => {}
            HistMove::Load(i) => self.load_history_line(i),
            HistMove::Restore => {
                let Some(c) = self.compose.get_mut(slot) else {
                    return;
                };
                c.hist_cursor = None;
                let shown = c.history.back().map(String::as_str);
                if shown == Some(c.input.as_str()) {
                    c.input = std::mem::take(&mut c.hist_stash);
                    c.compose_caret_end = true;
                }
            }
        }
    }

    fn load_history_line(&mut self, index: usize) {
        let Some(slot) = self.compose_slot_idx() else {
            return;
        };
        let Some(c) = self.compose.get_mut(slot) else {
            return;
        };
        let Some(line) = c.history.get(index).cloned() else {
            return;
        };
        if c.hist_cursor.is_none() {
            c.hist_stash = c.input.clone();
        }
        c.hist_cursor = Some(index);
        c.input = line;
        c.compose_caret_end = true;
    }

    /// Purple Transfer80 strip under the status grid (BIN / 7+ TX or RX).
    fn transfer_row_open(&self) -> bool {
        if self
            .autobin
            .as_ref()
            .is_some_and(|live| live.state.recv_progress().is_some() || live.state.tx_active())
        {
            return true;
        }
        if self.seven_tx.is_some() {
            return true;
        }
        self.seven.iter().any(|s| s.progress().is_some())
    }

    /// Snapshot for the Transfer80 strip: path, bytes, status label, clock.
    fn transfer_view(&self) -> Option<TransferView> {
        if let Some(live) = self.autobin.as_ref() {
            if let Some((got, total, name)) = live.state.recv_progress() {
                return Some(TransferView {
                    path: name,
                    got,
                    total,
                    status: "BIN-RX",
                    waiting: false,
                    started: live.started,
                });
            }
            if let Some((got, total, name, waiting)) = live.state.tx_progress() {
                return Some(TransferView {
                    path: name,
                    got,
                    total,
                    status: "BIN-TX",
                    waiting,
                    started: live.started,
                });
            }
        }
        if let Some(tx) = self.seven_tx.as_ref() {
            return Some(TransferView {
                path: tx.name.clone(),
                got: tx.offset as u64,
                total: tx.data.len() as u64,
                status: "7+-TX",
                waiting: false,
                started: tx.started,
            });
        }
        for sniff in &self.seven {
            if let Some((got, total, name, started)) = sniff.progress() {
                let path = self.cfg.paths.user_dir.join(&name);
                return Some(TransferView {
                    path: crate::paths::display_native(&path),
                    got,
                    total,
                    status: "7+-RX",
                    waiting: false,
                    started,
                });
            }
        }
        None
    }

    /// Magenta status grid (white hairline cells), same idea as the channel row.
    /// Idle only — transfer uses [`draw_transfer80_head`] / [`draw_transfer80_foot`].
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
        let fg = self.state_fg;
        let font = FontId::monospace(14.0);
        let hair = Stroke::new(1.0, Color32::WHITE);

        {
            let title = self.pane_title();
            let galley = painter.layout_no_wrap(title, FontId::proportional(24.0), fg);
            let title_cell = Rect::from_min_max(rect.left_top(), Pos2::new(title_x, rect.bottom()));
            let pos = Pos2::new(
                title_cell.center().x - galley.size().x * 0.5,
                title_cell.center().y - galley.size().y * 0.5 - 3.0,
            );
            painter.galley(pos, galley, fg);
        }

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
                let galley = painter.layout_no_wrap((*text).clone(), font.clone(), fg);
                let pos = Pos2::new(
                    cell.left() + 8.0,
                    cell.center().y - galley.size().y * 0.5 - 1.0,
                );
                painter.galley(pos, galley, fg);
            }
        }

        painter.line_segment([rect.left_top(), rect.right_top()], hair);
        painter.line_segment([rect.left_top(), rect.left_bottom()], hair);
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], hair);
        painter.line_segment(
            [
                Pos2::new(title_x, rect.top()),
                Pos2::new(title_x, rect.bottom()),
            ],
            hair,
        );
        painter.line_segment(
            [Pos2::new(title_x, mid_y), Pos2::new(rect.right(), mid_y)],
            hair,
        );
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

    /// Transfer rows beside the clock: Channel | Information Transfer, then My/Buffers/….
    fn draw_transfer80_head(&self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, self.state_bg);
        let fg = self.state_fg;
        let font = FontId::monospace(13.0);
        let hair = Stroke::new(1.0, Color32::WHITE);
        let row_h = rect.height() / 2.0;
        let mid = rect.top() + row_h;

        let ch = self
            .active_channel()
            .or_else(|| self.autobin.as_ref().map(|l| l.channel))
            .or_else(|| self.seven_tx.as_ref().map(|t| t.channel))
            .unwrap_or(1);
        let peer = self
            .channel_peer
            .get(ch as usize)
            .and_then(|p| p.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("");
        let channel_txt = if peer.is_empty() {
            format!("Channel {ch}")
        } else {
            format!("Channel {ch}: {peer}")
        };
        let my = self.effective_mycall(ch);
        let at = |v: &[u32]| v.get(ch as usize).copied().unwrap_or(0);

        let r0 = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), mid));
        let split0 = r0.left() + r0.width() * 0.55;
        paint_status_cell(
            &painter,
            Rect::from_min_max(r0.left_top(), Pos2::new(split0, r0.bottom())),
            &channel_txt,
            &font,
            fg,
        );
        paint_status_cell(
            &painter,
            Rect::from_min_max(Pos2::new(split0, r0.top()), r0.right_bottom()),
            "Information Transfer",
            &font,
            fg,
        );

        let r1 = Rect::from_min_max(Pos2::new(rect.left(), mid), rect.right_bottom());
        let row1 = [
            format!("My: {my}"),
            format!("Buffers: {}", self.tnc_free),
            format!("Not disp: {}", at(&self.ch_not_disp)),
            format!("Not sent: {}", at(&self.ch_not_sent)),
            format!("Tries: {}", at(&self.ch_tries)),
            format!("Unack: {}", at(&self.ch_unack)),
        ];
        let cell_w = r1.width() / row1.len() as f32;
        for (i, text) in row1.iter().enumerate() {
            let cell = Rect::from_min_size(
                Pos2::new(r1.left() + i as f32 * cell_w, r1.top()),
                Vec2::new(cell_w, r1.height()),
            );
            paint_status_cell(&painter, cell, text, &font, fg);
        }

        // Hairlines (skip right — clock seam).
        painter.line_segment([rect.left_top(), rect.right_top()], hair);
        painter.line_segment([rect.left_top(), rect.left_bottom()], hair);
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], hair);
        painter.line_segment(
            [Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)],
            hair,
        );
        painter.line_segment(
            [Pos2::new(split0, rect.top()), Pos2::new(split0, mid)],
            hair,
        );
        for c in 1..row1.len() {
            let x = r1.left() + c as f32 * cell_w;
            painter.line_segment([Pos2::new(x, mid), Pos2::new(x, rect.bottom())], hair);
        }
    }

    /// Full-width strip under the clock: thin path/bytes/status + taller bar/times.
    fn draw_transfer80_foot(
        &self,
        ui: &mut Ui,
        rect: Rect,
        view: &TransferView,
        path_h: f32,
        bar_h: f32,
    ) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, self.state_bg);
        let fg = self.state_fg;
        let font = FontId::monospace(13.0);
        let hair = Stroke::new(1.0, Color32::WHITE);
        let path_h = path_h.min(rect.height() * 0.45).max(14.0);
        let bar_top = rect.top() + path_h;
        let r2 = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), bar_top));
        let r3 = Rect::from_min_max(Pos2::new(rect.left(), bar_top), rect.right_bottom());
        let _ = bar_h;

        // Thin path | Bytes | Status
        let path_w = r2.width() * 0.42;
        let bytes_w = r2.width() * 0.33;
        let x_path = r2.left() + path_w;
        let x_bytes = x_path + bytes_w;
        let path = truncate_path_for_bar(&view.path, 64);
        let bytes = format!("Bytes: {}/{}", view.got, view.total);
        let status = format!("Status: {}", view.status);
        paint_status_cell(
            &painter,
            Rect::from_min_max(r2.left_top(), Pos2::new(x_path, r2.bottom())),
            &path,
            &font,
            fg,
        );
        paint_status_cell(
            &painter,
            Rect::from_min_max(Pos2::new(x_path, r2.top()), Pos2::new(x_bytes, r2.bottom())),
            &bytes,
            &font,
            fg,
        );
        paint_status_cell(
            &painter,
            Rect::from_min_max(Pos2::new(x_bytes, r2.top()), r2.right_bottom()),
            &status,
            &font,
            fg,
        );

        // Taller bar + % | Elapsed/Baud and Estimated (each time cell shares bar_h/2).
        let time_w = (r3.width() * 0.36).clamp(220.0, 360.0);
        let bar_right = r3.right() - time_w;
        let time_mid = r3.top() + r3.height() * 0.5;
        let pct = if view.total == 0 {
            0.0
        } else {
            (view.got as f32 / view.total as f32).clamp(0.0, 1.0)
        };
        let elapsed = view.started.elapsed();
        let secs = elapsed.as_secs_f32().max(0.05);
        let baud = if view.got == 0 {
            0
        } else {
            ((view.got as f32) * 8.0 / secs).round() as u32
        };
        let eta_secs = if view.got == 0 {
            0
        } else {
            ((view.total as f32) * secs / (view.got as f32)).round() as u64
        };
        let e = elapsed.as_secs();
        let elapsed_txt = format!(
            "Elapsed Time: {:02}:{:02}:{:02}",
            e / 3600,
            (e / 60) % 60,
            e % 60
        );
        let baud_txt = format!("Eff. Baud: {baud}");
        let eta_txt = format!(
            "Estimated Time: {:02}:{:02}:{:02}",
            eta_secs / 3600,
            (eta_secs / 60) % 60,
            eta_secs % 60
        );

        if view.waiting {
            paint_status_cell(
                &painter,
                Rect::from_min_max(r3.left_top(), Pos2::new(bar_right, r3.bottom())),
                "waiting for #OK#",
                &font,
                fg,
            );
        } else {
            paint_dos_progress_bar(
                &painter,
                Rect::from_min_max(
                    Pos2::new(r3.left() + 4.0, r3.top() + 3.0),
                    Pos2::new(bar_right - 4.0, r3.bottom() - 3.0),
                ),
                pct,
                fg,
            );
        }

        let half = (r3.right() - bar_right) * 0.55;
        paint_status_cell(
            &painter,
            Rect::from_min_max(
                Pos2::new(bar_right, r3.top()),
                Pos2::new(bar_right + half, time_mid),
            ),
            &elapsed_txt,
            &font,
            fg,
        );
        paint_status_cell(
            &painter,
            Rect::from_min_max(
                Pos2::new(bar_right + half, r3.top()),
                Pos2::new(r3.right(), time_mid),
            ),
            &baud_txt,
            &font,
            fg,
        );
        paint_status_cell(
            &painter,
            Rect::from_min_max(Pos2::new(bar_right, time_mid), r3.right_bottom()),
            &eta_txt,
            &font,
            fg,
        );

        // Full-width outer box + cell hairlines (includes under-clock area).
        painter.line_segment([rect.left_top(), rect.right_top()], hair);
        painter.line_segment([rect.left_top(), rect.left_bottom()], hair);
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], hair);
        painter.line_segment([rect.right_top(), rect.right_bottom()], hair);
        painter.line_segment(
            [
                Pos2::new(rect.left(), bar_top),
                Pos2::new(rect.right(), bar_top),
            ],
            hair,
        );
        painter.line_segment(
            [Pos2::new(x_path, rect.top()), Pos2::new(x_path, bar_top)],
            hair,
        );
        painter.line_segment(
            [Pos2::new(x_bytes, rect.top()), Pos2::new(x_bytes, bar_top)],
            hair,
        );
        painter.line_segment(
            [
                Pos2::new(bar_right, bar_top),
                Pos2::new(bar_right, rect.bottom()),
            ],
            hair,
        );
        painter.line_segment(
            [
                Pos2::new(bar_right, time_mid),
                Pos2::new(rect.right(), time_mid),
            ],
            hair,
        );
        painter.line_segment(
            [
                Pos2::new(bar_right + half, bar_top),
                Pos2::new(bar_right + half, time_mid),
            ],
            hair,
        );
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
        self.poll_qrt_disconnects();
    }

    fn channel_tx_busy(&self, channel: u8) -> bool {
        let idx = channel as usize;
        let not_sent = self.ch_not_sent.get(idx).copied().unwrap_or(0);
        let unack = self.ch_unack.get(idx).copied().unwrap_or(0);
        not_sent + unack > 0
    }

    /// Queue DISC for after QRT I-frames / hostmode data leave the modem.
    fn arm_qrt_disconnect(&mut self, channel: u8) {
        let idx = channel as usize;
        if let Some(slot) = self.qrt_disc.get_mut(idx) {
            *slot = QrtDiscState::WaitReport {
                since: Instant::now(),
            };
        }
        if let Some(pad) = self.pads.get_mut(idx) {
            pad.push(PadKind::System, "*** QRT — disconnect after send");
        }
        self.status = format!("//Q ch{channel} — waiting for TX to finish");
        self.last_link_status = self.status.clone();
    }

    /// Fire deferred `//Q` disconnects once not_sent/unack are clear.
    fn poll_qrt_disconnects(&mut self) {
        let mut ready = Vec::new();
        for ch in 1..=self.channels {
            let idx = ch as usize;
            let Some(state) = self.qrt_disc.get(idx).copied() else {
                continue;
            };
            match state {
                QrtDiscState::Idle => {}
                QrtDiscState::WaitReport { since } => {
                    if self.channel_tx_busy(ch) {
                        if let Some(slot) = self.qrt_disc.get_mut(idx) {
                            *slot = QrtDiscState::Drain { since };
                        }
                    } else if since.elapsed() >= QRT_DISC_ARM {
                        // Still idle after arm — QRT already ACKed, or never queued.
                        ready.push(ch);
                    }
                }
                QrtDiscState::Drain { since } => {
                    if !self.channel_tx_busy(ch) || since.elapsed() >= QRT_DISC_TIMEOUT {
                        ready.push(ch);
                    }
                }
            }
        }
        for ch in ready {
            if let Some(slot) = self.qrt_disc.get_mut(ch as usize) {
                *slot = QrtDiscState::Idle;
            }
            self.disconnect_channel(ch);
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
                let link = if let Some(call) = self.channel_peer.get(idx).and_then(|p| p.as_ref()) {
                    format!("Connected to {call}")
                } else if let Some(call) = self.channel_connecting.get(idx).and_then(|p| p.as_ref())
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
                        format!("Bytes: {}", self.editor.byte_count()),
                    ],
                )
            }
            ActivePane::Check => {
                let sel = self
                    .bbs_rows
                    .get(self.bbs_cursor)
                    .map(|e| format!("{} {}", e.rubric, e.number))
                    .unwrap_or_else(|| "(none)".into());
                (
                    vec![format!("Selected: {sel}")],
                    vec![format!("Reference: ch {}", self.cfg.session.bbs_channel)],
                )
            }
            ActivePane::List => {
                let sel = self
                    .box_rows
                    .get(self.box_cursor)
                    .map(|e| e.number.to_string())
                    .unwrap_or_else(|| "(none)".into());
                let drawer = if self.cfg.session.file_drawer.trim().is_empty() {
                    "(current)".into()
                } else {
                    self.cfg.session.file_drawer.clone()
                };
                (
                    vec![
                        format!("Selected: {sel}"),
                        format!("Reference: ch {}", self.cfg.session.list_channel),
                    ],
                    vec![format!("File drawer: {drawer}")],
                )
            }
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
            ActivePane::Map => {
                let qth = if self.cfg.station.locator.is_empty() {
                    "(none)".into()
                } else {
                    self.cfg.station.locator.clone()
                };
                let plotted = self
                    .mheard
                    .entries
                    .iter()
                    .filter(|e| {
                        e.locator
                            .as_ref()
                            .map(|l| l.len() == 6 || l.len() == 8)
                            .unwrap_or(false)
                    })
                    .count();
                let hover = self.map_hover_loc.clone().unwrap_or_else(|| "—".into());
                let place = self
                    .map_hover_loc
                    .as_deref()
                    .and_then(|l| crate::geo::global().resolve_locator_label(l))
                    .unwrap_or_default();
                let row2 = if place.is_empty() {
                    format!("Cursor: {hover}")
                } else {
                    format!("Cursor: {hover}  {place}")
                };
                (
                    vec![format!("QTH: {qth}"), format!("Plotted: {plotted}")],
                    vec![row2],
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
                    vec![format!(
                        "Lines: {}",
                        self.spy_pads
                            .get(n.saturating_sub(1) as usize)
                            .map(|p| p.lines.len())
                            .unwrap_or(0)
                    )],
                )
            }
        }
    }

    fn draw_channel_grid(&mut self, ui: &mut Ui, rect: Rect) {
        // Looking at the channel puts its label back to grey.
        if let ActivePane::Channel(n) = self.active {
            self.clear_channel_attention(n);
        }
        if self.channel_unread.iter().any(|on| *on) {
            // White/red is a step, not a fade. Wake on the change only.
            let t = ui.input(|i| i.time);
            let period = ALERT_BLINK_HALF * 2.0;
            let phase = t % period;
            let wait = if phase < ALERT_BLINK_HALF {
                ALERT_BLINK_HALF - phase
            } else {
                period - phase
            };
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(wait.max(0.05)));
        }
        let painter = ui.painter_at(rect);
        // Menu-window background, so a 1px strip stays visible above/below the buttons.
        let menu_bg = self.menu_bg;
        painter.rect_filled(rect, 0.0, menu_bg);

        let cols = 6usize;
        let rows = 2usize;
        // Inset: 1px white border + 1px of that colour above/below the button rows.
        let grid = Rect::from_min_max(
            Pos2::new(rect.left() + 1.0, rect.top() + 2.0),
            Pos2::new(rect.right() - 1.0, rect.bottom() - 2.0),
        );
        let cell_w = grid.width() / cols as f32;
        let cell_h = grid.height() / rows as f32;

        for row in 0..rows {
            for col in 0..cols {
                let n = (row * cols + col + 1) as u8;
                let cell = Rect::from_min_size(
                    Pos2::new(
                        grid.left() + col as f32 * cell_w + 1.0,
                        grid.top() + row as f32 * cell_h + 1.0,
                    ),
                    Vec2::new((cell_w - 2.0).max(1.0), (cell_h - 2.0).max(1.0)),
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
                // Face stays dark. A connect or unread text, while this
                // channel is not open, blinks the label white → red.
                let alert = matches!(action, Some(ActivePane::Channel(ch)) if {
                    !selected && self.channel_unread.get(ch as usize).copied().unwrap_or(false)
                });
                let fill = if selected { menu_bg } else { VGA_DARK_GRAY };
                let text_color = if alert {
                    let t = ui.input(|i| i.time);
                    if (t / ALERT_BLINK_HALF) as i64 % 2 == 0 {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(220, 32, 32)
                    }
                } else {
                    VGA_GRAY
                };
                let id = ui.id().with(("ch", n));
                let resp = ui.interact(cell, id, Sense::click());
                // Top of the main window: slight round, white left/top, black bottom/right.
                retro::paint(
                    &painter,
                    cell,
                    fill,
                    selected || resp.is_pointer_button_down_on(),
                    retro::TOP_ROUND,
                );
                // Same JetBrains Mono as the pads. Zoom swaps Regular and Bold.
                let font = FontId::monospace(14.0);
                let anchor = egui::Align2::CENTER_CENTER;
                let at = cell.center();
                painter
                    .with_clip_rect(cell)
                    .text(at, anchor, &label, font, text_color);
                if resp.clicked() {
                    if let Some(pane) = action {
                        self.select(pane);
                    }
                }
            }
        }

        // 1px white surround — same treatment as the clock column.
        ui.painter()
            .rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0, Color32::WHITE));
    }

    fn mail_list_pane(&mut self, ui: &mut Ui, width: f32, height: f32, list: bool) {
        let bg = if list { self.list_bg } else { self.check_bg };
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let cursor = self.mail_cursor(list);
        let lines: Vec<String> = self
            .mail_rows(list)
            .iter()
            .map(MailEntry::display)
            .collect();
        let mut pick: Option<(usize, bool, bool)> = None;
        let font = FontId::monospace(self.pad_font_px());
        let row_h = ui.fonts(|f| f.row_height(&font)).max(self.pad_font_px());
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            Frame::none().fill(bg).show(ui, |ui| {
                ui.set_min_size(rect.size());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(false)
                    .id_salt(if list { "box_list" } else { "bbs_list" })
                    .show(ui, |ui| {
                        for (i, line) in lines.iter().enumerate() {
                            let selected = i == cursor;
                            let fill = if selected {
                                Color32::WHITE
                            } else {
                                Color32::TRANSPARENT
                            };
                            let fg = if selected {
                                Color32::BLACK
                            } else {
                                Color32::WHITE
                            };
                            let (row, resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width().max(20.0), row_h),
                                Sense::click(),
                            );
                            ui.painter().rect_filled(row, 0.0, fill);
                            let umlaut = if list {
                                self.session.umlaut_for(SLOT_LIST)
                            } else {
                                self.session.umlaut_for(SLOT_BBS)
                            };
                            let shown = show_channel_text(line, umlaut);
                            ui.painter().text(
                                row.left_center() + Vec2::new(4.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                &shown,
                                font.clone(),
                                fg,
                            );
                            if resp.double_clicked() {
                                pick = Some((i, false, true));
                            } else if resp.secondary_clicked() {
                                pick = Some((i, true, false));
                            } else if resp.clicked() {
                                pick = Some((i, false, false));
                            }
                        }
                    });
            });
        });
        if let Some((i, mark, read)) = pick {
            self.set_mail_cursor(list, i);
            if mark {
                self.toggle_mail_mark(list);
            }
            if read {
                self.read_mail(list);
            }
        }
    }

    /// Live view of the program on this channel, under the clock.
    /// At most a quarter of the screen wide, in a small face.
    fn draw_run_terminal(&self, ui: &mut Ui, panel: Rect) {
        let ActivePane::Channel(channel) = self.active else {
            return;
        };
        let Some(gate) = self.external_run.get(channel as usize) else {
            return;
        };
        if !gate.occupied() {
            return;
        }
        let name = gate.name().unwrap_or("").to_string();
        let body = self
            .external_screen
            .get(channel as usize)
            .map(|s| s.as_str())
            .unwrap_or("");
        let rect = Rect::from_min_size(
            Pos2::new(panel.right() - 410.0, panel.top()),
            Vec2::new(410.0, 200.0),
        );
        let font = FontId::monospace(8.0);
        let ink = Color32::from_rgb(255, 255, 85);
        let mut job = LayoutJob::default();
        let mut first = true;
        for line in body.lines() {
            if !first {
                job.append(
                    "\n",
                    0.0,
                    egui::TextFormat {
                        font_id: font.clone(),
                        color: Color32::WHITE,
                        ..Default::default()
                    },
                );
            }
            first = false;
            let typed = line.starts_with("> ");
            job.append(
                line,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: if typed { ink } else { Color32::WHITE },
                    ..Default::default()
                },
            );
        }
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            Frame::none()
                .fill(VGA_DOS)
                .stroke(Stroke::new(1.0, Color32::WHITE))
                .inner_margin(4.0)
                .show(ui, |ui| {
                    ui.set_min_size(rect.size() - Vec2::splat(2.0));
                    ui.label(RichText::new(name).font(font.clone()).color(ink));
                    egui::ScrollArea::vertical()
                        .id_salt(("run_term", channel))
                        .animated(false)
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if job.is_empty() {
                                ui.label(RichText::new("…").font(font).color(Color32::WHITE));
                            } else {
                                ui.add(egui::Label::new(job).wrap_mode(TextWrapMode::Wrap));
                            }
                        });
                });
        });
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
        if matches!(self.active, ActivePane::Map) {
            self.map_pane(ui, width, height);
            return;
        }
        if matches!(self.active, ActivePane::Check) && !self.bbs_rows.is_empty() {
            self.mail_list_pane(ui, width, height, false);
            return;
        }
        if matches!(self.active, ActivePane::List) && !self.box_rows.is_empty() {
            self.mail_list_pane(ui, width, height, true);
            return;
        }
        let bg = match self.active {
            ActivePane::Channel(_) => self.connect_bg,
            ActivePane::Spy(_) => self.spy_bg,
            ActivePane::Check => self.check_bg,
            ActivePane::List => self.list_bg,
            _ => self.monitor_bg,
        };
        let monitor = matches!(self.active, ActivePane::Monitor);
        let nudge = self.session.take_scroll_nudge();
        // ScrLock off → follow last line; on → freeze (QSO and split monitor).
        let stick = !self.session.scr_lock;
        let smooth = self.session.softscroll_for(slot_for(self.active));
        let show_bar = self.session.scrollbar;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        // Never enable horizontal scroll — wrap to the viewport instead.
        paint_text_pane(
            ui,
            rect,
            bg,
            "rx_full",
            stick,
            smooth,
            pane_scroll_owner(self.active),
            show_bar,
            nudge,
            false,
            self.pad_font_px(),
            |ui| {
                let cols = text_columns(ui, self.pad_font_px());
                if monitor {
                    self.monitor_layout_for(cols)
                } else {
                    self.active_layout_for(cols)
                }
            },
        );
        self.note_split_right_click(ui, rect);
    }

    /// Offline world map (Maidenhead + heard stations). Entered from MHeard.
    fn map_pane(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        if self.map_needs_world_fit {
            let aspect = rect.width() / rect.height().max(1.0);
            self.map_view.fit_world(aspect as f64);
            self.map_needs_world_fit = false;
        }
        let markers = map::collect_markers(
            &self.cfg.station.locator,
            self.mheard
                .entries
                .iter()
                .filter(|e| e.listed())
                .map(|e| (e.call.clone(), e.locator.clone(), e.lat, e.lon)),
        );
        let colors = map::MapColors {
            background: self.map_bg,
            land: self.map_fg,
            grid: self.map_grid,
            qth: self.map_qth,
            heard: self.map_heard,
        };
        let font = FontId::monospace(11.0);
        let hover = map::paint_map(
            ui,
            rect,
            &mut self.map_view,
            crate::geo::global(),
            colors,
            &markers,
            font,
        );
        self.map_hover_loc = hover;
    }

    /// Clickable MHeard sheet (row select → SPY confirm).
    fn mheard_pane(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let bg = self.mh_bg;
        let text_fg = self.mh_fg;
        let text_inv = self.mh_inv;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let show_bar = self.session.scrollbar;
        let nudge = self.session.take_scroll_nudge();
        let pick = self.mheard_spy_pick;
        let selected = self.mheard_selected;
        let open_at_top = self.mheard_scroll_top;
        self.mheard_scroll_top = false;
        let show_port = self.mheard_shows_port();
        let qth_ll = crate::locator::normalize_locator(&self.cfg.station.locator)
            .and_then(|loc| crate::locator::locator_to_lat_lon(&loc));
        let geo = crate::geo::global();
        let rows: Vec<(usize, HeardSheetRow)> = self
            .mheard
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.listed())
            .map(|(i, e)| {
                let geo_line = geo.station_detail_line(e.locator.as_deref(), e.lat, e.lon, qth_ll);
                (i, HeardSheetRow::from_station(e, geo_line))
            })
            .collect();
        let mut clicked: Option<usize> = None;
        let row_font = FontId::monospace(13.0);
        let head_font = FontId::monospace(12.0);
        let rule = Stroke::new(
            1.0,
            Color32::from_rgb(text_fg.r() / 3, text_fg.g() / 3, text_fg.b() / 3),
        );

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            let avail = ui.available_size();
            ui.set_min_size(avail);
            ui.painter().rect_filled(rect, 0.0, bg);
            Frame::none().fill(bg).inner_margin(0.0).show(ui, |ui| {
                let full = ui.max_rect();
                ui.set_min_size(full.size());
                let bar_w = if show_bar {
                    DOS_SCROLL_W.min(full.width() * 0.5)
                } else {
                    0.0
                };
                let text_outer =
                    Rect::from_min_max(full.min, Pos2::new(full.right() - bar_w, full.bottom()));
                let text = Rect::from_min_max(
                    Pos2::new(text_outer.left() + 4.0, text_outer.top() + 2.0),
                    Pos2::new(
                        (text_outer.right() - 2.0).max(text_outer.left() + 8.0),
                        (text_outer.bottom() - 2.0).max(text_outer.top() + 8.0),
                    ),
                );
                let bar_rect = Rect::from_min_max(
                    Pos2::new(text_outer.right(), full.top()),
                    full.right_bottom(),
                );
                let row_h = ui.fonts(|f| f.row_height(&row_font)).max(1.0) + 2.0;
                let head_h = ui.fonts(|f| f.row_height(&head_font)).max(1.0) + 4.0;
                let cell_w = ui.fonts(|f| f.glyph_width(&row_font, 'M')).max(1.0);
                let cols = mheard_sheet_cols(text.width(), show_port, cell_w);
                let payload_x = cols
                    .iter()
                    .take_while(|c| c.key != "payload")
                    .map(|c| c.width)
                    .sum::<f32>();

                let mut snap = None;
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(text), |ui| {
                    ui.set_min_size(text.size());
                    if pick {
                        ui.label(
                            RichText::new("SPY — click a station, then confirm")
                                .color(text_fg)
                                .strong(),
                        );
                    }
                    let header =
                        Rect::from_min_size(ui.cursor().min, Vec2::new(text.width(), head_h));
                    ui.allocate_rect(header, Sense::hover());
                    paint_mheard_sheet_header(
                        ui.painter(),
                        header,
                        &cols,
                        &head_font,
                        text_fg,
                        bg,
                        rule,
                    );

                    let body = Rect::from_min_max(
                        Pos2::new(text.left(), header.bottom()),
                        text.right_bottom(),
                    );
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
                        ui.set_min_size(body.size());
                        let mut list = egui::ScrollArea::vertical()
                            .id_salt("mheard_list")
                            .auto_shrink([false, false])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            );
                        if open_at_top {
                            list = list.vertical_scroll_offset(0.0);
                        }
                        let out = list.show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            if nudge != 0.0 {
                                ui.scroll_with_delta(Vec2::new(0.0, nudge));
                            }
                            if rows.is_empty() {
                                ui.label(
                                    RichText::new(
                                        "No stations heard yet — APRS/AX.25 UI frames appear here.",
                                    )
                                    .color(text_fg)
                                    .font(row_font.clone()),
                                );
                            } else {
                                for (i, row) in &rows {
                                    let is_sel = selected == Some(*i);
                                    let fill = if is_sel {
                                        text_inv
                                    } else {
                                        Color32::TRANSPARENT
                                    };
                                    let fg = if is_sel { bg } else { text_fg };
                                    let block_h = if row.geo_line.is_some() {
                                        row_h * 2.0
                                    } else {
                                        row_h
                                    };
                                    let (block_rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(body.width(), block_h),
                                        Sense::click(),
                                    );
                                    ui.painter().rect_filled(block_rect, 0.0, fill);
                                    let main = Rect::from_min_size(
                                        block_rect.min,
                                        Vec2::new(block_rect.width(), row_h),
                                    );
                                    paint_mheard_sheet_row(
                                        ui.painter(),
                                        main,
                                        &cols,
                                        row,
                                        &row_font,
                                        fg,
                                        rule,
                                    );
                                    if let Some(geo_line) = row.geo_line.as_deref() {
                                        let sub = Rect::from_min_size(
                                            Pos2::new(block_rect.left(), block_rect.top() + row_h),
                                            Vec2::new(block_rect.width(), row_h),
                                        );
                                        let x =
                                            (sub.left() + payload_x + 4.0).min(sub.right() - 8.0);
                                        let clipped = clip_sheet_cell(
                                            geo_line,
                                            (sub.right() - x).max(8.0),
                                            &row_font,
                                            ui.painter(),
                                        );
                                        ui.painter().text(
                                            Pos2::new(x, sub.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            clipped,
                                            row_font.clone(),
                                            fg,
                                        );
                                    }
                                    if resp.clicked() {
                                        clicked = Some(*i);
                                    }
                                }
                            }
                        });
                        snap = Some((
                            out.id,
                            out.state,
                            out.content_size.y,
                            out.inner_rect.height(),
                        ));
                    });
                });
                if show_bar && bar_rect.width() > 4.0 {
                    if let Some((sid, mut state, content_h, view_h)) = snap {
                        drive_dos_bar(ui, bar_rect, sid, &mut state, content_h, view_h, row_h);
                    }
                }
            });
        });

        if let Some(i) = clicked {
            self.mheard_selected = Some(i);
            if let Some(e) = self.mheard.entries.get(i) {
                self.status = if show_port {
                    match e.port {
                        Some(port) => format!("MHeard: {} on port {port}", e.call),
                        None => format!("MHeard: {}", e.call),
                    }
                } else {
                    format!("MHeard: {}", e.call)
                };
                if self.mheard_spy_pick {
                    self.open_mheard_spy_confirm(e.call.clone());
                }
            }
        }
    }

    /// Full-pane monospace editor (CP437 GPI files) with a macro cheat-sheet.
    ///
    /// The text field is fixed at 90 monospace columns; large files scroll via
    /// an outer `ScrollArea` (TextEdit alone was clipping long buffers).
    fn editor_pane(&mut self, ui: &mut Ui, width: f32, height: f32) {
        const EDITOR_COLS: f32 = 90.0;
        const EDITOR_FONT_SIZE: f32 = 16.0;
        let bg = self.editor_bg;
        let fg = self.editor_fg;
        // Same idle channel-button fill; white labels on dark grey.
        let help_bg = VGA_DARK_GRAY;
        let help_fg = Color32::WHITE;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            let font = FontId::monospace(EDITOR_FONT_SIZE);
            let char_w = ui.fonts(|f| f.glyph_width(&font, 'M'));
            let row_h = ui.fonts(|f| f.row_height(&font)).max(1.0);
            let editor_text_w = char_w * EDITOR_COLS;
            // Frame margin + vertical scrollbar gutter.
            let editor_col_w = (editor_text_w + 18.0).min(ui.available_width());
            let pane_h = ui.available_height();

            // TOP — not horizontal()'s vertical center — so a short CTEXT stays at the top.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(editor_col_w, pane_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        Frame::none()
                            .fill(bg)
                            .stroke(Stroke::new(1.0, VGA_GRAY))
                            .inner_margin(4.0)
                            .show(ui, |ui| {
                                ui.set_min_size(Vec2::new(
                                    (editor_col_w - 2.0).max(40.0),
                                    (pane_h - 2.0).max(40.0),
                                ));
                                ui.set_max_size(ui.min_size());
                                let avail = ui.available_size();
                                // Fill the viewport with editable rows so the caret can go
                                // under the last line; never shorter than the text itself.
                                let min_rows = ((avail.y / row_h).floor() as usize).max(8);
                                let content_rows = self.editor.text.split('\n').count().max(1);
                                let rows = content_rows.max(min_rows);
                                let before = self.editor.text.clone();
                                let underscore_w = ui.fonts(|f| f.glyph_width(&font, '_')).max(6.0);
                                // Hide egui's tall bar; paint a DOS `_` after show().
                                ui.visuals_mut().text_cursor.stroke =
                                    Stroke::new(0.0, Color32::TRANSPARENT);
                                let mut output = None;
                                egui::ScrollArea::vertical()
                                    .id_salt("gp_editor_scroll")
                                    .auto_shrink([false, false])
                                    .max_height(avail.y)
                                    .show(ui, |ui| {
                                        ui.with_layout(
                                            egui::Layout::top_down(egui::Align::LEFT),
                                            |ui| {
                                                output = Some(
                                                    egui::TextEdit::multiline(
                                                        &mut self.editor.text,
                                                    )
                                                    .id_salt("gp_editor")
                                                    .font(font.clone())
                                                    .text_color(fg)
                                                    .frame(false)
                                                    .desired_width(editor_text_w)
                                                    .desired_rows(rows)
                                                    .lock_focus(true)
                                                    .show(ui),
                                                );
                                                if self.editor_resume {
                                                    if let Some(shown) = output.as_ref() {
                                                        let id = shown.response.id;
                                                        let mut state = egui::TextEdit::load_state(
                                                            ui.ctx(),
                                                            id,
                                                        )
                                                        .unwrap_or_default();
                                                        let cc = self
                                                            .editor
                                                            .cursor_cc
                                                            .min(self.editor.text.chars().count());
                                                        let end = egui::text::CCursor::new(cc);
                                                        state.cursor.set_char_range(Some(
                                                            egui::text::CCursorRange::one(end),
                                                        ));
                                                        state.store(ui.ctx(), id);
                                                        shown.response.request_focus();
                                                    }
                                                    self.editor_resume = false;
                                                }
                                            },
                                        );
                                    });
                                if let Some(output) = output {
                                    if output.response.has_focus() {
                                        let commit = ui.input(|i| dropped_ime_commit(&i.events));
                                        if !commit.is_empty() && self.editor.text == before {
                                            let cursor = output
                                                .cursor_range
                                                .map(|r| r.primary.ccursor.index)
                                                .unwrap_or_else(|| before.chars().count());
                                            let at = insert_at_char(
                                                &mut self.editor.text,
                                                cursor,
                                                &commit,
                                            );
                                            let mut state = egui::TextEdit::load_state(
                                                ui.ctx(),
                                                output.response.id,
                                            )
                                            .unwrap_or_default();
                                            state.cursor.set_char_range(Some(
                                                egui::text::CCursorRange::one(
                                                    egui::text::CCursor::new(at),
                                                ),
                                            ));
                                            state.store(ui.ctx(), output.response.id);
                                            self.editor.cursor_cc = at;
                                        }
                                    }
                                    if self.editor.text != before {
                                        self.editor.dirty = true;
                                    }
                                    if let Some(cursor) = output.cursor_range {
                                        let cc = cursor.as_sorted_char_range().start;
                                        let snap = self.editor.text.clone();
                                        self.editor.update_cursor_from_galley(&snap, cc);
                                        if output.response.has_focus() {
                                            paint_dos_caret(
                                                ui,
                                                output.galley_pos,
                                                output.galley.as_ref(),
                                                &cursor.primary,
                                                row_h,
                                                underscore_w,
                                                self.editor_fg,
                                            );
                                        }
                                    }
                                }
                            });
                    },
                );

                let help_w = ui.available_width().max(0.0);
                if help_w > 40.0 {
                    ui.allocate_ui_with_layout(
                        Vec2::new(help_w, pane_h),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            Frame::none()
                                .fill(help_bg)
                                .stroke(Stroke::new(1.0, VGA_GRAY))
                                .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                                .show(ui, |ui| {
                                    // Fixed cheat-sheet: no scroll — only the blue editor scrolls.
                                    ui.set_min_size(Vec2::new(
                                        (help_w - 2.0).max(40.0),
                                        (pane_h - 2.0).max(40.0),
                                    ));
                                    ui.set_max_size(ui.min_size());
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                            ui.label(
                                                RichText::new("Macros")
                                                    .strong()
                                                    .color(help_fg)
                                                    .size(15.0),
                                            );
                                            ui.label(
                                                RichText::new("*.GPI / CTEXT / mail")
                                                    .color(help_fg)
                                                    .size(12.0),
                                            );
                                            ui.add_space(4.0);
                                            for (token, desc) in macros::help_entries() {
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 6.0;
                                                    ui.label(
                                                        RichText::new(*token)
                                                            .monospace()
                                                            .color(help_fg)
                                                            .size(13.0),
                                                    );
                                                    ui.label(
                                                        RichText::new(*desc)
                                                            .color(help_fg)
                                                            .size(12.0),
                                                    );
                                                });
                                            }
                                        },
                                    );
                                });
                        },
                    );
                }
            });
        });
    }

    fn session_bar(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, VGA_GRAY);
        // Classic raised strip: white on top, black underneath (also when the
        // bar rides up for channel splitscreen + monitor).
        painter.hline(
            rect.x_range(),
            rect.top() + 0.5,
            Stroke::new(1.0, Color32::WHITE),
        );
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, Color32::BLACK),
        );

        // Behind the buttons. A still press still clicks; a move resizes.
        // Drag-only, and not focusable, so the compose box keeps the keyboard.
        let split_bar = if matches!(self.active, ActivePane::Channel(_)) {
            Some(ui.interact(
                rect,
                egui::Id::new("gp_split_bar"),
                Sense {
                    click: false,
                    drag: true,
                    focusable: false,
                },
            ))
        } else {
            self.split_dragging = false;
            None
        };

        let on = VGA_GREEN; // same as clock ♫ when on (not bright LightGreen)
        let off = VGA_GRAY;
        let text_c = Color32::BLACK;
        let y = rect.top() + 2.0;
        let h = (rect.height() - 4.0).max(16.0);
        let gap = 2.0;
        let (name_label, name_known) = match self.active {
            ActivePane::Channel(channel) => self.peer_name_label(channel),
            _ => (String::new(), false),
        };
        let name_width = ui
            .painter()
            .layout_no_wrap(name_label.clone(), FontId::proportional(11.0), text_c)
            .size()
            .x
            + 14.0;

        let mut clicks: Vec<SessionClick> = Vec::new();
        let mut controls: Vec<Rect> = Vec::new();

        let mut paint_cell = |ui: &mut Ui,
                              origin_x: f32,
                              label: &str,
                              lit: bool,
                              action: SessionClick,
                              w: f32,
                              glyph: CellGlyph|
         -> f32 {
            let cell = Rect::from_min_size(Pos2::new(origin_x, y), Vec2::new(w, h));
            controls.push(cell);
            let id = ui.id().with(("sess", action, label));
            let resp = ui.interact(cell, id, Sense::click());
            // ▲▼ are not toggles, so they never use the green “on” face.
            // Only a channel with the split open draws them as live buttons.
            // The Monitor has no split, so the pair stays grey there, same as
            // a closed split, Spy, BBS-Menu, and Box-List.
            let arrow = matches!(glyph, CellGlyph::TriUp | CellGlyph::TriDown);
            let arrow_clickable = match self.active {
                ActivePane::Channel(_) => true,
                ActivePane::Monitor => !self.session.scr_lock,
                _ => false,
            };
            let arrow_off = arrow
                && match self.active {
                    ActivePane::Channel(_) => !self.split_on,
                    _ => true,
                };
            // The key is live only while this channel is linked to a station.
            let key_off = matches!(glyph, CellGlyph::Key)
                && match self.active {
                    ActivePane::Channel(n) => !self.channel_linked(n),
                    _ => true,
                };
            let inert = arrow_off || key_off;
            let fill = if inert {
                Color32::from_rgb(210, 210, 210)
            } else if lit {
                on
            } else {
                off
            };
            let mark = if inert {
                Color32::from_rgb(130, 130, 130)
            } else {
                text_c
            };
            let painter = ui.painter();
            painter.rect_filled(cell, 0.0, fill);
            // A lit toggle stays sunken. ▲▼ sink only while the mouse is held.
            let pressed = if arrow {
                arrow_clickable && resp.is_pointer_button_down_on()
            } else if key_off {
                false
            } else {
                lit || resp.is_pointer_button_down_on()
            };
            paint_bevel(painter, cell, pressed);
            match glyph {
                CellGlyph::Text => {
                    painter.text(
                        cell.center() - Vec2::new(0.0, 1.0),
                        egui::Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.0),
                        mark,
                    );
                }
                CellGlyph::Stairs => {
                    // Solid stair rising to the right. A Unicode stair is a box.
                    let c = cell.center();
                    let sw = 5.0;
                    let sh = 4.0;
                    let left = c.x - sw * 1.5;
                    let bottom = c.y + sh * 1.5;
                    for i in 0..3 {
                        let i = i as f32;
                        painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(left + i * sw, bottom - (i + 1.0) * sh),
                                Pos2::new(left + (i + 1.0) * sw, bottom),
                            ),
                            0.0,
                            mark,
                        );
                    }
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
                    painter.add(egui::Shape::convex_polygon(pts, mark, Stroke::NONE));
                }
                CellGlyph::Key => {
                    // Drawn key. The session font has no reliable 🗝 glyph.
                    let c = cell.center();
                    let bow = Pos2::new(c.x - 4.5, c.y);
                    painter.circle_stroke(bow, 3.2, Stroke::new(1.5, mark));
                    painter.line_segment(
                        [Pos2::new(bow.x + 3.0, c.y), Pos2::new(c.x + 6.5, c.y)],
                        Stroke::new(1.5, mark),
                    );
                    painter.line_segment(
                        [Pos2::new(c.x + 3.5, c.y), Pos2::new(c.x + 3.5, c.y + 3.2)],
                        Stroke::new(1.5, mark),
                    );
                    painter.line_segment(
                        [Pos2::new(c.x + 6.5, c.y), Pos2::new(c.x + 6.5, c.y + 3.2)],
                        Stroke::new(1.5, mark),
                    );
                }
            }
            if resp.clicked() && (!arrow || arrow_clickable) && !key_off {
                clicks.push(action);
            }
            // Right-click CT: same connect-text file picker.
            if resp.secondary_clicked() && matches!(action, SessionClick::Ctext) {
                clicks.push(SessionClick::CtextPick);
            }
            w + gap
        };

        // Trailing controls always visible on the right.
        // ▲▼ stay ordinary buttons (never the green “on” face).
        let trail = [
            (
                "ScrLock",
                self.session.scr_lock,
                SessionClick::ScrLock,
                54.0_f32,
                CellGlyph::Text,
            ),
            ("", false, SessionClick::ScrollUp, 22.0, CellGlyph::TriUp),
            (
                "",
                false,
                SessionClick::ScrollDown,
                22.0,
                CellGlyph::TriDown,
            ),
            (
                "B",
                self.session.scrollbar,
                SessionClick::Scrollbar,
                22.0,
                CellGlyph::Text,
            ),
        ];
        // Each modem is a badge plus its own lamp, just left of ScrLock.
        let lamp_w = h;
        let modem_span: f32 = self
            .modems
            .iter()
            .map(|face| modem_badge_w(&face.label) + gap + lamp_w + gap)
            .sum();
        let trail_w: f32 = trail.iter().map(|(_, _, _, w, _)| w + gap).sum::<f32>() + modem_span;
        let modem_x = rect.right() - trail_w - 2.0;
        let mut tx = modem_x + modem_span;
        for (label, lit, action, w, glyph) in trail {
            tx += paint_cell(ui, tx, label, lit, action, w, glyph);
        }

        // Monitor: spy slots sit just left of the modem badges.
        let spy_cell_w = 20.0_f32;
        let lock_w = 26.0_f32;
        let on_channel = matches!(self.active, ActivePane::Channel(_));
        let spy_n = if matches!(self.active, ActivePane::Monitor | ActivePane::Spy(_)) {
            self.spy_limit()
        } else {
            0
        };
        let right_mid_w = if spy_n > 0 {
            spy_n as f32 * (spy_cell_w + gap)
        } else if on_channel {
            lock_w + gap
        } else {
            0.0
        };

        // Leading controls stay on the left.
        let mut x = rect.left() + 2.0;
        let left_limit = (rect.right() - trail_w - right_mid_w - 8.0).max(x + 40.0);
        let settings_limit = if on_channel {
            left_limit - name_width - gap
        } else {
            left_limit
        };

        let qrg = self.session.qrg_label();
        let ct_slot = self.ctext_slot();
        let ct = self.session.ctext_label_for(ct_slot);
        let ct_own = self.session.channel_ctext_path(ct_slot).is_some();
        let ct_w = (ct.len() as f32 * 7.5 + 14.0).clamp(70.0, 160.0);
        let chrome_slot = slot_for(self.active);
        let soft = self.session.softscroll_lit(self.active);
        let step = self.session.line_step_for(chrome_slot);
        let lead_owned = [
            (
                qrg.as_str(),
                false,
                SessionClick::Qrg,
                104.0_f32,
                CellGlyph::Text,
            ),
            (
                ct.as_str(),
                ct_own,
                SessionClick::Ctext,
                ct_w,
                CellGlyph::Text,
            ),
            (
                "äöü",
                self.session.umlaut_lit(self.active),
                SessionClick::Ascii,
                34.0,
                CellGlyph::Text,
            ),
            (
                step.label(),
                soft,
                SessionClick::LineStep,
                26.0,
                if soft {
                    CellGlyph::Stairs
                } else {
                    CellGlyph::Text
                },
            ),
            (
                "Echo",
                self.session.echo_lit(self.active),
                SessionClick::Echo,
                38.0,
                CellGlyph::Text,
            ),
            (
                "♪",
                self.session.bell_lit(self.active),
                SessionClick::Bell,
                26.0,
                CellGlyph::Text,
            ),
            ("Clear", false, SessionClick::Clear, 46.0, CellGlyph::Text),
        ];
        for (label, lit, action, w, glyph) in lead_owned {
            if x + w > settings_limit {
                break;
            }
            x += paint_cell(ui, x, label, lit, action, w, glyph);
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
            }
            ActivePane::Channel(n) => {
                for (label, lit, action, w) in [
                    (
                        "7+",
                        self.session.seven_plus_for(n),
                        SessionClick::SevenPlus,
                        28.0_f32,
                    ),
                    (
                        "Remote",
                        self.session.remote_for(n),
                        SessionClick::Remote,
                        50.0,
                    ),
                    ("BIN-RX", self.session.bin_rx, SessionClick::BinRx, 50.0),
                    ("BIN-TX", self.session.bin_tx, SessionClick::BinTx, 50.0),
                ] {
                    if x + w > settings_limit {
                        break;
                    }
                    x += paint_cell(ui, x, label, lit, action, w, CellGlyph::Text);
                }
                paint_cell(
                    ui,
                    x,
                    &name_label,
                    name_known,
                    SessionClick::Name,
                    name_width,
                    CellGlyph::Text,
                );
                let trail_left = rect.right() - trail_w - 2.0;
                let lit = self.channel_linked(n)
                    && self.channel_lock.get(n as usize).copied().unwrap_or(false);
                let _ = paint_cell(
                    ui,
                    trail_left - (lock_w + gap),
                    "",
                    lit,
                    SessionClick::Lock,
                    lock_w,
                    CellGlyph::Key,
                );
            }
            _ => {}
        }

        drop(paint_cell);

        let now = Instant::now();
        let mut mx = modem_x;
        for (index, face) in self.modems.iter().enumerate() {
            let badge_w = modem_badge_w(&face.label);
            paint_modem_badge(ui, mx, y, h, &face.label, face.phase, badge_w);
            mx += badge_w + gap;
            paint_link_lamp(
                ui,
                &mut controls,
                mx,
                y,
                lamp_w,
                h,
                index,
                self.link_tex.as_ref(),
                face.tx_until.is_some_and(|t| t > now),
                face.rx_until.is_some_and(|t| t > now),
            );
            mx += lamp_w + gap;
        }

        if let Some(resp) = split_bar.filter(|_| ui.is_enabled()) {
            let (hover, down, pressed, origin, delta, abort) = ui.input(|i| {
                (
                    i.pointer.hover_pos(),
                    i.pointer.primary_down(),
                    i.pointer.primary_pressed(),
                    i.pointer.press_origin(),
                    i.pointer.delta().y,
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if abort {
                self.split_dragging = false;
            } else if pressed && origin.is_some_and(|p| rect.contains(p)) {
                // Whole grey bar, including the gaps around ▲▼. Latched for
                // the whole press: the bar moves, so a fresh hit test drops it.
                self.split_dragging = true;
                self.split_drag_dy = 0.0;
            }
            let over_control = hover.is_some_and(|p| controls.iter().any(|c| c.contains(p)));
            let resizing = self.split_drag_dy.abs() > SPLIT_DRAG_SLOP;
            if resizing || (resp.hovered() && !over_control) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }
            if self.split_dragging && down && delta != 0.0 {
                if let Some(apply) = split_drag_step(self.split_drag_dy, delta) {
                    self.drag_split(apply);
                    ui.ctx().request_repaint();
                }
                self.split_drag_dy += delta;
            }
            if self.split_on {
                ui.painter()
                    .hline(rect.x_range(), rect.top(), Stroke::new(2.0, Color32::WHITE));
            }
            self.note_split_right_click(ui, rect);
        }

        for action in clicks {
            self.handle_session_click(action);
        }
        let released = ui.input(|i| !i.pointer.primary_down());
        if released {
            self.split_dragging = false;
            self.split_drag_dy = 0.0;
        }
    }

    /// Remember the native window. A minimized frame is skipped.
    fn track_window(&mut self, ctx: &Context) {
        let (outer, inner, maximized, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.outer_rect.map(|r| (r.min.x, r.min.y)),
                v.inner_rect.map(|r| (r.width(), r.height())),
                v.maximized.unwrap_or(false),
                v.minimized.unwrap_or(false),
            )
        });
        let next = self.cfg.window.remember(outer, inner, maximized, minimized);
        if next != self.cfg.window {
            self.cfg.window = next;
            self.window_dirty = true;
            self.window_dirty_at = Instant::now();
        }
        if self.window_dirty && self.window_dirty_at.elapsed() >= Duration::from_millis(800) {
            self.flush_window();
        }
    }

    fn flush_window(&mut self) {
        if !self.window_dirty {
            return;
        }
        if let Err(e) = self.cfg.persist_window(&self.config_path) {
            self.log_event(format!("window: {e}"));
            return;
        }
        self.window_dirty = false;
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
            SessionClick::Ctext | SessionClick::CtextPick => self.pick_ctext_file(),
            SessionClick::Ascii => {
                let on = self.session.toggle_umlaut(self.active);
                self.status = if on {
                    "Umlaut conversion ON".into()
                } else {
                    "Umlaut conversion OFF".into()
                };
                self.persist_session_chrome();
            }
            SessionClick::LineStep => {
                let on = self.session.toggle_softscroll(self.active);
                self.status = softscroll_status(on);
                self.persist_session_chrome();
            }
            SessionClick::Echo => {
                let slot = slot_for(self.active);
                let on = self.session.toggle_echo(self.active);
                self.status = echo_status(slot, on);
                self.persist_session_chrome();
            }
            SessionClick::Bell => {
                let on = self.session.toggle_bell(self.active);
                self.status = if on {
                    "Sounds ON — connect, disconnect, newline".into()
                } else {
                    "Sounds OFF — connect, disconnect, newline".into()
                };
                self.persist_session_chrome();
            }
            SessionClick::Clear => self.clear_active_pad(),
            SessionClick::Spy(n) => {
                if n > self.spy_limit() {
                    return;
                }
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
                self.set_seven_plus_pane(self.active, None);
            }
            SessionClick::Remote => {
                let on = self.session.toggle_remote(self.active);
                self.status = if on {
                    "Remote ON — //RUN allowed".into()
                } else {
                    "Remote OFF — //RUN refused".into()
                };
                self.persist_session_chrome();
            }
            SessionClick::BinRx => {
                let busy = self.autobin.as_ref().is_some_and(|live| {
                    live.state.pending_offer().is_some() || live.state.recv_progress().is_some()
                });
                if busy {
                    self.cancel_autobin();
                } else {
                    self.session.bin_rx = !self.session.bin_rx;
                    self.status = format!("BIN-RX {}", on_off(self.session.bin_rx));
                }
            }
            SessionClick::BinTx => {
                let sending = self
                    .autobin
                    .as_ref()
                    .is_some_and(|live| live.state.tx_active());
                if sending {
                    self.cancel_bin_tx();
                } else {
                    self.start_bin_tx();
                }
            }
            SessionClick::Name => self.open_name_dialog(),
            SessionClick::Lock => {
                let ActivePane::Channel(n) = self.active else {
                    return;
                };
                if !self.channel_linked(n) {
                    return;
                }
                let on = self.channel_lock.get(n as usize).copied().unwrap_or(false);
                let next = !on;
                if let Some(flag) = self.channel_lock.get_mut(n as usize) {
                    *flag = next;
                }
                self.status = if next {
                    match self.linked_call(n) {
                        Some(peer) => format!(
                            "Lock ON — {} / {}",
                            cipher::base_call(&self.effective_mycall(n)),
                            cipher::base_call(&peer)
                        ),
                        None => "Lock ON".into(),
                    }
                } else {
                    "Lock OFF".into()
                };
            }
            SessionClick::ScrLock => {
                self.session.scr_lock = !self.session.scr_lock;
                self.status = format!("ScrLock {}", on_off(self.session.scr_lock));
                self.persist_session_chrome();
            }
            SessionClick::ScrollUp => self.on_scroll_arrow(1.0),
            SessionClick::ScrollDown => self.on_scroll_arrow(-1.0),
            SessionClick::Scrollbar => {
                self.session.scrollbar = !self.session.scrollbar;
                self.status = format!("scrollbar {}", on_off(self.session.scrollbar));
            }
        }
    }

    fn pick_ctext_file(&mut self) {
        let slot = self.ctext_slot();
        let current = self.ctext_path_for(slot);
        let start = current
            .parent()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| Some(crate::paths::data_dir()));
        let title = if slot == 0 {
            "Select Connect text (global)".to_string()
        } else {
            format!("Select Connect text — Channel {slot}")
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Connect text (GPI)", &["gpi", "GPI", "txt"])
            .set_title(&title);
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("CTEXT.GPI")
                .to_string();
            if slot == 0 {
                self.session.set_ctext_path(path);
                self.status = format!("CT global: {name}");
                self.log_event(format!("Connect text → {name}"));
            } else {
                self.session.set_channel_ctext(slot, path);
                if self.session.channel_ctext_path(slot).is_none() {
                    self.status = format!("CT channel {slot}: global");
                    self.log_event(format!("Connect text channel {slot} → global"));
                } else {
                    self.status = format!("CT channel {slot}: {name}");
                    self.log_event(format!("Connect text channel {slot} → {name}"));
                }
            }
            self.persist_session_chrome();
        }
    }

    fn qrg_dialog(&mut self, ctx: &Context) {
        if !self.session.show_qrg_dialog {
            return;
        }
        let mut open = true;
        centered_popup("QRG — frequency (MHz)", egui::Id::new("gp_qrg"), &mut open).show(
            ctx,
            |ui| {
                ui.set_min_width(280.0);
                ui.label(RichText::new("Frequency in MHz").font(menu_font(16.0)));
                let edit = retro::field(
                    ui,
                    &mut self.session.qrg_edit,
                    ui.available_width(),
                    "27.235",
                    menu_font(17.0),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if retro::apply(ui, false, menu_font(20.0)) || enter {
                    match self.session.set_qrg_from_edit() {
                        Ok(()) => {
                            self.status = self.session.qrg_label();
                            self.log_event(self.session.qrg_label());
                            self.persist_session_chrome();
                        }
                        Err(e) => self.status = e,
                    }
                }
            },
        );
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
        let joined_global = slot != 0 && edited.trim().is_empty();
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
        } else if joined_global {
            format!("channel {slot}, global pool")
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
        let slot = slot.clamp(1, self.spy_limit());
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

    /// GP DOS `MAXSPY`: 1..=8 spy windows. Default is 1.
    fn spy_limit(&self) -> u8 {
        self.cfg.session.max_spy.clamp(1, 8) as u8
    }

    fn first_empty_spy_slot(&self) -> Option<u8> {
        let limit = self.spy_limit() as usize;
        self.cfg
            .session
            .spy_calls
            .iter()
            .take(limit)
            .enumerate()
            .find(|(_, c)| c.is_empty())
            .map(|(i, _)| (i as u8) + 1)
    }

    /// First disconnected channel `1..=channels`.
    fn first_free_channel(&self) -> Option<u8> {
        (1..=self.channels).find(|&n| !self.channel_connected(n))
    }

    /// `CALL`, `CALL,VIA`, or a `NAMES.GP` call / ident / name.
    fn resolve_connect_target(
        &self,
        raw: &str,
    ) -> Result<(String, Vec<String>, Vec<String>), String> {
        let raw = raw.trim();
        if raw.contains(',')
            || raw
                .split_whitespace()
                .any(|w| w.eq_ignore_ascii_case("via"))
        {
            let (dest, vias) = Self::parse_connect_target(raw)?;
            return Ok((dest, vias, Vec::new()));
        }
        match self.names.route_for(raw, &self.names_qrg()) {
            Ok(Some(route)) => Ok(route_parts(route)),
            Ok(None) => {
                let (dest, vias) = Self::parse_connect_target(raw)?;
                Ok((dest, vias, Vec::new()))
            }
            Err(err) => Err(err),
        }
    }

    /// Parse `CALL`, `CALL,VIA`, or `CALL via VIA` into dest + digi path.
    fn parse_connect_target(raw: &str) -> Result<(String, Vec<String>), String> {
        let parts = ax25::split_connect_hops(raw);
        if parts.is_empty() {
            return Err("connect: empty call".into());
        }
        let dest = crate::config::normalize_mycall(&parts[0])?;
        let mut vias = Vec::new();
        for p in &parts[1..] {
            vias.push(crate::config::normalize_mycall(p)?);
        }
        Ok((dest, vias))
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
        match self.resolve_connect_target(&call) {
            Ok((dest, vias, down)) => self.begin_connect(ch, &dest, &vias, down),
            Err(err) => self.status = err,
        }
    }

    fn begin_connect(&mut self, channel: u8, dest: &str, vias: &[String], downlinks: Vec<String>) {
        if !(1..=self.channels).contains(&channel) || self.channel_connected(channel) {
            self.status = format!("connect: channel {channel} is unavailable");
            return;
        }
        let pool = self.pool_name(channel);
        let mut local = self.air_call_for_peer(&pool, dest, Some(channel));
        if self.software_ax25() && local.eq_ignore_ascii_case(dest) && vias.is_empty() {
            // A local test still needs two distinct AX.25 endpoints.
            let base = cipher::base_call(&local);
            let Some(call) = (1..=15).map(|ssid| format!("{base}-{ssid}")).find(|call| {
                !(1..=self.channels).any(|ch| {
                    ch != channel
                        && self
                            .channel_other_call(ch)
                            .is_some_and(|peer| peer.eq_ignore_ascii_case(dest))
                        && self.effective_mycall(ch).eq_ignore_ascii_case(call)
                })
            }) else {
                self.status = "connect: no unused source SSID".into();
                return;
            };
            local = call;
        }
        self.adopt_air_call(channel, &local);
        if !self.configure_sessions()
            || !self.send_session_command(SessionCommand::Connect {
                channel,
                port: self.cfg.tnc.channel_port(channel),
                local: local.clone(),
                remote: dest.into(),
                via: vias.to_vec(),
            })
        {
            return;
        }
        let idx = usize::from(channel);
        self.channel_connecting[idx] = Some(dest.to_ascii_uppercase());
        self.channel_via[idx] = vias.to_vec();
        self.channel_down[idx] = downlinks;
        self.channel_down_wait[idx] = None;
        self.set_link_outgoing(channel, true);
        self.clear_lock(channel);
        self.session.spy_slot = None;
        self.active = ActivePane::Channel(channel);
        self.status = format!(
            "Connecting {local} to {dest} on port {}",
            self.cfg.tnc.channel_port(channel)
        );
        self.pads[idx].push(PadKind::System, format!("*** {}", self.status));
    }

    fn open_connect_dialog_for_channel(&mut self, channel: u8) {
        let ch = channel.clamp(1, self.channels);
        self.session.open_connect_dialog(ch, "");
        self.connect_dialog_focus = true;
        self.show_connect_names = false;
        self.status = format!("Connect — enter call for ch{ch}");
    }

    fn apply_connect_from_edit(&mut self) -> Result<(), String> {
        let ch = self.session.connect_channel.clamp(1, self.channels);
        let (dest, vias, down) = self.resolve_connect_target(&self.session.connect_edit)?;
        self.session.show_connect_dialog = false;
        self.begin_connect(ch, &dest, &vias, down);
        Ok(())
    }

    fn peer_name_label(&self, channel: u8) -> (String, bool) {
        if self.channel_linked(channel) {
            if let Some(name) = self
                .linked_call(channel)
                .and_then(|call| self.names.name_for(&call, &self.names_qrg()))
            {
                return (format!("Name: {name}"), true);
            }
        }
        ("Name".into(), false)
    }

    fn open_name_dialog(&mut self) {
        let ActivePane::Channel(ch) = self.active else {
            self.status = "Name — select a channel".into();
            return;
        };
        let call = self.linked_call(ch).unwrap_or_default();
        let qrg = self.names_qrg();
        let entries = self.names.entries();
        let existing = entries
            .iter()
            .find(|(_, entry)| entry.call.eq_ignore_ascii_case(&call) && entry.scope.matches(&qrg))
            .or_else(|| {
                entries.iter().find(|(_, entry)| {
                    cipher::base_call(&entry.call).eq_ignore_ascii_case(&cipher::base_call(&call))
                        && !call.is_empty()
                        && entry.scope.matches(&qrg)
                })
            });
        if let Some((index, entry)) = existing {
            self.name_source = Some(*index);
            self.name_edit = entry.clone();
        } else {
            self.name_source = None;
            self.name_edit = NameEntry {
                call,
                via: self.link_vias(ch).join(" "),
                ..NameEntry::default()
            };
        }
        self.name_error.clear();
        self.name_channel = ch;
        self.show_name_dialog = true;
        self.name_dialog_focus = true;
        self.status = "NAMES.GP station editor".into();
    }

    fn apply_name_from_edit(&mut self) -> Result<(), String> {
        self.names.save_entry(self.name_source, &self.name_edit)?;
        self.show_name_dialog = false;
        self.status = format!(
            "NAMES.GP {} saved",
            self.name_edit.call.to_ascii_uppercase()
        );
        Ok(())
    }

    fn remove_name_from_edit(&mut self) -> Result<(), String> {
        let source = self
            .name_source
            .ok_or_else(|| "NAMES.GP: select a saved station to remove".to_string())?;
        let call = self.names.remove_entry(source)?;
        self.name_source = None;
        self.name_edit = NameEntry::default();
        self.name_error.clear();
        self.name_dialog_focus = true;
        self.status = format!("NAMES.GP {call} removed");
        Ok(())
    }

    fn name_dialog(&mut self, ctx: &Context) {
        if !self.show_name_dialog {
            return;
        }
        let mut open = true;
        centered_popup(
            "NAMES.GP station editor",
            egui::Id::new(("gp_name_dialog", self.name_channel)),
            &mut open,
        )
        .show(ctx, |ui| {
            ui.set_min_width(440.0);
            let entries = self.names.entries();
            name_selector(
                ui,
                "name_entry",
                self.name_source
                    .and_then(|index| {
                        entries
                            .iter()
                            .find(|(i, _)| *i == index)
                            .map(|(_, entry)| format!("{} - {}", entry.call, entry.scope.label()))
                    })
                    .unwrap_or_else(|| "New station".into()),
                |ui| {
                    if name_select_row(ui, "New station", self.name_source.is_none(), false)
                        .clicked()
                    {
                        self.name_source = None;
                        self.name_edit = NameEntry::default();
                        self.name_error.clear();
                    }
                    for (index, entry) in &entries {
                        if name_select_row(
                            ui,
                            &format!("{} ({}) - {}", entry.call, entry.name, entry.scope.label()),
                            self.name_source == Some(*index),
                            false,
                        )
                        .clicked()
                        {
                            self.name_source = Some(*index);
                            self.name_edit = entry.clone();
                            self.name_error.clear();
                        }
                    }
                },
            );
            let mut enter = false;
            for (label, text, hint) in [
                ("CALL", &mut self.name_edit.call, "N0CALL"),
                ("NAME", &mut self.name_edit.name, "Name (22 characters)"),
                (
                    "VIA",
                    &mut self.name_edit.via,
                    "DB0ABC or N>DB0ABC D>DB0XYZ",
                ),
                ("IDENT (optional)", &mut self.name_edit.ident, "Alias"),
            ] {
                ui.label(RichText::new(label).font(menu_font(16.0)));
                let te = retro::field(ui, text, ui.available_width(), hint, menu_font(17.0));
                if self.name_dialog_focus && label == "CALL" {
                    te.request_focus();
                    self.name_dialog_focus = false;
                }
                enter |= te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            ui.label(RichText::new("Station type").font(menu_font(16.0)));
            name_selector(ui, "name_kind", self.name_edit.kind.label(), |ui| {
                for kind in StationKind::ALL {
                    if name_select_row(ui, kind.label(), self.name_edit.kind == kind, false)
                        .clicked()
                    {
                        self.name_edit.kind = kind;
                    }
                }
            });
            ui.label(RichText::new("QRG band / integer MHz").font(menu_font(16.0)));
            name_selector(ui, "name_scope", self.name_edit.scope.label(), |ui| {
                if name_select_row(
                    ui,
                    "All QRGs",
                    self.name_edit.scope == NameScope::All,
                    false,
                )
                .clicked()
                {
                    self.name_edit.scope = NameScope::All;
                }
                let current = qrg_key(&self.names_qrg()).to_string();
                if name_select_row(
                    ui,
                    &format!("Current QRG ({current})"),
                    self.name_edit.scope == NameScope::Qrg(current.clone()),
                    false,
                )
                .clicked()
                {
                    self.name_edit.scope = NameScope::Qrg(current);
                }
                for (label, key) in [
                    ("160 m - 1 MHz", "1"),
                    ("80 m - 3 MHz", "3"),
                    ("60 m - 5 MHz", "5"),
                    ("40 m - 7 MHz", "7"),
                    ("30 m - 10 MHz", "10"),
                    ("20 m - 14 MHz", "14"),
                    ("17 m - 18 MHz", "18"),
                    ("15 m - 21 MHz", "21"),
                    ("12 m - 24 MHz", "24"),
                    ("11 m - 27 MHz", "27"),
                    ("10 m - 28 MHz", "28"),
                    ("10 m - 29 MHz", "29"),
                    ("6 m - 50 MHz", "50"),
                    ("4 m - 70 MHz", "70"),
                    ("2 m - 144 MHz", "144"),
                    ("2 m - 145 MHz", "145"),
                    ("2 m - 146 MHz", "146"),
                    ("1.25 m - 222 MHz", "222"),
                    ("70 cm - 430 MHz", "430"),
                    ("70 cm - 433 MHz", "433"),
                    ("70 cm - 440 MHz", "440"),
                    ("23 cm - 1240 MHz", "1240"),
                ] {
                    if name_select_row(
                        ui,
                        label,
                        self.name_edit.scope == NameScope::Qrg(key.into()),
                        false,
                    )
                    .clicked()
                    {
                        self.name_edit.scope = NameScope::Qrg(key.into());
                    }
                }
            });
            match &mut self.name_edit.scope {
                NameScope::Qrg(key) | NameScope::OtherQrg(key) => {
                    ui.label("MHz number (custom QRG allowed; only the part before . is used)");
                    retro::field(ui, key, ui.available_width(), "145", menu_font(17.0));
                }
                NameScope::All => {}
            }
            ui.label("VIA: plain calls are L2 digis; TYPE>CALL preserves node routing.");
            if self.name_source.is_some()
                && retro::button(
                    ui,
                    "Remove selected station",
                    ui.available_width(),
                    26.0,
                    menu_font(17.0),
                )
            {
                if let Err(e) = self.remove_name_from_edit() {
                    self.name_error = e.clone();
                    self.status = e;
                }
            }
            if !self.name_error.is_empty() {
                ui.label(RichText::new(&self.name_error).color(Color32::DARK_RED));
            }
            if retro::apply(ui, false, menu_font(20.0)) || enter {
                if let Err(e) = self.apply_name_from_edit() {
                    self.name_error = e.clone();
                    self.status = e;
                }
            }
        });
        if !open {
            self.show_name_dialog = false;
            self.status = "Name cancelled".into();
        }
    }

    fn connect_dialog(&mut self, ctx: &Context) {
        if !self.session.show_connect_dialog {
            self.show_connect_names = false;
            return;
        }
        let ch = self.session.connect_channel;
        let mut open = true;
        centered_popup(
            format!("Connect — channel {ch}"),
            egui::Id::new(("gp_connect_dialog", ch)),
            &mut open,
        )
        .show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(RichText::new("Whom to connect to?").font(menu_font(18.0)));
            ui.label(
                RichText::new("Call, a name from NAMES.GP, CALL,VIA, or CALL via DIGI")
                    .color(Color32::BLACK)
                    .font(menu_font(16.0)),
            );
            let te = retro::field(
                ui,
                &mut self.session.connect_edit,
                ui.available_width(),
                "N0CALL",
                menu_font(17.0),
            );
            if self.connect_dialog_focus {
                te.request_focus();
                self.connect_dialog_focus = false;
            }
            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(6.0);
            self.connect_name_field(ui);
            if retro::apply(ui, false, menu_font(20.0)) || enter {
                if let Err(e) = self.apply_connect_from_edit() {
                    self.status = e;
                }
            }
        });
        if !open {
            self.session.show_connect_dialog = false;
            self.show_connect_names = false;
            self.status = "Connect cancelled".into();
        }
    }

    /// The Name field on the connect dialog. A click opens `NAMES.GP`.
    fn connect_name_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Name").font(menu_font(16.0)));
        let width = ui.available_width().max(220.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
        let fill = if resp.hovered() || self.show_connect_names {
            Color32::from_rgb(214, 214, 214)
        } else {
            Color32::from_rgb(236, 236, 236)
        };
        retro::paint(ui.painter(), rect, fill, true, retro::ROW_ROUND);
        let text = if self.show_connect_names {
            "list open"
        } else {
            "click for NAMES.GP"
        };
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y - 2.0),
            egui::Align2::CENTER_CENTER,
            text,
            menu_font(17.0),
            Color32::BLACK,
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            if self.show_connect_names {
                self.show_connect_names = false;
            } else {
                self.open_connect_name_list();
            }
        }
    }

    fn open_connect_name_list(&mut self) {
        let path = self.names.path().to_path_buf();
        self.names = NamesFile::load_from(path);
        self.show_connect_names = true;
    }

    /// Click a stored station to connect. The button shows the call, and the
    /// name in parentheses when one is stored. Global names precede the active
    /// QRG list; selecting a row uses that entry's complete saved route.
    fn connect_names_window(&mut self, ctx: &Context) {
        if !self.show_connect_names || !self.session.show_connect_dialog {
            return;
        }
        let qrg = self.names_qrg();
        let picks = self.names.name_list(&qrg);
        let qrg_heading = format!("{}MHz List", qrg_key(&qrg));
        let mut chosen: Option<ListedName> = None;
        let mut open = true;
        centered_popup(
            "Names",
            egui::Id::new(("gp_connect_names", self.session.connect_channel)),
            &mut open,
        )
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            let gap = 4.0;
            let rows = 10.0;
            let max_h = retro::ROW_H * rows + gap * (rows - 1.0);
            egui::ScrollArea::vertical()
                .max_height(max_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = gap;
                    let width = ui.available_width().max(320.0);
                    for (global, heading) in [(true, "All QRGs"), (false, qrg_heading.as_str())] {
                        ui.label(RichText::new(heading).font(menu_font(16.0)));
                        let entries: Vec<_> = picks
                            .iter()
                            .filter(|entry| (entry.scope == NameScope::All) == global)
                            .collect();
                        if entries.is_empty() {
                            ui.label(RichText::new("No stations").font(menu_font(14.0)));
                        }
                        for entry in entries {
                            let row = retro::entry(
                                ui,
                                &entry.button_label(),
                                "",
                                Color32::TRANSPARENT,
                                false,
                                width,
                                menu_font(17.0),
                            );
                            if row.clicked() && !row.double_clicked() {
                                chosen = Some(entry.clone());
                            }
                        }
                    }
                });
        });
        if !open {
            self.show_connect_names = false;
        }
        if let Some(entry) = chosen {
            self.connect_listed(&entry);
        }
    }

    fn connect_listed(&mut self, entry: &ListedName) {
        let route = match &entry.route {
            Ok(route) => route.clone(),
            Err(error) => {
                self.status = error.clone();
                return;
            }
        };
        let ch = self.session.connect_channel.clamp(1, self.channels);
        self.show_connect_names = false;
        self.session.show_connect_dialog = false;
        let (dest, via, downlinks) = route_parts(route);
        self.begin_connect(ch, &dest, &via, downlinks);
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
        centered_popup("Start Spy", egui::Id::new("gp_mheard_spy"), &mut open).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(
                RichText::new(format!("Start Spy for {call}"))
                    .color(Color32::BLACK)
                    .font(menu_font(18.0)),
            );
            if retro::choice(ui, "Yes — start Spy", menu_font(17.0)) {
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
            if retro::choice(ui, "No — pick another", menu_font(17.0)) {
                self.show_mheard_spy_confirm = false;
                self.mheard_spy_pending.clear();
                self.status = "Select a call in MHeard for Spy".into();
            }
            if retro::choice(ui, "Abort", menu_font(17.0)) {
                self.show_mheard_spy_confirm = false;
                self.mheard_spy_pick = false;
                self.mheard_spy_pending.clear();
                self.status = "Spy from MHeard aborted".into();
            }
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
        self.clear_spy_auto_slot(idx);
        if let Err(e) = self.cfg.persist_spy_calls(&self.config_path) {
            self.status = format!("config: {e}");
        } else {
            self.status = format!("Spy {slot} cleared");
        }
    }

    fn apply_spy_call_from_edit(&mut self) -> Result<(), String> {
        let slot = self
            .session
            .spy_slot
            .unwrap_or(1)
            .clamp(1, self.spy_limit());
        let raw = self.session.spy_edit.trim().to_string();
        self.cfg.normalize_spy_calls();
        let idx = slot.saturating_sub(1) as usize;
        // Manual SPY dialog — always `CALL.spy`, not AutoSpy `.NNN`.
        self.clear_spy_auto_slot(idx);
        if raw.is_empty() {
            if let Some(c) = self.cfg.session.spy_calls.get_mut(idx) {
                c.clear();
            }
        } else {
            let call = crate::config::normalize_mycall(&raw)?;
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
        centered_popup(
            format!("SPY — window {slot}"),
            egui::Id::new("gp_spy"),
            &mut open,
        )
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(
                RichText::new("Call to spy. Leave empty to turn it off.").font(menu_font(16.0)),
            );
            retro::field(
                ui,
                &mut self.session.spy_edit,
                ui.available_width(),
                "",
                menu_font(17.0),
            );
            if retro::apply(ui, false, menu_font(20.0)) {
                if let Err(e) = self.apply_spy_call_from_edit() {
                    self.status = e;
                }
            }
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
        centered_popup(title, egui::Id::new("gp_mycall"), &mut open).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(RichText::new("Call sign").font(menu_font(16.0)));
            retro::field(
                ui,
                &mut self.session.mycall_edit,
                ui.available_width(),
                "",
                menu_font(17.0),
            );
            if self.session.mycall_slot != 0 {
                ui.label(
                    RichText::new("Empty, or the same as the global call, joins the global pool.")
                        .font(menu_font(16.0)),
                );
            }
            if retro::apply(ui, false, menu_font(20.0)) {
                if let Err(e) = self.apply_mycall_from_edit() {
                    self.status = e;
                }
            }
        });
        if !open {
            self.session.show_mycall_dialog = false;
        }
    }

    fn icon_strip(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, self.menu_bg);

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
            ActivePane::Channel(n) => toolbar_for_channel(self.channel_connected(n)),
            other => toolbar_for(other),
        };
        let n = toolbar.len().max(1);
        let ppp = ui.ctx().pixels_per_point();
        let size = icon_display_size(ppp);
        let icon_w = size.x;
        let icon_h = size.y;
        let gap = ICON_STRIP_GAP;
        // Fixed 5px of the menu colour between icons; center the row when the strip is wider.
        let row_w = n as f32 * icon_w + (n.saturating_sub(1) as f32) * gap;
        let start_x = snap_to_pixel(rect.left() + ((rect.width() - row_w) * 0.5).max(0.0), ppp);
        let y = snap_to_pixel(rect.top() + ((rect.height() - icon_h) * 0.5).max(0.0), ppp);

        let mut clicked: Option<&'static str> = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            let mut x = start_x;
            for &icon_idx in toolbar {
                let label = ICON_SLOT_LABELS.get(icon_idx).copied().unwrap_or("?");
                let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::new(icon_w, icon_h));
                let id = ui.id().with(("icon", icon_idx, label));
                let resp = ui.interact(cell, id, Sense::click());
                // Grey raised button face (classic GP), then the bitmap.
                // EMPTY (slot 8) is the Map button on MHeard/Map — still has art in ICONS.GP.
                ui.painter().rect_filled(cell, 0.0, VGA_GRAY);
                if let Some(tex) = self.icon_tex.get(icon_idx).cloned() {
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
                    ("EMPTY", ActivePane::Map) => true,
                    ("SPY", ActivePane::Spy(_)) => true,
                    ("SPY", ActivePane::Mh) if self.mheard_spy_pick => true,
                    ("CHECK", ActivePane::Check) | ("BBS", ActivePane::Check) => true,
                    ("LIST", ActivePane::List) => true,
                    _ => false,
                };
                // DOS 3D: active / held → sunken (black TL, white BR); else raised.
                paint_icon_bevel(
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
                "SETUP" => self.open_setup(),
                "SORT" => {
                    if matches!(self.active, ActivePane::Check) {
                        self.sort_bbs();
                    } else {
                        self.status = "Sort is the BBS menu".into();
                    }
                }
                "FIND" => {
                    if matches!(self.active, ActivePane::Check | ActivePane::List) {
                        self.find_open = true;
                        self.find_text.clear();
                    } else {
                        self.status = "Find is the BBS menu or the Box list".into();
                    }
                }
                "MONITOR" => self.select(ActivePane::Monitor),
                "EDIT" => self.open_ctext_editor(),
                "SAVE" => self.save_active(),
                "LOAD" => self.editor_load_dialog(),
                "MHEARD" => self.select(ActivePane::Mh),
                "EMPTY" => {
                    // Map button (EMPTY slot on MHeard / Map strips).
                    // World fit runs once per session on first paint (`map_needs_world_fit`).
                    self.select(ActivePane::Map);
                }
                "SPY" => {
                    if matches!(self.active, ActivePane::Mh) {
                        self.spy_from_mheard();
                    } else {
                        self.open_spy_call_dialog();
                    }
                }
                "KILL SPY" => self.kill_spy_call(),
                "EXIT" => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                "DOS" => self.status = self.beacon_sched.send_now(),
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

/// Complete lines plus any trailing fragment after a multi-line compose paste.
fn split_compose_paste(normalized: &str) -> (Vec<String>, String) {
    let ends_nl = normalized.ends_with('\n');
    let mut parts: Vec<String> = normalized.lines().map(|s| s.to_string()).collect();
    let remainder = if ends_nl {
        String::new()
    } else {
        parts.pop().unwrap_or_default()
    };
    (parts, remainder)
}

fn autobin_safe_name(raw: &str) -> String {
    let base = raw.rsplit(['\\', '/']).next().unwrap_or("").trim();
    let safe: String = base
        .chars()
        .filter(|c| {
            c.is_ascii() && !matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | ':' | '\\' | '/')
        })
        .collect();
    if safe.is_empty() || safe == "." || safe == ".." {
        "autobin.bin".to_string()
    } else {
        safe
    }
}

fn replace_part_file(part: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    std::fs::rename(part, dest)
}

/// `<GP>:` / `<GPNG>:` inside a frame that may still start with file bytes.
fn gp_notice_from(info: &[u8]) -> Option<Vec<u8>> {
    let at = info
        .windows(4)
        .position(|w| w == b"<GP>")
        .or_else(|| info.windows(6).position(|w| w == b"<GPNG>"));
    at.map(|i| info[i..].to_vec())
}

fn autobin_offer_label(offer: &crate::transfer::autobin::BinOffer) -> String {
    let name = offer.filename.as_deref().unwrap_or("file");
    format!("{name}  {} bytes", offer.length)
}

/// Drop `#BIN#` lines. `None` when nothing else was in the frame.
fn strip_bin_headers(info: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut dropped = false;
    let mut i = 0;
    while i < info.len() {
        let start = i;
        while i < info.len() && info[i] != b'\r' && info[i] != b'\n' {
            i += 1;
        }
        let line = &info[start..i];
        let offer = line.starts_with(b"#BIN#") || line.starts_with(b"#BIN_V2#");
        let mut sep = 0;
        if i < info.len() && info[i] == b'\r' {
            sep = 1;
            if i + 1 < info.len() && info[i + 1] == b'\n' {
                sep = 2;
            }
        } else if i < info.len() && info[i] == b'\n' {
            sep = 1;
        }
        if offer {
            dropped = true;
        } else {
            out.extend_from_slice(line);
            if sep > 0 {
                out.extend_from_slice(&info[i..i + sep]);
            }
        }
        if sep == 0 {
            break;
        }
        i += sep;
    }
    if !dropped {
        return Some(info.to_vec());
    }
    if out
        .iter()
        .all(|b| matches!(b, b'\r' | b'\n' | b' ' | b'\t'))
    {
        None
    } else {
        Some(out)
    }
}

struct LiveAutoBin {
    channel: u8,
    state: AutoBinState,
    started: Instant,
    /// N(S) of the file chunk in flight. The next chunk waits for N(R).
    tx_id: Option<u64>,
    /// Part file the receive is streaming into. Renamed when the length is in.
    recv_file: Option<std::fs::File>,
    recv_part: Option<PathBuf>,
    recv_final: Option<PathBuf>,
}

/// 7Plus part on the wire (`//Read *.p??` / `*.7pl`), paced like AutoBin TX.
struct LiveSevenTx {
    channel: u8,
    name: String,
    data: Vec<u8>,
    offset: usize,
    started: Instant,
    tx_id: Option<u64>,
}

/// One frame of the Transfer80 purple strip.
struct TransferView {
    path: String,
    got: u64,
    total: u64,
    status: &'static str,
    waiting: bool,
    started: Instant,
}

fn truncate_path_for_bar(path: &str, max_chars: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_chars {
        return path.to_string();
    }
    if max_chars <= 3 {
        return chars.into_iter().take(max_chars).collect();
    }
    let keep = max_chars - 3;
    let mut out: String = chars[chars.len() - keep..].iter().collect();
    out.insert_str(0, "...");
    out
}

fn paint_status_cell(painter: &egui::Painter, cell: Rect, text: &str, font: &FontId, fg: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), fg);
    let pos = Pos2::new(
        cell.left() + 6.0,
        cell.center().y - galley.size().y * 0.5 - 2.0,
    );
    painter.galley(pos, galley, fg);
}

/// DOS Transfer80 green block bar with a percent box at the end.
fn paint_dos_progress_bar(painter: &egui::Painter, area: Rect, pct: f32, fg: Color32) {
    let pct_w = 44.0_f32.min(area.width() * 0.18);
    let gap = 3.0_f32;
    let bar = Rect::from_min_max(
        area.left_top(),
        Pos2::new(
            (area.right() - pct_w - gap).max(area.left() + 20.0),
            area.bottom(),
        ),
    );
    let pct_box = Rect::from_min_max(
        Pos2::new(bar.right() + gap, area.top()),
        Pos2::new(area.right(), area.bottom()),
    );
    let border = Color32::from_rgb(160, 160, 160);
    let empty = Color32::from_rgb(64, 64, 64);
    painter.rect_stroke(bar, 0.0, Stroke::new(1.0, border));
    painter.rect_stroke(pct_box, 0.0, Stroke::new(1.0, border));
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(pct_box.left() + 1.0, pct_box.top() + 1.0),
            Pos2::new(pct_box.right() - 1.0, pct_box.bottom() - 1.0),
        ),
        0.0,
        empty,
    );

    let inner = Rect::from_min_max(
        Pos2::new(bar.left() + 2.0, bar.top() + 2.0),
        Pos2::new(bar.right() - 2.0, bar.bottom() - 2.0),
    );
    let blocks = ((inner.width() / 8.0).floor() as i32).clamp(8, 40);
    let block_w = inner.width() / blocks as f32;
    let filled = ((pct.clamp(0.0, 1.0) * blocks as f32).round() as i32).clamp(0, blocks);
    for i in 0..blocks {
        let x0 = inner.left() + i as f32 * block_w;
        let cell = Rect::from_min_max(
            Pos2::new(x0 + 0.5, inner.top()),
            Pos2::new(x0 + block_w - 0.5, inner.bottom()),
        );
        painter.rect_filled(cell, 0.0, if i < filled { VGA_GREEN } else { empty });
    }
    painter.text(
        pct_box.center(),
        egui::Align2::CENTER_CENTER,
        format!("{:.0}%", pct * 100.0),
        FontId::monospace(12.0),
        fg,
    );
}

fn file_ftime(path: &std::path::Path) -> u32 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let Ok(modified) = meta.modified() else {
        return 0;
    };
    let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return 0;
    };
    let Some(utc) = chrono::DateTime::from_timestamp(dur.as_secs() as i64, 0) else {
        return 0;
    };
    let local = utc.with_timezone(&Local);
    autobin::pack_ftime(
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListOffer {
    list: bool,
    rows: Vec<MailEntry>,
    channel: u8,
}

/// Call this station answers with for one more link to `peer`.
///
/// `circuits` are `(our call, their call)`. The same two calls in either
/// order are one pair. SSID 0 is the bare pool name. The first extra
/// link of that pair is `-1`, then the lowest number still free. A
/// different peer stays on the bare name. Nothing here is a channel number.
fn air_call_for_pair(pool: &str, peer: &str, circuits: &[(&str, &str)]) -> String {
    call_with_ssid(pool, pair_ssid(pool, peer, circuits))
}

fn pair_ssid(pool: &str, peer: &str, circuits: &[(&str, &str)]) -> u8 {
    let mut used = [false; 16];
    for &(a, b) in circuits {
        let Some(our) = our_call_in_circuit(pool, peer, a, b) else {
            continue;
        };
        let Some(ssid) = occupied_ssid(our, pool) else {
            continue;
        };
        if (ssid as usize) < used.len() {
            used[ssid as usize] = true;
        }
    }
    let mut n = 0u8;
    while n < 15 && used[n as usize] {
        n += 1;
    }
    n
}

/// End of the circuit that is this station, when the other end is `peer`.
fn our_call_in_circuit<'a>(pool: &str, peer: &str, a: &'a str, b: &'a str) -> Option<&'a str> {
    let base = cipher::base_call(pool);
    let ours = |call: &str| cipher::base_call(call).eq_ignore_ascii_case(&base);
    let is_peer = |call: &str| call.eq_ignore_ascii_case(peer);
    if ours(a) && is_peer(b) {
        Some(a)
    } else if ours(b) && is_peer(a) {
        Some(b)
    } else {
        None
    }
}

/// SSID this call occupies in `pool`. The configured pool name is 0,
/// including when that name itself has no numeric suffix.
fn occupied_ssid(call: &str, pool: &str) -> Option<u8> {
    let base = cipher::base_call(pool);
    if !cipher::base_call(call).eq_ignore_ascii_case(&base) {
        return None;
    }
    if call.eq_ignore_ascii_case(pool) || call.eq_ignore_ascii_case(&base) {
        return Some(0);
    }
    let (_, ssid) = call.split_once('-')?;
    let n: u8 = ssid.parse().ok()?;
    if n > 15 {
        None
    } else {
        Some(n)
    }
}

fn call_with_ssid(pool: &str, ssid: u8) -> String {
    let base = cipher::base_call(pool);
    if ssid == 0 {
        if pool.eq_ignore_ascii_case(&base) {
            base
        } else {
            pool.to_ascii_uppercase()
        }
    } else {
        format!("{base}-{ssid}")
    }
}

enum ListAnswer {
    Replace,
    Append,
    Interrupt,
}

enum CellGlyph {
    Text,
    TriUp,
    TriDown,
    /// Soft-scroll mark: three steps rising to the right.
    Stairs,
    /// Channel lock. Drawn; the session font has no key emoji.
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SessionClick {
    Qrg,
    /// Choose the connect-text file (`CTEXT.GPI`, `CTEXT_A.GPI`, …).
    Ctext,
    /// Right-click on CT. Same file picker.
    CtextPick,
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
    Name,
    Lock,
    ScrLock,
    ScrollUp,
    ScrollDown,
    Scrollbar,
}

fn route_parts(route: Route) -> (String, Vec<String>, Vec<String>) {
    (route.link_call, route.link_digis, route.downlinks)
}

/// `C DB0DOZ` / `C! ON5PL` / `C DH1DAE DB0FN` → the station we are waiting to see.
fn downlink_target(cmd: &str) -> String {
    let rest = cmd.trim();
    let rest = rest
        .strip_prefix("C!")
        .or_else(|| rest.strip_prefix("c!"))
        .unwrap_or(rest);
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix('C')
        .or_else(|| rest.strip_prefix('c'))
        .unwrap_or(rest);
    rest.split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

/// `CB0MVB-2` → `CB0MVB`. Empty when the token is not a call.
fn transcript_call(call: &str) -> String {
    let call = crate::journal::base_call(call);
    call.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// A name with no extension, or an extension other than `log`, becomes `*.LOG`.
fn with_log_extension(path: std::path::PathBuf) -> std::path::PathBuf {
    let keep = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("log"));
    if keep {
        return path;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".LOG");
    path.with_file_name(name)
}

/// Next free AutoSpy file `CALL.000` … `CALL.999` in `SpyDir`.
fn next_spy_nnn(dir: &std::path::Path, call: &str) -> Option<u16> {
    for n in 0u16..=999 {
        let path = dir.join(format!("{call}.{n:03}"));
        if !path.exists() {
            return Some(n);
        }
    }
    None
}

/// `ch call` lines. Missing file is an empty list.
fn load_last_calls(path: &std::path::Path, n: usize) -> Vec<Option<String>> {
    use std::io::BufRead;
    let mut out = vec![None; n];
    let Ok(file) = std::fs::File::open(path) else {
        return out;
    };
    for raw in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let mut parts = raw.split_whitespace();
        let Some(idx) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
            continue;
        };
        let Some(call) = parts.next() else {
            continue;
        };
        if idx < out.len() {
            let call = transcript_call(call);
            if !call.is_empty() {
                out[idx] = Some(call);
            }
        }
    }
    out
}

/// Keep a cached open-time banner, but stamp the call that asked `//P`.
fn rewrite_timing_mycall(banner: &str, mycall: &str) -> String {
    const PREFIX: &str = "Timing-Parameters of ";
    let Some(rest) = banner.strip_prefix(PREFIX) else {
        return banner.to_string();
    };
    let Some(colon) = rest.find(':') else {
        return banner.to_string();
    };
    format!("{PREFIX}{mycall}{}", &rest[colon..])
}

fn on_off(v: bool) -> &'static str {
    if v {
        "ON"
    } else {
        "OFF"
    }
}

fn echo_status(channel: u8, on: bool) -> String {
    if channel == 0 {
        return format!("Echo {}", on_off(on));
    }
    if on {
        format!("Echo channel {channel} ON — your text and the other station")
    } else {
        format!("Echo channel {channel} OFF — only the other station")
    }
}

/// Same connect-text file under different absolute/relative spellings.
fn paths_same_connect_text(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    match (a.file_name(), b.file_name()) {
        (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
        _ => false,
    }
}

/// Each byte becomes the Unicode scalar of the same value (DOS Latin-1).
fn latin1_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| char::from_u32(u32::from(b)).unwrap_or('?'))
        .collect()
}

/// Idle age for `//Activity`: seconds, then minutes, then hours.
fn activity_age_phrase(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} second(s)")
    } else if secs < 3600 {
        format!("{} minute(s)", secs / 60)
    } else {
        format!("{} hour(s)", secs / 3600)
    }
}

/// Local 24-hour `hh:mm` with leading zeros, inserted after `fm`.
fn is_fm_header(line: &str) -> bool {
    ax25::split_monitor_fm(line).is_some()
}

/// InsertMonLine: blank row before a new `fm` packet when the pad already has
/// a non-empty last line (payload or previous header).
fn should_insert_mon_blank(line: &str, prev: Option<&str>) -> bool {
    is_fm_header(line) && prev.is_some_and(|p| !p.trim().is_empty())
}

fn frame_calls(src: &str, dest: &str, via: &[String]) -> Vec<String> {
    let mut calls = Vec::new();
    if let Some(call) = normalize_call(src) {
        calls.push(call);
    }
    if let Some(call) = normalize_call(dest) {
        calls.push(call);
    }
    for hop in via {
        if let Some(call) = normalize_call(hop) {
            calls.push(call);
        }
    }
    calls
}

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

/// `fm [HH:MM] SRC to DEST [via A,B] …` → source, destination, digis.
fn heard_from_fm(line: &str) -> Option<(String, String, Vec<String>)> {
    let src = extract_heard_call(line)?;
    let lower = line.to_ascii_lowercase();
    let to_at = lower.find(" to ")?;
    let after = line[to_at + 4..].trim_start();
    let after_lower = after.to_ascii_lowercase();
    let via_at = after_lower.find(" via ");
    let ctl_at = after_lower.find(" ctl ");
    let dest_end = [via_at, ctl_at]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(after.len());
    let dest = after[..dest_end]
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .to_string();
    if dest.is_empty() {
        return None;
    }
    let via = if let Some(at) = via_at {
        let via_txt = &after[at + 5..];
        let via_end = via_txt
            .to_ascii_lowercase()
            .find(" ctl ")
            .unwrap_or(via_txt.len());
        via_txt[..via_end]
            .split(',')
            .map(|hop| hop.trim().trim_end_matches('*').to_string())
            .filter(|hop| !hop.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    Some((src, dest, via))
}

/// Best-effort source call from monitor / APRS-ish text (`fm CALL`, `CALL>`).
fn extract_heard_call(text: &str) -> Option<String> {
    if let Some((_, tail)) = ax25::split_monitor_fm(text) {
        if let Some(call) = call_after_fm(tail) {
            return Some(call);
        }
    }
    let lower = text.to_ascii_lowercase();
    if let Some(pos) = lower.find("fm ") {
        if let Some(call) = call_after_fm(&text[pos + 3..]) {
            return Some(call);
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

fn call_after_fm(after: &str) -> Option<String> {
    let mut tokens = after.split_whitespace();
    let mut token = tokens.next()?;
    if is_hhmm_token(token) {
        token = tokens.next()?;
    }
    let token = token.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
    if token.is_empty() {
        None
    } else {
        Some(token.to_ascii_uppercase())
    }
}

/// How long a COM send or receive keeps its monitor glass bright.
const LINK_LAMP_HOLD: Duration = Duration::from_millis(280);

/// Unread channel label: white, then red, then white. Half of one blink.
const ALERT_BLINK_HALF: f64 = 0.55;

/// How often a quiet monitor looks for packets, lamp flashes, and new chat.
const MONITOR_POLL: Duration = Duration::from_millis(500);

/// Quiet window: look again within half a second, and also on the whole
/// second so the clock hand steps then and not at the half-second poll.
/// `subsec_millis` is the current millisecond within that second (`0..1000`).
fn idle_monitor_repaint(subsec_millis: u32) -> Duration {
    until_next_clock_second(subsec_millis).min(MONITOR_POLL)
}

/// Sleep until just after the next whole second. `subsec_millis` is the
/// current millisecond within that second (`0..1000`).
fn until_next_clock_second(subsec_millis: u32) -> Duration {
    let sub = u64::from(subsec_millis.min(999));
    let wait = 1000u64.saturating_sub(sub).saturating_add(30);
    Duration::from_millis(wait.max(50))
}

/// Non-clickable COM lamp. The PNG screens are transparent; red (front,
/// send) and green (back, receive) are painted behind those holes.
fn paint_link_lamp(
    ui: &mut Ui,
    controls: &mut Vec<Rect>,
    origin_x: f32,
    y: f32,
    w: f32,
    h: f32,
    index: usize,
    tex: Option<&TextureHandle>,
    tx_on: bool,
    rx_on: bool,
) {
    let cell = Rect::from_min_size(Pos2::new(origin_x, y), Vec2::new(w, h));
    controls.push(cell);
    let _resp = ui.interact(
        cell,
        ui.id().with(("sess-link-lamp", index)),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(cell, 0.0, VGA_GRAY);
    paint_bevel(painter, cell, false);
    let inner = cell.shrink(2.0);
    let side = inner.width().min(inner.height());
    let img = Rect::from_center_size(inner.center(), Vec2::splat(side));
    // Source pixels of the transparent glass. Image is 64×64.
    // Front PC is the lower-left screen; the back PC is the upper-right one.
    let hole = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let s = side / 64.0;
        Rect::from_min_max(
            img.min + Vec2::new(x0 * s, y0 * s),
            img.min + Vec2::new(x1 * s, y1 * s),
        )
    };
    let tx_color = if tx_on {
        Color32::from_rgb(255, 36, 36)
    } else {
        Color32::from_rgb(48, 0, 0)
    };
    let rx_color = if rx_on {
        Color32::from_rgb(36, 255, 64)
    } else {
        Color32::from_rgb(0, 42, 0)
    };
    painter.rect_filled(hole(12.0, 30.0, 31.0, 48.0), 0.0, tx_color);
    painter.rect_filled(hole(33.0, 11.0, 52.0, 28.0), 0.0, rx_color);
    if let Some(tex) = tex {
        painter.image(
            tex.id(),
            img,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

struct ModemFace {
    label: String,
    phase: LinkPhase,
    tx_until: Option<Instant>,
    rx_until: Option<Instant>,
}

impl ModemFace {
    fn new(label: String) -> Self {
        Self {
            label,
            phase: LinkPhase::Pending,
            tx_until: None,
            rx_until: None,
        }
    }

    /// Time left on the TX or RX glass, if either is still lit.
    fn lamp_remaining(&self, now: Instant) -> Option<Duration> {
        [self.tx_until, self.rx_until]
            .into_iter()
            .flatten()
            .filter(|t| *t > now)
            .map(|t| t.saturating_duration_since(now))
            .min()
    }
}

fn com_badge(port: &str) -> String {
    let port = port.trim();
    if port.is_empty() {
        "COM".into()
    } else {
        port.to_string()
    }
}

struct HeardSheetRow {
    date: String,
    time: String,
    port: String,
    from: String,
    to: String,
    packets: String,
    payload: String,
    via: String,
    /// Optional second line: `GRID  Region  Country  N Km from QTH`.
    geo_line: Option<String>,
}

impl HeardSheetRow {
    fn from_station(e: &super::mheard::HeardStation, geo_line: Option<String>) -> Self {
        Self {
            date: e.date.clone(),
            time: e.time.clone(),
            port: match e.port {
                Some(port) if port > 0 => port.to_string(),
                _ => String::new(),
            },
            from: e.call.clone(),
            to: e.to.clone(),
            packets: mheard::format_packet_count(e.packets),
            payload: mheard::clip_last_msg(&e.last_msg),
            via: e.via.clone(),
            geo_line,
        }
    }

    fn cell(&self, key: &str) -> &str {
        match key {
            "date" => &self.date,
            "time" => &self.time,
            "port" => &self.port,
            "from" => &self.from,
            "to" => &self.to,
            "packets" => &self.packets,
            "payload" => &self.payload,
            "via" => &self.via,
            _ => "",
        }
    }
}

struct HeardSheetCol {
    key: &'static str,
    title: &'static str,
    width: f32,
}

fn mheard_sheet_cols(total: f32, show_port: bool, cell_w: f32) -> Vec<HeardSheetCol> {
    // Pad each fixed column so the full value fits: date, time, call, packet count.
    // Last Payload and Via take the remaining width and clip only there.
    let pad = 10.0;
    let cols_chars: &[(&str, &str, f32)] = if show_port {
        &[
            ("date", "Date", 10.0),
            ("time", "Time", 5.0),
            ("port", "fm", 2.0),
            ("from", "From", 9.0),
            ("to", "To", 9.0),
            ("packets", "Pckts", 5.0),
        ]
    } else {
        &[
            ("date", "Date", 10.0),
            ("time", "Time", 5.0),
            ("from", "From", 9.0),
            ("to", "To", 9.0),
            ("packets", "Pckts", 5.0),
        ]
    };
    let mut fixed: Vec<(&str, &str, f32)> = cols_chars
        .iter()
        .map(|(key, title, chars)| {
            let title_w = title.len() as f32 * cell_w + pad;
            let body_w = *chars * cell_w + pad;
            (*key, *title, title_w.max(body_w))
        })
        .collect();
    let used: f32 = fixed.iter().map(|c| c.2).sum();
    let rest = (total - used).max(cell_w * 16.0);
    let payload_w = (rest * 0.68).max(cell_w * 10.0);
    let via_w = (rest - payload_w).max(cell_w * 6.0);
    fixed.push(("payload", "Last Payload", payload_w));
    fixed.push(("via", "Via", via_w));
    fixed
        .into_iter()
        .map(|(key, title, width)| HeardSheetCol { key, title, width })
        .collect()
}

fn paint_mheard_sheet_header(
    painter: &egui::Painter,
    rect: Rect,
    cols: &[HeardSheetCol],
    font: &FontId,
    fg: Color32,
    bg: Color32,
    rule: Stroke,
) {
    painter.rect_filled(rect, 0.0, mix_rgb(bg, fg, 0.12));
    let mut x = rect.left();
    for (i, col) in cols.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(x, rect.top()),
            Vec2::new(col.width, rect.height()),
        );
        painter.text(
            Pos2::new(cell.left() + 4.0, cell.center().y),
            egui::Align2::LEFT_CENTER,
            col.title,
            font.clone(),
            fg,
        );
        if i + 1 < cols.len() {
            let edge = cell.right();
            painter.line_segment(
                [
                    Pos2::new(edge, rect.top() + 1.0),
                    Pos2::new(edge, rect.bottom() - 1.0),
                ],
                rule,
            );
        }
        x += col.width;
    }
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], rule);
}

fn paint_mheard_sheet_row(
    painter: &egui::Painter,
    rect: Rect,
    cols: &[HeardSheetCol],
    row: &HeardSheetRow,
    font: &FontId,
    fg: Color32,
    rule: Stroke,
) {
    let mut x = rect.left();
    for (i, col) in cols.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(x, rect.top()),
            Vec2::new(col.width, rect.height()),
        );
        let raw = row.cell(col.key);
        let clip = matches!(col.key, "payload" | "via");
        let text = if clip {
            clip_sheet_cell(raw, col.width, font, painter)
        } else {
            raw.to_string()
        };
        let align = if col.key == "packets" || col.key == "port" {
            egui::Align2::RIGHT_CENTER
        } else {
            egui::Align2::LEFT_CENTER
        };
        let pos = if col.key == "packets" || col.key == "port" {
            Pos2::new(cell.right() - 4.0, cell.center().y)
        } else {
            Pos2::new(cell.left() + 4.0, cell.center().y)
        };
        painter.text(pos, align, text, font.clone(), fg);
        if i + 1 < cols.len() {
            let edge = cell.right();
            painter.line_segment(
                [Pos2::new(edge, rect.top()), Pos2::new(edge, rect.bottom())],
                rule,
            );
        }
        x += col.width;
    }
}

fn clip_sheet_cell(text: &str, width: f32, font: &FontId, painter: &egui::Painter) -> String {
    let inner = (width - 8.0).max(8.0);
    if painter
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE)
        .size()
        .x
        <= inner
    {
        return text.to_string();
    }
    let ellipsis = "...";
    let mut out = String::new();
    for ch in text.chars() {
        let next = format!("{out}{ch}{ellipsis}");
        if painter
            .layout_no_wrap(next, font.clone(), Color32::WHITE)
            .size()
            .x
            > inner
        {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        ellipsis.to_string()
    } else {
        format!("{out}{ellipsis}")
    }
}

/// Keep the startup locator to eight Maidenhead characters.
fn clip_locator_edit(text: &mut String) {
    let next: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if *text != next {
        *text = next;
    }
}

/// Shown while a finished-looking grid is not a locator. Shorter text is still being typed.
fn locator_field_warning(text: &str) -> Option<&'static str> {
    let raw = text.trim();
    let n = raw.len();
    if raw.is_empty() || crate::locator::normalize_locator(raw).is_some() {
        return None;
    }
    if n == 6 || n >= 8 {
        Some("Locator is 6 or 8 characters (e.g. JO32KX)")
    } else {
        None
    }
}

/// Apply refuses to open the link until the locator is empty or valid.
fn locator_blocks_start(text: &str) -> Option<String> {
    let raw = text.trim();
    if raw.is_empty() || crate::locator::normalize_locator(raw).is_some() {
        None
    } else {
        Some("Locator is 6 or 8 characters (e.g. JO32KX)".into())
    }
}

fn modem_faces(cfg: &Config) -> Vec<ModemFace> {
    let faces = match cfg.tnc.protocol_kind() {
        HostProtocol::Tfpcx => cfg
            .tnc
            .links
            .iter()
            .filter(|link| link.is_enabled())
            .map(|link| {
                let label = if link.uses_tcp() {
                    "TCP".into()
                } else {
                    com_badge(&link.serial_port)
                };
                ModemFace::new(label)
            })
            .collect::<Vec<_>>(),
        HostProtocol::Kiss if cfg.tnc.kiss_use_tcp() => vec![ModemFace::new("TCP".into())],
        _ => vec![ModemFace::new(com_badge(&cfg.serial.port))],
    };
    if faces.is_empty() {
        vec![ModemFace::new(com_badge(&cfg.serial.port))]
    } else {
        faces
    }
}

fn modem_badge_w(label: &str) -> f32 {
    (label.len() as f32 * 8.0 + 16.0).clamp(52.0, 88.0)
}

fn paint_modem_badge(
    ui: &mut Ui,
    origin_x: f32,
    y: f32,
    h: f32,
    label: &str,
    phase: LinkPhase,
    w: f32,
) {
    let cell = Rect::from_min_size(Pos2::new(origin_x, y), Vec2::new(w, h));
    let _resp = ui.interact(
        cell,
        ui.id().with(("sess-modem", label, origin_x.to_bits())),
        Sense::hover(),
    );
    let painter = ui.painter();
    let fill = match phase {
        LinkPhase::Open => VGA_GREEN,
        LinkPhase::Closed => Color32::from_rgb(200, 32, 32),
        LinkPhase::Pending | LinkPhase::Retry => VGA_GRAY,
    };
    painter.rect_filled(cell, 0.0, fill);
    paint_bevel(painter, cell, phase == LinkPhase::Open);
    painter.text(
        cell.center() - Vec2::new(0.0, 1.0),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        Color32::BLACK,
    );
}

/// Room kept under the welcome picture for the KISS, WA8DED, and Baycom card.
const START_CARD_RESERVE: f32 = 312.0;
/// Height of the TFPCX card with its port grid, until a taller card is measured.
const START_TFPCX_RESERVE: f32 = 440.0;
/// Room kept under the radio card for the two-line welcome credits.
const START_CREDITS_H: f32 = 56.0;
/// Clear air between the TFPCX card and the orange credits.
const START_CARD_CREDITS_GAP: f32 = 12.0;

fn paint_start_credits(painter: &egui::Painter, screen: Rect) {
    let color = Color32::from_rgb(255, 168, 0);
    let font = FontId::proportional(15.0);
    let text = version::startup_credits();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return;
    }
    let galleys: Vec<_> = lines
        .iter()
        .map(|line| painter.layout_no_wrap((*line).to_string(), font.clone(), color))
        .collect();
    let gap = 2.0;
    let total_h = galleys.iter().map(|g| g.size().y).sum::<f32>()
        + gap * (galleys.len().saturating_sub(1) as f32);
    let mut y = screen.bottom() - 12.0 - total_h;
    for galley in galleys {
        let x = screen.center().x - galley.size().x * 0.5;
        let h = galley.size().y;
        painter.galley(Pos2::new(x, y), galley, color);
        y += h + gap;
    }
}

/// Empty saved detail means we have not recorded a device yet, so the port name is enough.
fn port_identity_matches(saved: &str, live: &str) -> bool {
    saved.is_empty() || live.is_empty() || saved.eq_ignore_ascii_case(live)
}

/// A closed list. One click opens the second panel. No arrow on the row.
fn menu_open(ui: &mut Ui, label: &str, value: &str, width: f32) -> bool {
    let resp = retro::entry(
        ui,
        label,
        value,
        retro::TAB_OPEN,
        false,
        width,
        menu_font(17.0),
    );
    resp.clicked() && !resp.double_clicked()
}

/// Equal cells for ports 0–11. Two-digit labels stay in the same columns as 0–5.
fn paint_port_grid(
    ui: &mut Ui,
    chosen: &[u8],
    row_w: f32,
    font: &FontId,
    mut toggle: impl FnMut(u8),
) {
    const COLS: usize = 6;
    let gap = 3.0;
    let cell_w = ((row_w - gap * (COLS as f32 - 1.0)) / COLS as f32).max(22.0);
    let cell_h = 22.0;
    let last = usize::from(crate::config::TncConfig::TFPCX_MAX_PORT);
    let rows = last / COLS + 1;
    for row in 0..rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for col in 0..COLS {
                let port_n = row * COLS + col;
                if port_n > last {
                    break;
                }
                let port = port_n as u8;
                let on = chosen.contains(&port);
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(cell_w, cell_h), Sense::click());
                let fill = if on {
                    Color32::from_rgb(180, 210, 180)
                } else if response.hovered() {
                    Color32::from_rgb(214, 214, 214)
                } else {
                    Color32::from_rgb(192, 192, 192)
                };
                retro::paint(
                    ui.painter(),
                    rect,
                    fill,
                    response.is_pointer_button_down_on(),
                    retro::ROW_ROUND,
                );
                ui.painter().text(
                    rect.center() + Vec2::new(0.0, -1.0),
                    egui::Align2::CENTER_CENTER,
                    format!("{port}"),
                    font.clone(),
                    Color32::BLACK,
                );
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    toggle(port);
                }
            }
        });
    }
}

fn link_kind_label(protocol: &str) -> &'static str {
    match protocol {
        "kiss" => "KISS",
        "wa8ded" => "WA8DED",
        _ => "Off",
    }
}

fn radio_link_forms(
    tnc: &crate::config::TncConfig,
    port: &str,
    baud: u32,
    host: &str,
    tcp_port: &str,
) -> Vec<RadioLinkForm> {
    let mut forms: Vec<RadioLinkForm> = tnc
        .links
        .iter()
        .take(3)
        .map(|link| RadioLinkForm {
            protocol: if link.is_enabled() {
                link.protocol.to_ascii_lowercase()
            } else {
                "off".into()
            },
            ports: link.ports.clone(),
            tcp: link.uses_tcp(),
            port_name: if link.serial_port.is_empty() {
                port.to_string()
            } else {
                link.serial_port.clone()
            },
            baud: if link.baud == 0 { baud } else { link.baud },
            host: if link.tcp_host.is_empty() {
                host.to_string()
            } else {
                link.tcp_host.clone()
            },
            tcp_port: if link.tcp_port == 0 {
                tcp_port.to_string()
            } else {
                link.tcp_port.to_string()
            },
        })
        .collect();
    if forms.is_empty() {
        forms.push(RadioLinkForm {
            protocol: "kiss".into(),
            ports: vec![0, 1],
            tcp: false,
            port_name: port.to_string(),
            baud,
            host: host.to_string(),
            tcp_port: tcp_port.to_string(),
        });
    }
    while forms.len() < 3 {
        forms.push(RadioLinkForm {
            protocol: "off".into(),
            ports: Vec::new(),
            tcp: false,
            port_name: port.to_string(),
            baud,
            host: host.to_string(),
            tcp_port: tcp_port.to_string(),
        });
    }
    forms
}

fn port_menu_label(ports: &[DetectedPort], selected: &str) -> String {
    if selected.is_empty() {
        return "choose a port".into();
    }
    ports
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(selected))
        .map(DetectedPort::menu_label)
        .unwrap_or_else(|| format!("{selected} — not connected"))
}

const RADIO_PROTOCOLS: [HostProtocol; 4] = [
    HostProtocol::Kiss,
    HostProtocol::Wa8ded,
    HostProtocol::Baycom,
    HostProtocol::Tfpcx,
];

fn protocol_label(kind: HostProtocol) -> &'static str {
    match kind {
        HostProtocol::Wa8ded => "TAPR TNC-2 WA8DED",
        HostProtocol::Tfpcx => "TFPCX - Multi Modem",
        HostProtocol::Kiss => "KISS",
        HostProtocol::Baycom => "Baycom",
    }
}

fn baud_choices(current: u32, protocol: HostProtocol) -> Vec<u32> {
    if protocol == HostProtocol::Baycom {
        return vec![1200];
    }
    let mut rates = vec![1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200];
    if current > 0 && !rates.contains(&current) {
        rates.push(current);
        rates.sort_unstable();
    }
    rates
}

fn com_number_from_port(port: &str) -> Option<u32> {
    let rest = port
        .strip_prefix("COM")
        .or_else(|| port.strip_prefix("com"))?;
    let n: u32 = rest.parse().ok()?;
    (n > 0).then_some(n)
}

/// Startup picture. `None` if the PNG cannot be decoded.
fn load_start_screen(ctx: &Context) -> Option<TextureHandle> {
    const BYTES: &[u8] = include_bytes!("../../assets/start.png");
    let img = match image::load_from_memory_with_format(BYTES, image::ImageFormat::Png) {
        Ok(img) => img,
        Err(err) => {
            tracing::warn!("start screen: {err}");
            return None;
        }
    };
    let rgba = img.into_rgba8();
    let color = ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    // Nearest keeps the pixel lettering sharp when the window scales it.
    Some(ctx.load_texture("gp_start", color, TextureOptions::NEAREST))
}

/// COM lamp artwork. `None` if the PNG cannot be decoded.
fn load_link_lamps(ctx: &Context) -> Option<TextureHandle> {
    const BYTES: &[u8] = include_bytes!("../../assets/network.png");
    let img = match image::load_from_memory_with_format(BYTES, image::ImageFormat::Png) {
        Ok(img) => img,
        Err(err) => {
            tracing::warn!("link lamp: {err}");
            return None;
        }
    };
    let rgba = img.into_rgba8();
    let color = ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    Some(ctx.load_texture("gp_link_lamps", color, TextureOptions::NEAREST))
}

fn paint_icon_grid(ui: &mut Ui, edit: &mut IconEdit) {
    let (w, h, mut pixels) = {
        let Some(icon) = edit.icons.get(edit.index) else {
            return;
        };
        (icon.width, icon.height, icon.pixels.clone())
    };
    if w == 0 || h == 0 {
        return;
    }
    let cell = (560.0 / w as f32).clamp(4.0, 8.0);
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(w as f32 * cell, h as f32 * cell),
        Sense::click_and_drag(),
    );
    let color = if ui.input(|i| i.pointer.secondary_down()) {
        edit.right_color
    } else {
        edit.color
    } & 15;
    if resp.is_pointer_button_down_on() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if let Some((x, y)) = icon_cell(rect, pos, w, h) {
                let i = y * w + x;
                if let Some(px) = pixels.get_mut(i) {
                    *px = color;
                }
                if let Some(icon) = edit.current_mut() {
                    if let Some(px) = icon.pixels.get_mut(i) {
                        *px = color;
                    }
                }
            }
        }
    }
    blit_pixels(ui.painter(), rect, &pixels, w, h);
    ui.painter()
        .rect_stroke(rect.expand(1.0), 0.0, Stroke::new(2.0, Color32::BLACK));
    if let Some(pos) = resp.hover_pos() {
        if let Some((x, y)) = icon_cell(rect, pos, w, h) {
            let cw = rect.width() / w as f32;
            let ch = rect.height() / h as f32;
            let cell_rect = Rect::from_min_size(
                rect.min + Vec2::new(x as f32 * cw, y as f32 * ch),
                Vec2::new(cw, ch),
            );
            ui.painter()
                .rect_stroke(cell_rect, 0.0, Stroke::new(1.0, Color32::WHITE));
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
}

fn icon_editor_side(
    ui: &mut Ui,
    edit: &mut IconEdit,
    close: &mut bool,
    reload: &mut Option<PathBuf>,
    note: &mut Option<String>,
) {
    ui.vertical(|ui| {
        ui.set_width(188.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            RichText::new(edit.file_name())
                .font(menu_font(16.0))
                .color(Color32::BLACK),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(140.0, 80.0), Sense::hover());
            if let Some(icon) = edit.icons.get(edit.index) {
                blit_pixels(ui.painter(), rect, &icon.pixels, icon.width, icon.height);
            }
            ui.painter()
                .rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::BLACK));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if retro::button(ui, "Up", 52.0, 28.0, menu_font(15.0)) {
                    edit.step(1);
                }
                if retro::button(ui, "Down", 52.0, 28.0, menu_font(15.0)) {
                    edit.step(-1);
                }
            });
        });
        let n = edit.icons.len().max(1);
        ui.label(
            RichText::new(format!("Icon:  {}/{n}", edit.index + 1))
                .font(menu_font(16.0))
                .color(Color32::BLACK),
        );
        if let Some(slot) = ICON_SLOT_LABELS.get(edit.index) {
            ui.label(
                RichText::new(*slot)
                    .font(menu_font(16.0))
                    .color(Color32::BLACK),
            );
        }
        for (label, color) in [
            ("Left-click color:", &mut edit.color),
            ("Right-click color:", &mut edit.right_color),
        ] {
            ui.label(
                RichText::new(label)
                    .font(menu_font(16.0))
                    .color(Color32::BLACK),
            );
            let pen = (*color as usize) & 15;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (swatch, _) = ui.allocate_exact_size(Vec2::new(72.0, 40.0), Sense::hover());
                let (r, g, b) = VGA16_RGB[pen];
                ui.painter()
                    .rect_filled(swatch, 0.0, Color32::from_rgb(r, g, b));
                ui.painter()
                    .rect_stroke(swatch, 0.0, Stroke::new(1.0, Color32::BLACK));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if retro::button(ui, "Up", 52.0, 28.0, menu_font(15.0)) {
                        *color = (pen as u8 + 1) % 16;
                    }
                    if retro::button(ui, "Down", 52.0, 28.0, menu_font(15.0)) {
                        *color = (pen as u8 + 15) % 16;
                    }
                });
            });
            let name = setup::VGA_NAMES.get(pen).copied().unwrap_or("");
            ui.label(
                RichText::new(name)
                    .font(menu_font(16.0))
                    .color(Color32::BLACK),
            );
        }
        ui.label(RichText::new("Palette: left / right click sets that pen").font(menu_font(14.0)));
        let pen = (edit.color as usize) & 15;
        let (pal, pal_resp) =
            ui.allocate_exact_size(Vec2::new(8.0 * 22.0, 2.0 * 22.0), Sense::click());
        for i in 0..16 {
            let col = i % 8;
            let row = i / 8;
            let cell = Rect::from_min_size(
                pal.min + Vec2::new(col as f32 * 22.0, row as f32 * 22.0),
                Vec2::splat(20.0),
            );
            let (r, g, b) = VGA16_RGB[i];
            ui.painter()
                .rect_filled(cell, 0.0, Color32::from_rgb(r, g, b));
            let stroke = if i == pen {
                Stroke::new(2.0, Color32::WHITE)
            } else {
                Stroke::new(1.0, Color32::BLACK)
            };
            ui.painter().rect_stroke(cell, 0.0, stroke);
        }
        if pal_resp.clicked() || pal_resp.secondary_clicked() {
            if let Some(pos) = pal_resp.interact_pointer_pos() {
                let col = ((pos.x - pal.left()) / 22.0) as usize;
                let row = ((pos.y - pal.top()) / 22.0) as usize;
                if col < 8 && row < 2 {
                    let color = (row * 8 + col) as u8;
                    if pal_resp.secondary_clicked() {
                        edit.right_color = color;
                    } else {
                        edit.color = color;
                    }
                }
            }
        }
        ui.add_space(8.0);
        if retro::button(ui, "Save", 168.0, 32.0, menu_font(18.0)) {
            match edit.save() {
                Ok(()) => {
                    *reload = Some(edit.path.clone());
                    *note = Some(format!("Saved {}", edit.file_name()));
                }
                Err(err) => *note = Some(format!("Save: {err}")),
            }
        }
        if retro::button(ui, "Apply", 168.0, 32.0, menu_font(18.0)) {
            *close = true;
        }
    });
}

fn icon_cell(rect: Rect, pos: Pos2, w: usize, h: usize) -> Option<(usize, usize)> {
    if w == 0 || h == 0 || !rect.contains(pos) {
        return None;
    }
    let x = ((pos.x - rect.left()) / rect.width() * w as f32) as usize;
    let y = ((pos.y - rect.top()) / rect.height() * h as f32) as usize;
    (x < w && y < h).then_some((x, y))
}

fn blit_pixels(painter: &egui::Painter, rect: Rect, pixels: &[u8], w: usize, h: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let cw = rect.width() / w as f32;
    let ch = rect.height() / h as f32;
    for y in 0..h {
        for x in 0..w {
            let Some(idx) = pixels.get(y * w + x) else {
                continue;
            };
            let (r, g, b) = VGA16_RGB[(*idx as usize) & 15];
            let cell = Rect::from_min_size(
                rect.min + Vec2::new(x as f32 * cw, y as f32 * ch),
                Vec2::new(cw + 0.4, ch + 0.4),
            );
            painter.rect_filled(cell, 0.0, Color32::from_rgb(r, g, b));
        }
    }
}

fn load_icon_textures(ctx: &Context, icon_file: &str) -> Vec<TextureHandle> {
    let path = resolve_icon_path(icon_file);
    let loaded = textures_from_icon_file(ctx, &path);
    if !loaded.is_empty() {
        return loaded;
    }
    let fallback = crate::paths::data_file("ICONS.GP");
    if fallback != path {
        return textures_from_icon_file(ctx, &fallback);
    }
    Vec::new()
}

fn textures_from_icon_file(ctx: &Context, path: &std::path::Path) -> Vec<TextureHandle> {
    if !path.is_file() {
        return Vec::new();
    }
    tracing::info!(file = %path.display(), "loading icon library");
    let Ok(lib) = IconLibrary::load(path) else {
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

/// Grey-menu face. Named family so the terminal panes can keep Consolas.
const MENU_FACE: &str = "gp_menu";

fn menu_family() -> FontFamily {
    FontFamily::Name(MENU_FACE.into())
}

/// Dialog labels, titles, and fields. Not the RX pads.
fn menu_font(size: f32) -> FontId {
    FontId::new(size, menu_family())
}

/// While a grey dialog is up, widget text uses the menu face.
/// Closed, the defaults come back so hover tips stay as they were.
fn apply_popup_text_style(ctx: &Context, menu: bool) {
    ctx.style_mut(|style| {
        let face = if menu {
            menu_family()
        } else {
            FontFamily::Proportional
        };
        // Dialog type is a step larger so it stays easy to read.
        let (small, body, button, heading) = if menu {
            (14.0, 16.0, 18.0, 20.0)
        } else {
            (9.0, 12.5, 12.5, 18.0)
        };
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::new(small, face.clone()));
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::new(body, face.clone()));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::new(button, face.clone()));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::new(heading, face));
        let d = egui::Style::default();
        if menu {
            style.spacing.item_spacing = Vec2::new(8.0, 6.0);
            style.spacing.button_padding = Vec2::new(12.0, 6.0);
            // Short rows: a stock widget must not grow taller than the line.
            style.spacing.interact_size.y = 22.0;
        } else {
            style.spacing.item_spacing = d.spacing.item_spacing;
            style.spacing.button_padding = d.spacing.button_padding;
            style.spacing.interact_size = d.spacing.interact_size;
        }
    });
}

fn read_first_font(candidates: &[&str]) -> Option<(String, Vec<u8>)> {
    for path in candidates {
        let path = std::path::Path::new(path);
        if !path.is_file() {
            continue;
        }
        match std::fs::read(path) {
            Ok(bytes) => return Some((path.display().to_string(), bytes)),
            Err(err) => tracing::warn!("failed to read font {}: {err}", path.display()),
        }
    }
    None
}

/// JetBrains Mono, no-ligature. SIL OFL 1.1 — see `assets/fonts/OFL.txt`.
/// `include_bytes` compiles the TTF into the exe, so pads look the same on every OS.
/// Zoom off uses Regular. Zoom on uses Bold. Only the monospace family.
const JETBRAINS_MONO: &[u8] = include_bytes!("../../assets/fonts/ttf/JetBrainsMonoNL-Regular.ttf");
const JETBRAINS_MONO_BOLD: &[u8] =
    include_bytes!("../../assets/fonts/ttf/JetBrainsMonoNL-Bold.ttf");
const VARIATION_SELECTORS: &[u8] =
    include_bytes!("../../assets/fonts/ttf/GPNG-VariationSelectors.ttf");

/// DOS `Zoom`. `1` (and anything below `2`) is the regular face.
fn zoom_bold(zoom: u32) -> bool {
    zoom >= 2
}

/// JetBrains Mono for the RX pads and the input line. Segoe UI for grey dialogs.
///
/// The built-in proportional face is Ubuntu Light. On a grey panel its stems
/// are about one pixel, so black text looks jagged. Segoe UI is a regular
/// weight and keeps a solid edge at dialog sizes.
fn install_console_font(ctx: &Context, bold: bool) {
    let mut fonts = FontDefinitions::default();
    let face: &[u8] = if bold {
        JETBRAINS_MONO_BOLD
    } else {
        JETBRAINS_MONO
    };
    fonts
        .font_data
        .insert("gp_console".into(), FontData::from_static(face));
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "gp_console".into());
    fonts.font_data.insert(
        "gp_variation_selectors".into(),
        FontData::from_static(VARIATION_SELECTORS),
    );
    for family in [FontFamily::Monospace, FontFamily::Proportional] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("gp_variation_selectors".into());
    }
    tracing::info!(
        "console font: JetBrains Mono {} (embedded)",
        if bold { "Bold" } else { "Regular" }
    );

    let proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some((path, bytes)) = read_first_font(&[
        r"C:\Windows\Fonts\segoeui.ttf", // Segoe UI
        r"C:\Windows\Fonts\calibri.ttf",
        r"C:\Windows\Fonts\arial.ttf",
    ]) {
        fonts
            .font_data
            .insert(MENU_FACE.into(), FontData::from_owned(bytes));
        let mut chain = vec![MENU_FACE.into()];
        chain.extend(proportional);
        fonts
            .families
            .insert(FontFamily::Name(MENU_FACE.into()), chain);
        tracing::info!("menu font: {path}");
    } else {
        tracing::warn!("no system UI TTF found; grey menus keep the default face");
        fonts
            .families
            .insert(FontFamily::Name(MENU_FACE.into()), proportional);
    }
    ctx.set_fonts(fonts);
}

/// Draw the tall clock column. Returns `true` when the ♫ gong button was clicked.
fn draw_analog_clock(ui: &mut Ui, rect: Rect, gong_on: bool, column: Color32) -> bool {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, column);

    let now = Local::now();
    // Dial fills most of the slim column; badges sit centered under it.
    let badge_h = 18.0;
    let badge_gap = 4.0;
    let side_pad = 6.0;
    // Dial from the column width. Shrink only when the two badges would not fit,
    // so the version badge stays on the bottom edge of the header.
    let radius_w = ((rect.width() - side_pad * 2.0) * 0.5 - 1.0).clamp(36.0, 70.0);
    let under_dial = 8.0 + badge_h * 2.0 + badge_gap;
    let room = (rect.height() - 12.0 - under_dial).max(72.0);
    let radius = radius_w.min(room * 0.5);
    let cx = rect.center().x;
    let dial_center = Pos2::new(cx, rect.top() + 12.0 + radius);

    // Dark grey face, no outline ring.
    painter.circle_filled(dial_center, radius + 1.0, VGA_DARK_GRAY);

    let tick_yellow = Color32::from_rgb(255, 255, 85);
    let hour_len = 8.0;
    let minute_len = hour_len * 0.5;
    // 60 minute ticks; every 5th is a longer hour mark.
    for i in 0..60 {
        let a = (i as f32) / 60.0 * TAU - TAU / 4.0;
        let hour = i % 5 == 0;
        let len = if hour { hour_len } else { minute_len };
        let width = if hour { 2.0 } else { 1.0 };
        let outer = dial_center + Vec2::angled(a) * (radius - 2.0);
        let inner = dial_center + Vec2::angled(a) * (radius - 2.0 - len);
        painter.line_segment([inner, outer], Stroke::new(width, tick_yellow));
    }

    let h = now.hour() as f32 % 12.0 + now.minute() as f32 / 60.0;
    let m = now.minute() as f32 + now.second() as f32 / 60.0;
    // Whole seconds. A sweeping hand would repaint the window all day.
    let s = now.second() as f32;

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
    let date_pos = Pos2::new(cx, dial_center.y + radius + 8.0 + badge_h * 0.5);
    let date_rect = Rect::from_center_size(date_pos, Vec2::new(86.0, badge_h));
    // Date / version badges: dark grey raised buttons (unpressed bevel).
    retro::paint_edge(
        &painter,
        date_rect,
        VGA_DARK_GRAY,
        false,
        retro::TOP_ROUND,
        1.5,
    );
    painter.text(
        date_pos,
        egui::Align2::CENTER_CENTER,
        date,
        FontId::monospace(13.0),
        Color32::WHITE,
    );

    let ver_pos = Pos2::new(cx, date_pos.y + badge_h * 0.5 + badge_gap + badge_h * 0.5);
    let ver_rect = Rect::from_center_size(ver_pos, Vec2::new(92.0, badge_h));
    retro::paint_edge(
        &painter,
        ver_rect,
        VGA_DARK_GRAY,
        false,
        retro::TOP_ROUND,
        1.5,
    );
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
    retro::paint_edge(
        painter,
        btn,
        fill,
        gong_on || resp.is_pointer_button_down_on(),
        retro::TOP_ROUND,
        1.5,
    );
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

/// How many monospace cells fit in the scroll viewport.
fn text_columns(ui: &Ui, font_px: f32) -> usize {
    let char_w = ui
        .fonts(|fonts| fonts.glyph_width(&FontId::monospace(font_px), '0'))
        .max(1.0);
    // The DOS bar sits outside this viewport. A few pixels of slack keep
    // rounding from opening a horizontal bar.
    let width = (ui.available_width() - 4.0).max(char_w);
    ((width / char_w).floor() as usize).max(8)
}

/// Six pixels a second: one pixel about every sixth of a second.
const SOFT_SCROLL_PX_PER_SEC: f32 = 6.0;

/// Per-pane memory for the stair / `/` follow. Keyed by the scroll area.
#[derive(Clone, Copy, Debug, Default)]
struct SoftCrawl {
    owner: u64,
    seen: bool,
    /// Still walking toward the newest line. A scroll upward clears it.
    follow: bool,
    /// Last frame's ScrLock-off (`stick`) so unlocking can re-engage follow.
    stick: bool,
    last_offset: f32,
    last_max: f32,
    /// Fraction of a pixel still owed to the crawl clock.
    carry: f32,
    last_t: f64,
}

/// Stable id so Monitor, a channel, and a spy do not share one crawl.
fn pane_scroll_owner(pane: ActivePane) -> u64 {
    match pane {
        ActivePane::Monitor => 1,
        ActivePane::Editor => 2,
        ActivePane::Check => 3,
        ActivePane::List => 4,
        ActivePane::Mh => 5,
        ActivePane::Map => 6,
        ActivePane::Channel(n) => 100 + n as u64,
        ActivePane::Spy(n) => 200 + n as u64,
    }
}

fn softscroll_status(smooth: bool) -> String {
    format!("Softscroll {}", on_off(smooth))
}

/// `true` while new lines should move this view (ScrLock off).
///
/// ScrLock on (`stick == false`) freezes. ScrLock off follows the end: a
/// scroll-up may pause briefly, but any growth of the pad (new text) takes
/// hold again. Landing on the last line also resumes. Clear (shorter pad)
/// is not treated as a scroll-up.
fn keep_following(
    stick: bool,
    offset: f32,
    last_offset: f32,
    max_off: f32,
    last_max: f32,
    was: bool,
) -> bool {
    if !stick {
        return false;
    }
    // New text arrived — always jump/crawl to the end while ScrLock is off.
    if max_off > last_max + 0.5 {
        return true;
    }
    let shrunk = max_off + 0.5 < last_max;
    if !shrunk && offset < last_offset - 0.75 {
        return false;
    }
    if max_off - offset <= 1.0 {
        return true;
    }
    was
}

/// Hard (`/`) jumps to `max_off`. The stair adds one pixel about every 1/6s.
/// `carry` is the unused fraction. A long pause moves at most half a second.
fn soft_scroll_toward(offset: f32, max_off: f32, carry: f32, dt: f32, smooth: bool) -> (f32, f32) {
    let max_off = max_off.max(0.0);
    let offset = offset.clamp(0.0, max_off);
    if !smooth || max_off - offset <= 0.5 {
        return (max_off, 0.0);
    }
    let mut carry = carry + dt.clamp(0.0, 0.5) * SOFT_SCROLL_PX_PER_SEC;
    let step = carry.floor();
    carry -= step;
    let offset = (offset + step).min(max_off);
    if max_off - offset <= 0.5 {
        (max_off, 0.0)
    } else {
        (offset, carry)
    }
}

/// Scroll column from CHANNEL1.png: arrow buttons, cyan shaft, gray thumb.
struct DosBarParts {
    up: Rect,
    down: Rect,
    /// Cyan interior. The thumb travels inside this rect.
    track: Rect,
    thumb: Rect,
}

#[derive(Clone, Copy, Default)]
struct DosHeld {
    up: bool,
    down: bool,
    thumb: bool,
}

/// Press that started on an arrow, the shaft, or the thumb.
#[derive(Clone, Copy, Default)]
struct DosPress {
    zone: u8,
    grab: f32,
    next: f64,
    repeats: u8,
}

fn dos_bar_parts(bar: Rect, offset: f32, max_off: f32, viewport: f32) -> DosBarParts {
    let arrow = bar
        .width()
        .min(bar.height() * 0.45)
        .min(bar.height() * 0.5)
        .max(0.0);
    let up = Rect::from_min_size(bar.min, Vec2::new(bar.width(), arrow));
    let down = Rect::from_min_size(
        Pos2::new(bar.left(), (bar.bottom() - arrow).max(bar.top())),
        Vec2::new(bar.width(), arrow),
    );
    // Three pixels of chrome each side: white, gray, black | cyan | white, gray, black.
    let track = Rect::from_min_max(
        Pos2::new(bar.left() + 3.0, up.bottom()),
        Pos2::new(
            (bar.right() - 3.0).max(bar.left() + 3.0),
            down.top().max(up.bottom()),
        ),
    );
    let thumb = dos_thumb(track, offset, max_off, viewport);
    DosBarParts {
        up,
        down,
        track,
        thumb,
    }
}

/// Short thumb at the top when the pad fits (MONITOR.png). Otherwise the
/// thumb is the viewport's share of the shaft, never shorter than that stub.
fn dos_thumb(track: Rect, offset: f32, max_off: f32, viewport: f32) -> Rect {
    if track.height() < 2.0 || track.width() < 2.0 {
        return Rect::from_min_size(track.min, Vec2::ZERO);
    }
    let min_h = 20.0_f32.min(track.height());
    let thumb_h = if max_off <= 0.5 {
        min_h
    } else {
        let content = (viewport + max_off).max(1.0);
        let ratio = (viewport / content).clamp(0.0, 1.0);
        (track.height() * ratio).clamp(min_h, track.height())
    };
    let travel = (track.height() - thumb_h).max(0.0);
    let t = if max_off <= 0.5 {
        0.0
    } else {
        (offset / max_off).clamp(0.0, 1.0)
    };
    Rect::from_min_size(
        Pos2::new(track.left(), track.top() + travel * t),
        Vec2::new(track.width(), thumb_h),
    )
}

fn dos_hit(parts: &DosBarParts, bar: Rect, pos: Pos2) -> u8 {
    if parts.up.contains(pos) {
        1
    } else if parts.down.contains(pos) {
        2
    } else if parts.thumb.contains(pos) {
        5
    } else if pos.x >= bar.left()
        && pos.x < bar.right()
        && pos.y >= parts.up.bottom()
        && pos.y < parts.thumb.top()
    {
        3
    } else if pos.x >= bar.left()
        && pos.x < bar.right()
        && pos.y >= parts.thumb.bottom()
        && pos.y < parts.down.top()
    {
        4
    } else {
        0
    }
}

/// Arrows step one line. The cyan shaft steps one page. The thumb drags.
fn dos_bar_input(
    ui: &Ui,
    scroll_id: egui::Id,
    parts: &DosBarParts,
    bar: Rect,
    offset: f32,
    max_off: f32,
    page: f32,
    line: f32,
) -> (f32, bool, DosHeld) {
    let sense = Sense {
        click: true,
        drag: true,
        focusable: false,
    };
    let response = ui.interact(bar, scroll_id.with("dos_bar"), sense);
    response.on_hover_cursor(egui::CursorIcon::Default);

    let pointer = ui.input(|i| i.pointer.interact_pos());
    let down = ui.is_enabled() && ui.input(|i| i.pointer.primary_down());
    let pressed = ui.is_enabled() && ui.input(|i| i.pointer.primary_pressed());
    let now = ui.input(|i| i.time);
    let key = scroll_id.with("dos_press");
    let ctx = ui.ctx();
    let mut press = ctx
        .data(|d| d.get_temp::<DosPress>(key))
        .filter(|p| p.zone != 0);

    if !down {
        press = None;
    } else if pressed {
        if let Some(pos) = pointer {
            let zone = dos_hit(parts, bar, pos);
            if zone != 0 {
                press = Some(DosPress {
                    zone,
                    grab: pos.y - parts.thumb.top(),
                    next: now,
                    repeats: 0,
                });
            }
        }
    }

    let mut offset = offset.clamp(0.0, max_off.max(0.0));
    let mut moved = false;
    let mut held = DosHeld::default();
    if let (Some(pos), Some(mut press)) = (pointer, press) {
        let on_thumb = press.zone == 5;
        let still = match press.zone {
            1 => parts.up.contains(pos),
            2 => parts.down.contains(pos),
            3 => pos.y < parts.thumb.top() && pos.y >= parts.up.bottom(),
            4 => pos.y >= parts.thumb.bottom() && pos.y < parts.down.top(),
            5 => true,
            _ => false,
        };
        held.up = press.zone == 1 && still;
        held.down = press.zone == 2 && still;
        held.thumb = on_thumb;
        if max_off > 0.5 && on_thumb {
            let travel = (parts.track.height() - parts.thumb.height()).max(1.0);
            let top = (pos.y - press.grab).clamp(
                parts.track.top(),
                (parts.track.bottom() - parts.thumb.height()).max(parts.track.top()),
            );
            let t = ((top - parts.track.top()) / travel).clamp(0.0, 1.0);
            offset = t * max_off;
            moved = true;
        } else if max_off > 0.5 && still && now >= press.next {
            let step = match press.zone {
                1 => -line,
                2 => line,
                3 => -page,
                4 => page,
                _ => 0.0,
            };
            if step != 0.0 {
                offset = (offset + step).clamp(0.0, max_off);
                moved = true;
                let delay = if press.repeats == 0 { 0.32 } else { 0.06 };
                press.repeats = press.repeats.saturating_add(1);
                press.next = now + delay;
            }
        }
        if down {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
        ctx.data_mut(|d| d.insert_temp(key, press));
    } else {
        ctx.data_mut(|d| d.remove_temp::<DosPress>(key));
    }
    (offset, moved, held)
}

/// Paint the CHANNEL1 bar and store a new offset when the user moves it.
fn drive_dos_bar(
    ui: &Ui,
    bar: Rect,
    scroll_id: egui::Id,
    state: &mut egui::scroll_area::State,
    content_h: f32,
    view_h: f32,
    row_h: f32,
) {
    let max_off = (content_h - view_h).max(0.0);
    let line = row_h.max(1.0);
    let page = (view_h - line).max(line);
    let parts = dos_bar_parts(bar, state.offset.y, max_off, view_h);
    let (offset, moved, held) = dos_bar_input(
        ui,
        scroll_id,
        &parts,
        bar,
        state.offset.y,
        max_off,
        page,
        line,
    );
    if moved {
        state.offset.y = offset;
        (*state).store(ui.ctx(), scroll_id);
    }
    let parts = dos_bar_parts(bar, state.offset.y, max_off, view_h);
    paint_dos_scrollbar(ui.painter(), bar, &parts, held);
}

fn paint_dos_scrollbar(painter: &egui::Painter, bar: Rect, parts: &DosBarParts, held: DosHeld) {
    paint_dos_arrow_button(painter, parts.up, true, held.up);
    paint_dos_arrow_button(painter, parts.down, false, held.down);
    let shaft = Rect::from_min_max(
        Pos2::new(bar.left(), parts.up.bottom()),
        Pos2::new(bar.right(), parts.down.top().max(parts.up.bottom())),
    );
    if shaft.height() < 1.0 {
        return;
    }
    painter.rect_filled(shaft, 0.0, VGA_GRAY);
    let y = shaft.y_range();
    painter.vline(shaft.left() + 0.5, y, Stroke::new(1.0, Color32::WHITE));
    painter.vline(shaft.left() + 2.5, y, Stroke::new(1.0, Color32::BLACK));
    painter.vline(shaft.right() - 2.5, y, Stroke::new(1.0, Color32::WHITE));
    painter.vline(shaft.right() - 0.5, y, Stroke::new(1.0, Color32::BLACK));
    if parts.track.width() > 1.0 && parts.track.height() > 1.0 {
        painter.rect_filled(parts.track, 0.0, VGA_CYAN);
    }
    if parts.thumb.width() > 1.0 && parts.thumb.height() > 1.0 {
        painter.rect_filled(parts.thumb, 0.0, VGA_GRAY);
        paint_bevel(painter, parts.thumb, held.thumb);
    }
}

fn paint_dos_arrow_button(painter: &egui::Painter, rect: Rect, up: bool, pressed: bool) {
    if rect.height() < 4.0 || rect.width() < 4.0 {
        return;
    }
    painter.rect_filled(rect, 0.0, VGA_GRAY);
    paint_bevel(painter, rect, pressed);
    paint_dos_arrow(painter, rect, up, pressed);
}

/// Outline triangle, doubled vertically the way the VGA arrow is.
fn paint_dos_arrow(painter: &egui::Painter, rect: Rect, up: bool, pressed: bool) {
    let hi = if pressed {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let lo = if pressed {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    let left = rect.left() + 3.0;
    let right = rect.right() - 3.0;
    let top = rect.top() + 3.0;
    let bottom = rect.bottom() - 3.0;
    if right - left < 4.0 || bottom - top < 4.0 {
        return;
    }
    // The black right edge reads heavy, so the tip sits right of the shaft.
    let mid = ((left + right) * 0.5).floor() - 2.0;
    let span = (right - left).floor();
    let rows = (((bottom - top) / 2.0).floor() as i32).clamp(3, 7);
    for i in 0..rows {
        let t = i as f32 / (rows - 1).max(1) as f32;
        let half = (span * 0.5 * t).floor();
        let y = if up {
            top + i as f32 * 2.0
        } else {
            bottom - 2.0 - i as f32 * 2.0
        };
        let y = y.floor();
        let px = |x: f32, w: f32, color: Color32| {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(x, y), Vec2::new(w.max(1.0), 2.0)),
                0.0,
                color,
            );
        };
        if i == 0 {
            px(mid, 1.0, if up { hi } else { lo });
            continue;
        }
        if i + 1 == rows {
            if up {
                px(mid - half, 1.0, hi);
                px(mid - half + 1.0, (half * 2.0).max(1.0), lo);
            } else {
                px(mid - half, half * 2.0 + 1.0, hi);
            }
            continue;
        }
        px(mid - half, 1.0, hi);
        if half > 0.0 {
            px(mid + half, 1.0, lo);
        }
    }
}

/// Scrollable text. `id` keeps the Monitor pane and the channel split from
/// sharing one scrollbar position. The bytes are still the same pad.
///
/// `scroll_x` must stay off for monitor/channel pads: `ScrollArea::both`
/// makes `available_width` unbounded so word-wrap never fires. Rows are
/// wrapped to the live viewport width inside the vertical scroll area.
///
/// The vertical bar is the DOS one from CHANNEL1.png (cyan shaft, arrows,
/// gray thumb), not egui's overlay. The session-bar B control hides it.
///
/// With ScrLock off, `/` jumps to a new line and the stair crawls there.
fn paint_text_pane(
    ui: &mut Ui,
    rect: Rect,
    bg: Color32,
    id: &'static str,
    stick: bool,
    smooth: bool,
    owner: u64,
    show_bar: bool,
    nudge: f32,
    scroll_x: bool,
    font_px: f32,
    make_job: impl FnOnce(&Ui) -> LayoutJob,
) {
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        let avail = ui.available_size();
        ui.set_min_size(avail);
        // Cover the central panel. Its fill is monitor blue, and a short frame
        // leaves that blue on the right and bottom (worst on the grey MHeard).
        ui.painter().rect_filled(rect, 0.0, bg);
        // Fill the whole pane. A stroke on the right and bottom leaves a line,
        // and on MHeard that line is the monitor blue showing through.
        Frame::none().fill(bg).inner_margin(0.0).show(ui, |ui| {
            let full = ui.max_rect();
            ui.set_min_size(full.size());
            let bar_w = if show_bar {
                DOS_SCROLL_W.min(full.width() * 0.5)
            } else {
                0.0
            };
            let text_outer =
                Rect::from_min_max(full.min, Pos2::new(full.right() - bar_w, full.bottom()));
            let text = Rect::from_min_max(
                Pos2::new(text_outer.left() + 6.0, text_outer.top() + 4.0),
                Pos2::new(
                    (text_outer.right() - 4.0).max(text_outer.left() + 8.0),
                    (text_outer.bottom() - 4.0).max(text_outer.top() + 8.0),
                ),
            );
            let bar_rect = Rect::from_min_max(
                Pos2::new(text_outer.right(), full.top()),
                full.right_bottom(),
            );

            let area = if scroll_x {
                egui::ScrollArea::both()
            } else {
                egui::ScrollArea::vertical()
            };
            // Follow is applied after layout. egui's own stick jumps a whole line.
            // The DOS bar is drawn beside this viewport, so egui's bar stays hidden.
            let mut snap = None;
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(text), |ui| {
                ui.set_min_size(text.size());
                let out = area
                    .id_salt(id)
                    .animated(false)
                    .stick_to_bottom(false)
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        if nudge != 0.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, nudge));
                        }
                        let job = make_job(ui);
                        ui.add(egui::Label::new(job).wrap_mode(TextWrapMode::Extend));
                    });
                snap = Some((
                    out.id,
                    out.state,
                    out.content_size.y,
                    out.inner_rect.height(),
                ));
            });
            if let Some((sid, mut state, content_h, view_h)) = snap {
                let max_off = (content_h - view_h).max(0.0);
                let now = ui.input(|i| i.time);
                let ctx = ui.ctx().clone();
                let mut crawl = ctx
                    .data(|d| d.get_temp::<SoftCrawl>(sid))
                    .unwrap_or_default();
                let before = state.offset.y;
                // ScrLock off → follow. Turning it off again must resume even
                // if a prior lock had cleared `follow` while scrolled up.
                let unlocked = stick && !crawl.stick;

                if !stick {
                    crawl.follow = false;
                    crawl.carry = 0.0;
                } else if !crawl.seen {
                    crawl.seen = true;
                    crawl.owner = owner;
                    crawl.last_t = now;
                    state.offset.y = max_off;
                    crawl.follow = true;
                } else if unlocked {
                    crawl.owner = owner;
                    crawl.carry = 0.0;
                    crawl.last_t = now;
                    state.offset.y = max_off;
                    crawl.follow = true;
                } else if owner != crawl.owner {
                    // Another pad in this same viewport. Show its end at once;
                    // only lines that arrive afterwards crawl.
                    crawl.owner = owner;
                    crawl.carry = 0.0;
                    crawl.last_t = now;
                    if crawl.follow {
                        state.offset.y = max_off;
                    }
                } else {
                    let dt = (now - crawl.last_t).clamp(0.0, 0.5) as f32;
                    crawl.last_t = now;
                    crawl.follow = keep_following(
                        stick,
                        state.offset.y,
                        crawl.last_offset,
                        max_off,
                        crawl.last_max,
                        crawl.follow,
                    );
                    if crawl.follow {
                        let (y, carry) =
                            soft_scroll_toward(state.offset.y, max_off, crawl.carry, dt, smooth);
                        state.offset.y = y;
                        crawl.carry = carry;
                        if max_off - y > 0.5 {
                            ctx.request_repaint_after(Duration::from_millis(50));
                        }
                    } else {
                        crawl.carry = 0.0;
                    }
                }

                if show_bar && bar_rect.width() > 4.0 {
                    let row_h = ui
                        .fonts(|f| f.row_height(&FontId::monospace(font_px)))
                        .max(1.0);
                    let page = (view_h - row_h).max(row_h);
                    let parts = dos_bar_parts(bar_rect, state.offset.y, max_off, view_h);
                    let (offset, moved, held) = dos_bar_input(
                        ui,
                        sid,
                        &parts,
                        bar_rect,
                        state.offset.y,
                        max_off,
                        page,
                        row_h,
                    );
                    if moved {
                        state.offset.y = offset;
                        if max_off - state.offset.y > 1.0 {
                            crawl.follow = false;
                            crawl.carry = 0.0;
                        }
                    }
                    let parts = dos_bar_parts(bar_rect, state.offset.y, max_off, view_h);
                    paint_dos_scrollbar(ui.painter(), bar_rect, &parts, held);
                }

                crawl.stick = stick;
                crawl.seen = true;
                crawl.last_offset = state.offset.y;
                crawl.last_max = max_off;
                ctx.data_mut(|d| d.insert_temp(sid, crawl));
                if (state.offset.y - before).abs() > 0.01 {
                    state.store(&ctx, sid);
                }
            }
        });
    });
}

/// Characters from an IME commit (`°`, emoji, a dead-key umlaut).
/// Control characters are left out so a commit cannot sneak in a newline.
fn dropped_ime_commit(events: &[egui::Event]) -> String {
    let mut extra = String::new();
    for ev in events {
        if let egui::Event::Ime(egui::ImeEvent::Commit(text)) = ev {
            extra.extend(text.chars().filter(|c| !c.is_control()));
        }
    }
    extra
}

/// Insert `extra` at a character index. Returns the caret just after it.
fn insert_at_char(text: &mut String, char_index: usize, extra: &str) -> usize {
    let count = text.chars().count();
    let at = char_index.min(count);
    let byte = text
        .char_indices()
        .nth(at)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    text.insert_str(byte, extra);
    at + extra.chars().count()
}

fn compose_id_for(slot: usize) -> egui::Id {
    egui::Id::new(("gp_compose", slot))
}

fn place_compose_caret(ctx: &Context, id: egui::Id, char_index: usize) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(char_index);
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ctx, id);
}

/// Classic MS-DOS caret: a blinking underscore under the insert point.
fn paint_dos_caret(
    ui: &Ui,
    galley_pos: Pos2,
    galley: &egui::Galley,
    cursor: &egui::epaint::text::cursor::Cursor,
    row_h: f32,
    char_w: f32,
    color: Color32,
) {
    let rect =
        egui::text_selection::text_cursor_state::cursor_rect(galley_pos, galley, cursor, row_h);
    let style = &ui.visuals().text_cursor;
    let on = if style.blink {
        let t = ui.input(|i| i.time);
        let period = f64::from(style.on_duration + style.off_duration).max(0.05);
        let phase = t % period;
        if phase < f64::from(style.on_duration) {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f32(style.on_duration - phase as f32));
            true
        } else {
            ui.ctx().request_repaint_after(Duration::from_secs_f32(
                (style.on_duration + style.off_duration) - phase as f32,
            ));
            false
        }
    } else {
        true
    };
    if !on {
        return;
    }
    let y = rect.bottom() - 1.5;
    let x0 = rect.left();
    let x1 = x0 + char_w;
    ui.painter().line_segment(
        [Pos2::new(x0, y), Pos2::new(x1, y)],
        Stroke::new(2.0, color),
    );
}

enum HistMove {
    Stay,
    Load(usize),
    /// Arrow Down walked past the newest line, back to the text being typed.
    Restore,
}

/// `older` is Arrow Up. Newest history sits at `len - 1`.
fn history_step(len: usize, cursor: Option<usize>, older: bool) -> HistMove {
    if len == 0 {
        return HistMove::Stay;
    }
    if older {
        match cursor {
            None => HistMove::Load(len - 1),
            Some(0) => HistMove::Stay,
            Some(i) => HistMove::Load(i.saturating_sub(1)),
        }
    } else {
        match cursor {
            None => HistMove::Stay,
            Some(i) if i + 1 >= len => HistMove::Restore,
            Some(i) => HistMove::Load(i + 1),
        }
    }
}

/// True when a channel write has real chat text (not only CR/LF/spaces/BEL).
fn chunk_has_chat(chunk: &str) -> bool {
    strip_ansi(chunk)
        .chars()
        .any(|c| !c.is_control() && !c.is_whitespace())
}

fn clamp_split_lower(v: f32) -> f32 {
    v.clamp(0.15, 0.75)
}

/// Movement before a press on the session bar counts as a split drag.
/// Same distance egui uses before a press is no longer a button click.
const SPLIT_DRAG_SLOP: f32 = 6.0;

/// Pixels to apply this frame. `None` while the press is still a click.
fn split_drag_step(travel_before: f32, delta_y: f32) -> Option<f32> {
    let travel = travel_before + delta_y;
    if travel.abs() <= SPLIT_DRAG_SLOP {
        None
    } else if travel_before.abs() <= SPLIT_DRAG_SLOP {
        Some(travel)
    } else {
        Some(delta_y)
    }
}

/// Height of the monitor under the session bar. `body` is QSO + monitor.
fn split_monitor_height(body: f32, lower: f32) -> f32 {
    let min = 48.0;
    if body <= min * 2.0 {
        return (body * 0.5).max(0.0);
    }
    (body * clamp_split_lower(lower)).clamp(min, body - min)
}

/// Menu-colour air between neighbouring icon faces.
const ICON_STRIP_GAP: f32 = 5.0;
/// Menu-colour air above and below the pictures.
const ICON_STRIP_VPAD: f32 = 2.0;
/// Screen pixels per art pixel. 2 keeps a one-pixel stroke an even 2×2 block.
/// 1 is the file size. Anything between 1 and 2 makes some strokes fatter.
const ICON_PIXEL_SCALE: f32 = 2.0;
/// Every toolbar has eight slots (EXIT is the last).
const ICON_TOOLBAR_SLOTS: usize = 8;

fn icon_display_size(pixels_per_point: f32) -> Vec2 {
    let ppp = pixels_per_point.max(1.0);
    Vec2::new(
        ICON_WIDTH as f32 * ICON_PIXEL_SCALE / ppp,
        ICON_HEIGHT as f32 * ICON_PIXEL_SCALE / ppp,
    )
}

fn icon_strip_height(pixels_per_point: f32) -> f32 {
    icon_display_size(pixels_per_point).y + ICON_STRIP_VPAD * 2.0
}

fn snap_to_pixel(v: f32, pixels_per_point: f32) -> f32 {
    let ppp = pixels_per_point.max(1.0);
    (v * ppp).round() / ppp
}

/// Minimum width for `count` icons with [`ICON_STRIP_GAP`] between them.
fn icon_row_min_width(pixels_per_point: f32, count: usize) -> f32 {
    let size = icon_display_size(pixels_per_point);
    let n = count.max(1) as f32;
    n * size.x + (n - 1.0).max(0.0) * ICON_STRIP_GAP
}

pub fn run(cfg: Config, config_path: PathBuf, radio: RadioLink) -> anyhow::Result<()> {
    let title = version::window_title();
    let min_w = icon_row_min_width(1.0, ICON_TOOLBAR_SLOTS);
    let width = cfg.window.width.max(min_w);
    let height = cfg.window.height.max(700.0);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([width, height])
        .with_min_inner_size([min_w, 700.0])
        .with_title(title.clone())
        .with_icon(load_window_icon());
    if let (Some(x), Some(y)) = (cfg.window.x, cfg.window.y) {
        viewport = viewport.with_position([x, y]);
    }
    if cfg.window.maximized {
        viewport = viewport.with_maximized(true);
    }

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
            let app = GpGuiApp::new(&cc.egui_ctx, (*cfg).clone(), (*config_path).clone(), radio);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn idle_monitor_polls_twice_a_second_and_catches_the_clock() {
        assert_eq!(idle_monitor_repaint(0), Duration::from_millis(500));
        assert_eq!(idle_monitor_repaint(400), Duration::from_millis(500));
        // Past the half: the next whole second is sooner, so the hand stays on time.
        let near = idle_monitor_repaint(700);
        assert_eq!(near, until_next_clock_second(700));
        assert!(near < Duration::from_millis(500));
        let late = idle_monitor_repaint(980);
        assert!(late >= Duration::from_millis(50) && late <= Duration::from_millis(80));
    }

    #[test]
    fn icon_editor_up_increases_and_down_decreases_icons_and_both_colors() {
        with_offline_app("wa8ded", |app, _, ctx| {
            let icon = crate::ui::icons::GpIcon {
                width: 70,
                height: 40,
                pixels: vec![7; 70 * 40],
            };
            let mut edit = IconEdit {
                path: app.config_path.with_file_name("ICONS.GP"),
                raw: Vec::new(),
                icons: vec![icon; 17],
                index: 0,
                color: 0,
                right_color: 0,
            };
            for (label, occurrence, expected) in [
                ("Up", 0, (1, 0, 0)),
                ("Down", 0, (0, 0, 0)),
                ("Down", 0, (16, 0, 0)),
                ("Up", 0, (0, 0, 0)),
                ("Up", 1, (0, 1, 0)),
                ("Down", 1, (0, 0, 0)),
                ("Down", 1, (0, 15, 0)),
                ("Up", 1, (0, 0, 0)),
                ("Up", 2, (0, 0, 1)),
                ("Down", 2, (0, 0, 0)),
                ("Down", 2, (0, 0, 15)),
                ("Up", 2, (0, 0, 0)),
            ] {
                let mut target = Pos2::ZERO;
                for pressed in [None, Some(true), Some(false)] {
                    let mut input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(1000.0, 900.0),
                        )),
                        ..Default::default()
                    };
                    if let Some(pressed) = pressed {
                        input.events = vec![
                            egui::Event::PointerMoved(target),
                            egui::Event::PointerButton {
                                pos: target,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::default(),
                            },
                        ];
                    }
                    let output = ctx.run(input, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            icon_editor_side(ui, &mut edit, &mut false, &mut None, &mut None);
                        });
                    });
                    target = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == label => {
                                Some(text.pos + text.galley.size() * 0.5)
                            }
                            _ => None,
                        })
                        .nth(occurrence)
                        .expect("cycle button rendered");
                }
                assert_eq!(
                    (edit.index, edit.color, edit.right_color),
                    expected,
                    "{label} control {occurrence}"
                );
            }
        });
    }

    #[test]
    fn icon_editor_draws_independent_left_and_right_colors_without_background_actions() {
        with_offline_app("wa8ded", |app, _, ctx| {
            app.active = ActivePane::Channel(1);
            app.icon_editor = Some(IconEdit {
                path: app.config_path.with_file_name("ICONS.GP"),
                raw: Vec::new(),
                icons: vec![crate::ui::icons::GpIcon {
                    width: 70,
                    height: 40,
                    pixels: vec![7; 70 * 40],
                }],
                index: 0,
                color: 4,
                right_color: 1,
            });
            for (button, color) in [
                (egui::PointerButton::Primary, 10u8),
                (egui::PointerButton::Secondary, 12u8),
            ] {
                let mut target = Pos2::ZERO;
                for pressed in [None, Some(true), Some(false)] {
                    let mut input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(1000.0, 700.0),
                        )),
                        ..Default::default()
                    };
                    if let Some(pressed) = pressed {
                        input.events = vec![
                            egui::Event::PointerMoved(target),
                            egui::Event::PointerButton {
                                pos: target,
                                button,
                                pressed,
                                modifiers: egui::Modifiers::default(),
                            },
                        ];
                    }
                    let output = ctx.run(input, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            icon_editor_side(
                                ui,
                                app.icon_editor.as_mut().unwrap(),
                                &mut false,
                                &mut None,
                                &mut None,
                            );
                        });
                    });
                    let (r, g, b) = VGA16_RGB[color as usize];
                    target = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if rect.rect.size() == Vec2::splat(20.0)
                                    && rect.fill == Color32::from_rgb(r, g, b) =>
                            {
                                Some(rect.rect.center())
                            }
                            _ => None,
                        })
                        .expect("palette color is rendered");
                }
                let edit = app.icon_editor.as_ref().unwrap();
                assert_eq!(
                    if button == egui::PointerButton::Primary {
                        edit.color
                    } else {
                        edit.right_color
                    },
                    color
                );
            }
            app.icon_editor.as_mut().unwrap().color = 4;
            app.icon_editor.as_mut().unwrap().right_color = 1;
            assert!(app.modal_open());
            assert!(app.popup_chrome_open());
            let split = app.split_on;
            let mut grid = Rect::NOTHING;
            for (button, x, color) in [
                (egui::PointerButton::Primary, 2, 4),
                (egui::PointerButton::Secondary, 3, 1),
            ] {
                for pressed in [None, Some(true), Some(true), Some(false)] {
                    let pos = grid.min + Vec2::new(x as f32 * 8.0 + 4.0, 12.0);
                    let mut input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(1000.0, 700.0),
                        )),
                        ..Default::default()
                    };
                    if let Some(pressed) = pressed {
                        input.events = vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button,
                                pressed,
                                modifiers: egui::Modifiers::default(),
                            },
                        ];
                    }
                    let _ = ctx.run(input, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            grid = Rect::from_min_size(ui.cursor().min, Vec2::new(560.0, 320.0));
                            paint_icon_grid(ui, app.icon_editor.as_mut().unwrap());
                            ui.scope(|ui| {
                                ui.disable();
                                app.note_split_right_click(ui, grid);
                                app.session_bar(ui, 900.0, 26.0);
                                let bar = Rect::from_min_size(grid.min, Vec2::new(40.0, 300.0));
                                let parts = dos_bar_parts(bar, 100.0, 500.0, 100.0);
                                let (offset, moved, _) = dos_bar_input(
                                    ui,
                                    egui::Id::new("icon_background"),
                                    &parts,
                                    bar,
                                    100.0,
                                    500.0,
                                    100.0,
                                    10.0,
                                );
                                assert_eq!(offset, 100.0);
                                assert!(
                                    !moved,
                                    "held grid input must not scroll behind the editor"
                                );
                            });
                        });
                    });
                }
                assert_eq!(
                    app.icon_editor.as_ref().unwrap().icons[0].pixels[70 + x],
                    color
                );
                assert_eq!(app.split_on, split);
                assert!(!app.split_dragging);
            }
            assert_eq!(app.icon_editor.as_ref().unwrap().icons[0].pixels[70 + 2], 4);
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::A,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::ALT,
                    }],
                    ..Default::default()
                },
                |ctx| app.poll_hotkeys(ctx),
            );
            assert!(app.icon_editor.is_some());
        });
    }

    #[test]
    fn connect_names_window_shows_all_qrgs_before_the_active_mhz_list() {
        with_offline_app("wa8ded", |app, _, ctx| {
            let path = app.config_path.with_file_name("NAMES.GP");
            std::fs::write(
                &path,
                "<IF 27>\nT>N0AAA Band;\n<END>\n\
                T>N1BBB Global;\n<IF 145>\nT>N2CCC Hidden;\n<END>",
            )
            .unwrap();
            app.names = NamesFile::load_from(path);
            app.session.qrg_log = "27.235".into();
            app.session.show_connect_dialog = true;
            app.show_connect_names = true;
            for pass in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        time: Some(pass as f64),
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                        ..Default::default()
                    },
                    |ctx| app.connect_names_window(ctx),
                );
                if pass == 1 {
                    let texts: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) => Some((text.galley.text(), text.pos.y)),
                            _ => None,
                        })
                        .collect();
                    let y = |label| texts.iter().find(|(text, _)| *text == label).unwrap().1;
                    assert!(y("All QRGs") < y("N1BBB (Global)"));
                    assert!(y("N1BBB (Global)") < y("27MHz List"));
                    assert!(y("27MHz List") < y("N0AAA (Band)"));
                    assert!(!texts.iter().any(|(text, _)| *text == "N2CCC (Hidden)"));
                }
            }
        });
    }

    #[test]
    fn connect_names_selection_passes_exact_via_and_queues_node_routes_without_silent_fallback() {
        for protocol in ["wa8ded", "kiss", "baycom"] {
            with_offline_app(protocol, |app, commands, _| {
                let path = app.config_path.with_file_name("NAMES.GP");
                std::fs::write(
                    &path,
                    "T>N0PEER Global; D>N0DIGI\n\
                    <IF 27>\nT>N0PEER Band; D>N1DIGI\n\
                    T>N1PEER Node; D>N2DIGI N>N0NODE !>N1NODE T>N1PEER\n\
                    T>N2PEER Broken; D>%DIGI%\n<END>",
                )
                .unwrap();
                app.names = NamesFile::load_from(path);
                app.session.qrg_log = "27.235".into();
                let listed = app.names.name_list(&app.names_qrg());
                for (name, target, via, downlinks) in [
                    ("Global", "N0PEER", "N0DIGI", Vec::<String>::new()),
                    ("Band", "N0PEER", "N1DIGI", Vec::new()),
                    (
                        "Node",
                        "N0NODE",
                        "N2DIGI",
                        vec!["C! N1NODE".into(), "C N1PEER".into()],
                    ),
                ] {
                    app.clear_channel_link_state(1);
                    app.session.connect_channel = 1;
                    app.session.show_connect_dialog = true;
                    app.show_connect_names = true;
                    let entry = listed.iter().find(|entry| entry.name == name).unwrap();
                    app.connect_listed(entry);
                    let mut sent = None;
                    while let Ok(command) = commands.try_recv() {
                        if let TncCommand::Session(SessionCommand::Connect {
                            channel,
                            remote,
                            via,
                            ..
                        }) = command
                        {
                            assert_eq!(channel, 1);
                            sent = Some((remote, via));
                        }
                    }
                    assert_eq!(
                        sent,
                        Some((target.into(), vec![via.into()])),
                        "{protocol}: {name}"
                    );
                    assert_eq!(app.channel_down[1], downlinks, "{protocol}: {name}");
                    assert!(!app.session.show_connect_dialog);
                    assert!(!app.show_connect_names);
                }
                app.clear_channel_link_state(1);
                app.session.show_connect_dialog = true;
                app.show_connect_names = true;
                app.connect_listed(listed.iter().find(|entry| entry.name == "Broken").unwrap());
                assert!(app.status.contains("no PORT matches QRG"));
                assert!(app.session.show_connect_dialog);
                assert!(app.show_connect_names);
                assert!(commands.try_recv().is_err());
                assert!(app.channel_connecting[1].is_none());
            });
        }
    }

    #[test]
    fn station_editor_saves_manual_entries_and_tracks_known_connected_names() {
        with_offline_app("wa8ded", |app, _, ctx| {
            app.names = NamesFile::load_from(app.config_path.with_file_name("NAMES.GP"));
            app.active = ActivePane::Channel(1);
            assert_eq!(app.peer_name_label(1), ("Name".into(), false));
            app.open_name_dialog();
            assert!(app.show_name_dialog);
            assert!(app.name_edit.call.is_empty());
            app.name_edit = NameEntry {
                call: "N0PEER".into(),
                name: "Known".into(),
                kind: StationKind::Mailbox,
                via: "N0DIGI".into(),
                scope: NameScope::Qrg("145".into()),
                ..NameEntry::default()
            };
            let _ = ctx.run(egui::RawInput::default(), |ctx| app.name_dialog(ctx));
            app.apply_name_from_edit().unwrap();
            app.names = NamesFile::load_from(app.names.path().to_path_buf());
            app.session.qrg_log = "145.825".into();
            app.channel_peer[1] = Some("N0PEER-2".into());
            assert_eq!(app.peer_name_label(1), ("Name: Known".into(), true));
            app.open_name_dialog();
            assert_eq!(app.name_edit.call, "N0PEER");
            assert_eq!(app.name_edit.kind, StationKind::Mailbox);
            assert_eq!(app.name_edit.via, "N0DIGI");
            app.clear_channel_link_state(1);
            assert!(
                app.show_name_dialog,
                "disconnect must not discard the editor draft"
            );
            assert_eq!(app.peer_name_label(1), ("Name".into(), false));
            app.name_edit.name = "Updated".into();
            app.apply_name_from_edit().unwrap();
            assert_eq!(
                app.names.name_for("N0PEER", "145").as_deref(),
                Some("Updated")
            );
            app.channel_peer[1] = Some("N1NEW".into());
            assert_eq!(app.peer_name_label(1), ("Name".into(), false));
            app.open_name_dialog();
            assert_eq!(app.name_edit.call, "N1NEW");
            assert!(app.name_source.is_none());
            assert!(app.remove_name_from_edit().is_err());
            app.name_edit.name = "Eleven".into();
            app.name_edit.scope = NameScope::Qrg("27".into());
            app.apply_name_from_edit().unwrap();
            app.session.qrg_log = "27.235".into();
            assert_eq!(app.peer_name_label(1), ("Name: Eleven".into(), true));
            app.open_name_dialog();
            app.name_edit.call = "N2DRAFT".into();
            app.remove_name_from_edit().unwrap();
            assert_eq!(app.status, "NAMES.GP N1NEW removed");
            assert_eq!(app.peer_name_label(1), ("Name".into(), false));
            assert!(app.name_source.is_none());
            assert!(app.name_edit.call.is_empty());
            let reloaded = NamesFile::load_from(app.names.path().to_path_buf());
            assert!(reloaded.name_for("N1NEW", "27.235").is_none());
            assert_eq!(
                reloaded.name_for("N0PEER", "145").as_deref(),
                Some("Updated")
            );
        });
    }

    #[test]
    fn station_names_selectors_are_compact_square_bevelled_and_single_line() {
        let ctx = Context::default();
        install_console_font(&ctx, false);
        let labels = [
            "NL0MSK (Jara) - All QRGs",
            "B - Mailbox / BBS",
            "11 m - 27 MHz",
        ];
        let mut responses: Vec<egui::Response> = Vec::new();
        let mut rows = Vec::new();
        for frame in 0..7 {
            if frame % 2 == 1 {
                ctx.memory_mut(|m| m.open_popup(responses[frame / 2].id.with("popup")));
            }
            responses.clear();
            let output = ctx.run(
                egui::RawInput {
                    time: Some(frame as f64),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(520.0, 800.0))),
                    ..egui::RawInput::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_max_width(440.0);
                        let spacing = ui.spacing().clone();
                        for (index, label) in labels.iter().enumerate() {
                            responses.push(name_selector(
                                ui,
                                &format!("names_{index}"),
                                *label,
                                |ui| {
                                    rows.push(name_select_row(ui, label, false, false));
                                },
                            ));
                        }
                        assert_eq!(ui.spacing().interact_size, spacing.interact_size);
                        assert_eq!(ui.spacing().button_padding, spacing.button_padding);
                    });
                },
            );
            for response in responses.iter().chain(&rows) {
                assert!(response.rect.height() >= retro::ROW_H * 0.75);
                assert!(response.rect.height() <= retro::ROW_H);
            }
            for label in labels {
                let text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                        _ => None,
                    })
                    .expect("selector label is rendered");
                assert_eq!(text.galley.rows.len(), 1, "{label}");
                assert!(
                    text.galley
                        .job
                        .sections
                        .iter()
                        .all(|section| section.format.font_id == menu_font(17.0)),
                    "{label}"
                );
                assert!(
                    text.galley.size().x < responses[0].rect.width() - 32.0,
                    "{label}"
                );
                assert!(
                    text.galley.mesh_bounds.height() <= responses[0].rect.height() - 4.0,
                    "{label}: ink height {}",
                    text.galley.mesh_bounds.height()
                );
            }
            for response in responses
                .iter()
                .chain(rows.last().filter(|_| frame > 0 && frame % 2 == 0))
            {
                let edges: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect) if response.rect.contains_rect(rect.rect) => {
                            Some(rect)
                        }
                        _ => None,
                    })
                    .collect();
                assert!(edges
                    .iter()
                    .all(|edge| edge.rounding == egui::Rounding::ZERO));
                assert!(
                    edges.iter().any(|edge| edge.fill == Color32::WHITE),
                    "frame {frame}, rect {:?}, edges {:?}",
                    response.rect,
                    edges
                        .iter()
                        .map(|edge| (edge.rect, edge.fill))
                        .collect::<Vec<_>>()
                );
                assert!(edges.iter().any(|edge| edge.fill == Color32::BLACK));
            }
        }
        assert_eq!(
            rows.len(),
            6,
            "each popup gets a sizing pass and a visible pass"
        );
        let target = rows.last().unwrap().rect.center();
        let mut chosen = None;
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(520.0, 800.0))),
                    events: vec![
                        egui::Event::PointerMoved(target),
                        egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..egui::RawInput::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_max_width(440.0);
                        for (index, label) in labels.iter().enumerate() {
                            name_selector(ui, &format!("names_{index}"), *label, |ui| {
                                if name_select_row(ui, label, false, false).clicked() {
                                    chosen = Some(index);
                                }
                            });
                        }
                    });
                },
            );
        }
        assert_eq!(chosen, Some(2));
        assert!(!ctx.memory(|m| m.is_popup_open(responses[2].id.with("popup"))));
    }

    #[test]
    fn tfpcx_start_card_moves_up_clear_of_the_credits() {
        fn paint(app: &mut GpGuiApp, ctx: &Context, size: Vec2) -> Rect {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| app.paint_start_screen(ctx),
            );
            ctx.memory(|mem| mem.area_rect(egui::Id::new("gp_radio_setup")))
                .expect("radio card")
        }

        fn settle(app: &mut GpGuiApp, ctx: &Context, size: Vec2) -> Rect {
            let mut rect = Rect::NOTHING;
            for _ in 0..4 {
                rect = paint(app, ctx, size);
            }
            rect
        }

        let screen = Vec2::new(1160.0, 851.0);
        let credits_top = screen.y - START_CREDITS_H;
        with_offline_app("kiss", |app, _, ctx| {
            let dir = app.config_path.parent().unwrap();
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(&app.config_path, toml::to_string(&app.cfg).unwrap()).unwrap();

            let kiss = settle(app, ctx, screen);
            assert!(
                (kiss.top() - 400.0).abs() < 2.0,
                "kiss card left its anchor: {kiss:?}"
            );
            assert!(
                kiss.bottom() <= credits_top,
                "kiss overlaps credits: {kiss:?}"
            );

            app.radio_protocol = HostProtocol::Wa8ded;
            let wa8 = settle(app, ctx, screen);
            assert!(
                (wa8.top() - kiss.top()).abs() < 2.0,
                "wa8ded card moved: {wa8:?} kiss {kiss:?}"
            );

            app.radio_protocol = HostProtocol::Baycom;
            app.radio_baud = 1200;
            let bay = settle(app, ctx, screen);
            assert!(
                (bay.top() - kiss.top()).abs() < 2.0,
                "baycom card moved: {bay:?} kiss {kiss:?}"
            );

            app.radio_protocol = HostProtocol::Tfpcx;
            let tfpcx = settle(app, ctx, screen);
            assert!(
                tfpcx.top() + 8.0 < kiss.top(),
                "tfpcx card was not pushed up: {tfpcx:?} kiss {kiss:?} height {}",
                app.start_card_height
            );
            assert!(
                tfpcx.bottom() <= credits_top - 4.0,
                "tfpcx overlaps credits: {tfpcx:?} credits_top {credits_top} height {}",
                app.start_card_height
            );

            app.radio_links[0].tcp = true;
            let tcp = settle(app, ctx, screen);
            assert!(
                tcp.bottom() <= credits_top - 4.0,
                "tfpcx tcp overlaps credits: {tcp:?} height {}",
                app.start_card_height
            );

            app.radio_protocol = HostProtocol::Kiss;
            let back = settle(app, ctx, screen);
            assert!(
                (back.top() - kiss.top()).abs() < 2.0,
                "kiss card did not return: {back:?} vs {kiss:?}"
            );

            app.radio_protocol = HostProtocol::Tfpcx;
            let tall = settle(app, ctx, Vec2::new(1160.0, 1400.0));
            assert!(
                (tall.top() - kiss.top()).abs() < 2.0,
                "tfpcx moved while the credits were already clear: {tall:?}"
            );
            assert!(tall.bottom() <= 1400.0 - START_CREDITS_H);
        });
    }

    #[test]
    fn radio_protocol_choices_persist_with_baycom_software_sessions_and_radio_rate() {
        with_offline_app("wa8ded", |app, _, _| {
            std::fs::write(&app.config_path, toml::to_string(&app.cfg).unwrap()).unwrap();
            app.radio_call = "N0ME".into();
            app.radio_locator = "JO32KX".into();
            app.radio_port = "COM999".into();
            app.radio_baud = 9600;
            for (index, protocol, label) in [
                (0, "kiss", "KISS"),
                (1, "wa8ded", "TAPR TNC-2 WA8DED"),
                (2, "baycom", "Baycom"),
                (3, "tfpcx", "TFPCX - Multi Modem"),
            ] {
                app.apply_radio_pick(RadioPick::Protocol, index, &[]);
                app.store_radio_form().unwrap();
                let saved: Config =
                    toml::from_str(&std::fs::read_to_string(&app.config_path).unwrap()).unwrap();
                assert_eq!(saved.tnc.protocol, protocol);
                assert_eq!(saved.station.locator, "JO32KX");
                assert_eq!(saved.tnc.protocol_kind(), app.radio_protocol);
                assert_eq!(protocol_label(saved.tnc.protocol_kind()), label);
                if saved.tnc.protocol_kind() == HostProtocol::Baycom {
                    assert_eq!(saved.serial.baud, 1200);
                    app.apply_radio_pick(RadioPick::Baud, 1, &[]);
                    assert_eq!(app.radio_baud, 1200);
                    app.radio_baud = 9600;
                    assert!(app.store_radio_form().unwrap_err().contains("1200"));
                    app.radio_baud = 1200;
                    assert!(app.software_ax25());
                }
            }
            app.radio_locator = "jo32mw49".into();
            app.store_radio_form().unwrap();
            let saved: Config =
                toml::from_str(&std::fs::read_to_string(&app.config_path).unwrap()).unwrap();
            assert_eq!(saved.station.locator, "JO32MW49");
            app.radio_locator = "JO32".into();
            app.store_radio_form().unwrap();
            let saved: Config =
                toml::from_str(&std::fs::read_to_string(&app.config_path).unwrap()).unwrap();
            assert_eq!(saved.station.locator, "JO32MW49");
            app.radio_locator.clear();
            app.store_radio_form().unwrap();
            let saved: Config =
                toml::from_str(&std::fs::read_to_string(&app.config_path).unwrap()).unwrap();
            assert_eq!(saved.station.locator, "");
            assert!(locator_blocks_start("JO32").is_some());
            assert!(locator_blocks_start("").is_none());
        });
        with_offline_app("baycom", |app, _, _| {
            assert_eq!(app.radio_protocol, HostProtocol::Baycom);
            assert_eq!(app.radio_baud, 1200);
        });
    }

    fn with_offline_app(
        protocol: &str,
        test: impl FnOnce(&mut GpGuiApp, &mut mpsc::Receiver<TncCommand>, &Context),
    ) {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("gpng-connect-{}-{unique}", std::process::id()));
        let mut cfg = Config::default();
        cfg.tnc.protocol = protocol.into();
        cfg.paths.user_dir = dir.join("USER");
        cfg.paths.external_dir = dir.join("EXTERNAL");
        cfg.paths.log_dir = dir.join("LOG");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ctx = Context::default();
        let mut app = GpGuiApp::new(
            &ctx,
            cfg,
            dir.join("Config.toml"),
            RadioLink::new(rt.handle().clone()),
        );
        let (tx, mut rx) = mpsc::channel(32);
        app.cmd_tx = tx;
        test(&mut app, &mut rx, &ctx);
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn alt_h_opens_the_mheard_list() {
        with_offline_app("wa8ded", |app, _, ctx| {
            app.show_radio_gate = false;
            assert_eq!(app.active, ActivePane::Monitor);
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: egui::Modifiers::ALT,
                    events: vec![egui::Event::Key {
                        key: egui::Key::H,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::ALT,
                    }],
                    ..Default::default()
                },
                |ctx| app.poll_hotkeys(ctx),
            );
            assert_eq!(app.active, ActivePane::Mh);
        });
    }

    #[test]
    fn chat_relay_receives_packets_without_waiting_for_line_endings() {
        for protocol in ["wa8ded", "kiss", "baycom"] {
            with_offline_app(protocol, |app, rx, _| {
                app.channel_peer[1] = Some("N1PEER".into());
                app.channel_peer[2] = Some("N2PEER".into());
                if !app.session.remote_for(1) {
                    app.session.toggle_remote(ActivePane::Channel(1));
                }
                app.accept_rx_text(1, "//2\r", PadKind::Incoming);
                assert_eq!(app.chat_target(1), Some(2));
                while rx.try_recv().is_ok() {}

                for (from, to) in [(1, 2), (2, 1)] {
                    for ending in ["", "\r", "\n", "\r\n"] {
                        app.accept_rx_text(from, &format!("hello{ending}"), PadKind::Incoming);
                        let command = rx.try_recv().expect("chat must relay immediately");
                        let (channel, data) = match command {
                            TncCommand::HostFrame { channel, data }
                            | TncCommand::Session(SessionCommand::Send { channel, data, .. }) => {
                                (channel, data)
                            }
                            other => panic!("unexpected radio command: {other:?}"),
                        };
                        assert_eq!(channel, to);
                        assert_eq!(data, format!("N{from}PEER: hello\r").as_bytes());
                        assert!(rx.try_recv().is_err(), "chat relayed more than once");
                        app.accept_rx_text(from, "\n", PadKind::Incoming);
                        assert!(rx.try_recv().is_err(), "late newline repeated chat");
                    }
                }

                app.accept_rx_text(1, "//", PadKind::Incoming);
                assert_eq!(app.chat_target(1), Some(2));
                assert!(rx.try_recv().is_err());
                app.accept_rx_text(1, "0\r", PadKind::Incoming);
                assert_eq!(app.chat_target(1), None);
                while rx.try_recv().is_ok() {}
                app.accept_rx_text(2, "after leaving", PadKind::Incoming);
                assert!(rx.try_recv().is_err());
            });
        }
    }

    #[test]
    fn non_relay_chat_keeps_reassembling_packets() {
        with_offline_app("wa8ded", |app, rx, _| {
            app.channel_peer[1] = Some("N1PEER".into());
            app.accept_rx_text(1, "hello", PadKind::Incoming);
            app.accept_rx_text(1, " world", PadKind::Incoming);
            let seq = app.rx_row[1].expect("ordinary chat row stays open");
            assert_eq!(app.pads[1].text_if_seq(seq).unwrap(), "hello world");
            app.accept_rx_text(1, "\r\n", PadKind::Incoming);
            assert!(app.rx_row[1].is_none());
            assert!(rx.try_recv().is_err());
        });
    }

    #[test]
    fn cstatus_names_the_radio_port_only_when_the_link_has_one() {
        with_offline_app("kiss", |app, _, _| {
            app.channel_peer[1] = Some("N1PEER".into());
            let plain = app.cstatus_text();
            assert!(plain.contains("N1PEER"), "{plain}");
            assert!(!plain.contains("port"), "{plain}");
            app.channel_link_port[1] = Some(2);
            let marked = app.cstatus_text();
            assert!(marked.contains("N1PEER port 2"), "{marked}");
            assert!(marked.starts_with(&plain), "{marked}");
        });
    }

    #[test]
    fn remote_command_replies_end_in_crlf_across_packet_boundaries() {
        for protocol in ["wa8ded", "kiss"] {
            with_offline_app(protocol, |app, rx, _| {
                app.active = ActivePane::Channel(1);
                app.channel_peer[1] = Some("N1PEER".into());
                app.remote_prompt = false;
                app.cfg.buffers.packet_payload = 16;
                for (command, prefix) in [
                    ("//CS", "<GPNG>: Channel state"),
                    ("//VERSION", "<GPNG>: Version"),
                    ("//ECHO hello", "hello"),
                    ("//CAT", "<GPNG>: USER CONTENT"),
                ] {
                    let (action, args) = parse_line(command).unwrap();
                    app.handle_remote_on_channel(1, action, &args);
                    let mut wire = Vec::new();
                    while let Ok(command) = rx.try_recv() {
                        let TncCommand::Session(SessionCommand::Send { channel, data, .. }) =
                            command
                        else {
                            panic!("unexpected radio command");
                        };
                        assert_eq!(channel, 1);
                        assert!(data.len() <= 16);
                        wire.extend(data);
                    }
                    assert!(wire.starts_with(prefix.as_bytes()), "{protocol}: {command}");
                    assert!(wire.ends_with(b"\r\n"), "{protocol}: {command}: {wire:?}");
                }
            });
        }
    }

    #[test]
    fn remote_reply_terminates_existing_line_endings_without_extra_blank_lines() {
        with_offline_app("wa8ded", |app, rx, _| {
            app.active = ActivePane::Channel(1);
            for text in ["hello", "hello\r", "hello\n", "hello\r\n"] {
                app.send_remote_text(text);
                let TncCommand::Session(SessionCommand::Send { data, .. }) = rx.try_recv().unwrap()
                else {
                    panic!("expected channel text");
                };
                assert_eq!(data, b"hello\r\n", "{text:?}");
            }
            app.send_gpi_text("streaming");
            let TncCommand::Session(SessionCommand::Send { data, .. }) = rx.try_recv().unwrap()
            else {
                panic!("expected channel text");
            };
            assert_eq!(data, b"streaming");
        });
    }

    fn click_first_channel_icon(app: &mut GpGuiApp, ctx: &Context) {
        let mut position = Pos2::ZERO;
        for pressed in [None, Some(true), Some(false)] {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 200.0))),
                ..Default::default()
            };
            if let Some(pressed) = pressed {
                input.events.push(egui::Event::PointerMoved(position));
                input.events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let size = icon_display_size(ctx.pixels_per_point());
                    let width = 8.0 * size.x + 7.0 * ICON_STRIP_GAP;
                    position = ui.cursor().min + Vec2::new(size.x * 0.5, size.y * 0.5);
                    app.icon_strip(ui, width, size.y);
                });
            });
        }
    }

    #[test]
    fn software_monitor_lists_both_directions_control_frames_and_repeats_on_each_port() {
        for protocol in ["kiss", "baycom"] {
            with_offline_app(protocol, |app, commands, _| {
                app.pads[0].lines.clear();
                app.monitor_filter = MonitorFilter::iusc();
                app.channel_peer[1] = Some("N1TEST".into());
                let channel_rows = app.pads[1].lines.len();
                let (events, rx) = mpsc::channel(64);
                app.events = rx;
                let frames = [
                    ax25::build_sabm("N0ME", "N1TEST"),
                    ax25::build_ua("N0ME", "N1TEST", true),
                    ax25::build_i("N0ME", "N1TEST", 2, 3, false, b"binary\x00\xDB\xC0"),
                    ax25::build_rr("N0ME", "N1TEST", 3, true, false),
                    ax25::build_disc("N0ME", "N1TEST"),
                ];
                for port in [0, 3, 12] {
                    for frame in frames.iter().chain(frames.iter().take(1)) {
                        events
                            .try_send(TncEvent::HostFrame {
                                channel: port,
                                code: HostCode::Data,
                                payload: frame.clone(),
                            })
                            .unwrap();
                        events
                            .try_send(TncEvent::KissTx {
                                port,
                                frame: frame.clone(),
                            })
                            .unwrap();
                    }
                }
                app.drain_tnc();
                for kind in [PadKind::Incoming, PadKind::Own] {
                    assert_eq!(
                        app.pads[0]
                            .lines
                            .iter()
                            .filter(|line| line.kind == kind && line.text.contains(" ctl "))
                            .count(),
                        18
                    );
                }
                assert!(app.pads[0]
                    .lines
                    .iter()
                    .any(|line| line.text.starts_with("fm12 ")));
                assert_eq!(app.pads[1].lines.len(), channel_rows);
                assert!(commands.try_recv().is_err());

                app.pads[0].lines.clear();
                app.monitor_filter = MonitorFilter::default();
                for event in [
                    TncEvent::HostFrame {
                        channel: 12,
                        code: HostCode::Data,
                        payload: frames[0].clone(),
                    },
                    TncEvent::KissTx {
                        port: 12,
                        frame: frames[0].clone(),
                    },
                ] {
                    events.try_send(event).unwrap();
                }
                app.drain_tnc();
                assert!(
                    app.pads[0].lines.is_empty(),
                    "M N still hides decoded frames"
                );
            });
        }
    }

    #[test]
    fn kiss_undecodable_rx_and_tx_remain_visible_as_raw_monitor_diagnostics() {
        with_offline_app("kiss", |app, _, _| {
            app.pads[0].lines.clear();
            let (events, rx) = mpsc::channel(4);
            app.events = rx;
            let raw = vec![0x01, 0xDB, 0xC0];
            events
                .try_send(TncEvent::HostFrame {
                    channel: 12,
                    code: HostCode::Data,
                    payload: raw.clone(),
                })
                .unwrap();
            events
                .try_send(TncEvent::KissTx {
                    port: 12,
                    frame: raw,
                })
                .unwrap();
            app.drain_tnc();
            let lines: Vec<_> = app.pads[0].lines.iter().collect();
            assert_eq!(lines.len(), 2);
            for line in lines {
                assert_eq!(line.kind, PadKind::System);
                let rest = line.text.strip_prefix("GPNG ").unwrap();
                let (time, message) = rest.split_once(' ').unwrap();
                assert!(chrono::NaiveTime::parse_from_str(time, "%H:%M").is_ok());
                assert_eq!(message, "KISS port 12: undecodable AX.25 (3B) 01 DB C0");
            }
            let info = Color32::from_rgb(255, 168, 0);
            let job = app.pads[0].layout_job_monitor(
                100,
                Color32::BLUE,
                Color32::WHITE,
                info,
                padlog::PAD_FONT_SIZE,
            );
            assert!(job
                .sections
                .iter()
                .all(|section| section.format.color == info));
            assert!(app.pads[0].transcript_text().is_empty());
        });
    }

    #[test]
    fn alt_27_sgr_channel_input_is_sent_and_echoed_as_color() {
        with_offline_app("wa8ded", |app, rx, _| {
            app.active = ActivePane::Channel(1);
            app.compose[1].input = "←[1;31mNot!←[0m normal ←".into();
            app.submit();

            let TncCommand::Session(SessionCommand::Send { channel, data, .. }) =
                rx.try_recv().unwrap()
            else {
                panic!("expected channel text");
            };
            assert_eq!(channel, 1);
            assert_eq!(data, "\x1b[1;31mNot!\x1b[0m normal ←\r".as_bytes());
            assert_eq!(
                app.compose[1].history.back().unwrap(),
                "←[1;31mNot!←[0m normal ←"
            );
            let job = app.pads[1].layout_job_channel(100, false, padlog::PAD_FONT_SIZE);
            assert!(job.sections.iter().any(|section| {
                &job.text[section.byte_range.clone()] == "Not!"
                    && section.format.color == Color32::from_rgb(255, 85, 85)
            }));
            assert!(job.sections.iter().any(|section| {
                job.text[section.byte_range.clone()].contains("normal")
                    && section.format.color == PadKind::Own.color()
            }));
        });
    }

    #[test]
    fn alt_27_sgr_channel_input_still_sends_escape_when_color_is_disabled() {
        with_offline_app("wa8ded", |app, rx, _| {
            app.active = ActivePane::Channel(1);
            app.cfg.session.allow_color_ansi = false;
            app.compose[1].input = "←[1;31mNot!".into();
            app.submit();

            let TncCommand::Session(SessionCommand::Send { data, .. }) = rx.try_recv().unwrap()
            else {
                panic!("expected channel text");
            };
            assert_eq!(data, b"\x1b[1;31mNot!\r");
            let job = app.pads[1].layout_job_channel(100, false, padlog::PAD_FONT_SIZE);
            assert!(job.sections.iter().any(|section| {
                &job.text[section.byte_range.clone()] == "Not!"
                    && section.format.color == PadKind::Own.color()
            }));
        });
    }

    #[test]
    fn sgr_received_channel_colors_across_packet_cuts() {
        with_offline_app("wa8ded", |app, _, _| {
            app.accept_rx_text(1, "\x1b[3", PadKind::Incoming);
            app.accept_rx_text(1, "1mGrr\x1b[0m plain\r", PadKind::Incoming);
            let job = app.pads[1].layout_job_channel(100, false, padlog::PAD_FONT_SIZE);
            assert!(job.sections.iter().any(|section| {
                job.text[section.byte_range.clone()].contains("Grr")
                    && section.format.color == Color32::from_rgb(170, 0, 0)
            }));
            assert!(job.sections.iter().any(|section| {
                job.text[section.byte_range.clone()].contains("plain")
                    && section.format.color == PadKind::Incoming.color()
            }));
        });
    }

    #[test]
    fn alt_27_sgr_monitor_input_sends_escape_and_colors_echo() {
        with_offline_app("wa8ded", |app, rx, _| {
            app.active = ActivePane::Monitor;
            app.compose[0].input = "←[31mGrr and Stuff".into();
            app.submit();

            let TncCommand::Session(SessionCommand::Send {
                channel,
                data,
                id: None,
            }) = rx.try_recv().unwrap()
            else {
                panic!("expected monitor text");
            };
            assert_eq!(channel, 0);
            assert_eq!(data, b"\x1b[31mGrr and Stuff\r");
            let job = app.pads[0].layout_job_monitor(
                100,
                Color32::BLUE,
                Color32::WHITE,
                Color32::YELLOW,
                padlog::PAD_FONT_SIZE,
            );
            assert!(job.sections.iter().any(|section| {
                job.text[section.byte_range.clone()].contains("Grr and Stuff")
                    && section.format.color == Color32::from_rgb(170, 0, 0)
            }));
        });
    }

    #[test]
    fn sgr_monitor_received_host_and_kiss_payloads_keep_other_controls() {
        for protocol in ["wa8ded", "kiss"] {
            with_offline_app(protocol, |app, _, _| {
                let payload = b"\x1b[31mRed\x1b[1;32mGreen\x1b[0m plain\x07\x1b[2J";
                if protocol == "wa8ded" {
                    let (tx, rx) = mpsc::channel(8);
                    app.events = rx;
                    tx.try_send(TncEvent::HostFrame {
                        channel: 0,
                        code: HostCode::MonitorHeader,
                        payload: b"fm N1AAA to N1BBB".to_vec(),
                    })
                    .unwrap();
                    tx.try_send(TncEvent::HostFrame {
                        channel: 0,
                        code: HostCode::MonitorData,
                        payload: payload.to_vec(),
                    })
                    .unwrap();
                    app.drain_tnc();
                } else {
                    let raw = ax25::build_ui("N1BBB", "N1AAA", &[], payload);
                    app.kiss_show_transit(&ax25::parse_frame(&raw).unwrap(), 0);
                }
                let payload_color = Color32::from_rgb(3, 4, 5);
                let job = app.pads[0].layout_job_monitor(
                    100,
                    Color32::BLUE,
                    payload_color,
                    Color32::YELLOW,
                    padlog::PAD_FONT_SIZE,
                );
                for (word, color) in [
                    ("Red", Color32::from_rgb(170, 0, 0)),
                    ("Green", Color32::from_rgb(85, 255, 85)),
                    ("plain", payload_color),
                ] {
                    assert!(
                        job.sections.iter().any(|section| {
                            job.text[section.byte_range.clone()].contains(word)
                                && section.format.color == color
                        }),
                        "{protocol}: missing {word} in {job:?}"
                    );
                }
                assert!(job.text.contains("<0x07>"));
                assert!(job.text.contains("<0x1b>[2J"));
            });
        }
    }

    #[test]
    fn sgr_monitor_disabled_strips_colors_without_changing_other_bytes() {
        with_offline_app("kiss", |app, _, _| {
            app.cfg.session.allow_color_ansi = false;
            let raw = ax25::build_ui("N1BBB", "N1AAA", &[], b"\x1b[31mRed\x07");
            app.kiss_show_transit(&ax25::parse_frame(&raw).unwrap(), 0);
            let payload_color = Color32::from_rgb(3, 4, 5);
            let job = app.pads[0].layout_job_monitor(
                100,
                Color32::BLUE,
                payload_color,
                Color32::YELLOW,
                padlog::PAD_FONT_SIZE,
            );
            assert!(job.sections.iter().any(|section| {
                job.text[section.byte_range.clone()].contains("Red")
                    && section.format.color == payload_color
            }));
            assert!(job.text.contains("<0x07>"));
            assert!(!job.text.contains("<0x1b>"));
        });
    }

    #[test]
    fn disconnect_icon_cancels_pending_kiss_connect_without_retries() {
        with_offline_app("kiss", |app, rx, ctx| {
            let via = vec!["WIDE1-1".into()];
            app.begin_connect(1, "N1WRNG", &via, Vec::new());
            assert!(!app.channel_linked(1));
            assert!(app.channel_connecting[1].is_some());
            while rx.try_recv().is_ok() {}

            click_first_channel_icon(app, ctx);
            let TncCommand::Session(SessionCommand::Disconnect { channel }) =
                rx.try_recv().unwrap()
            else {
                panic!("expected session cancellation");
            };
            assert_eq!(channel, 1);
            assert!(!app.channel_connected(1));
            assert!(!app.channel_disconnecting(1));
            while rx.try_recv().is_ok() {}

            click_first_channel_icon(app, ctx);
            assert!(app.session.show_connect_dialog);
        });
    }

    #[test]
    fn disconnect_icon_cancels_pending_hostmode_connect() {
        with_offline_app("wa8ded", |app, rx, ctx| {
            app.begin_connect(1, "N1WRNG", &[], Vec::new());
            while rx.try_recv().is_ok() {}
            click_first_channel_icon(app, ctx);
            let TncCommand::Session(SessionCommand::Disconnect { channel }) =
                rx.try_recv().unwrap()
            else {
                panic!("expected hostmode disconnect");
            };
            assert_eq!(channel, 1);
            assert!(!app.channel_connected(1));
        });
    }

    #[test]
    fn disconnecting_established_kiss_link_still_waits_for_acknowledgment() {
        with_offline_app("kiss", |app, rx, ctx| {
            app.active = ActivePane::Channel(1);
            app.channel_peer[1] = Some("N1PEER".into());
            click_first_channel_icon(app, ctx);
            let TncCommand::Session(SessionCommand::Disconnect { channel }) =
                rx.try_recv().unwrap()
            else {
                panic!("expected session disconnect");
            };
            assert_eq!(channel, 1);
            assert!(app.channel_linked(1));
            assert!(app.channel_disconnecting(1));
        });
    }

    #[test]
    fn emoji_variation_selectors_are_invisible_in_text() {
        let ctx = Context::default();
        install_console_font(&ctx, false);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            for family in [FontFamily::Monospace, FontFamily::Proportional] {
                let font = FontId::new(16.0, family);
                ctx.fonts(|fonts| {
                    for selector in ['\u{fe0e}', '\u{fe0f}'] {
                        assert!(fonts.has_glyph(&font, selector));
                        assert_eq!(fonts.glyph_width(&font, selector), 0.0);
                    }
                    assert!(fonts.glyph_width(&font, '❤') > 0.0);
                });
            }
        });
    }
}
