//! ALT-U / SETUP menu. The rows follow the DOS screenshots and change with
//! the window that was open when the menu started.

use super::layout::ActivePane;

pub const VGA_NAMES: &[&str] = &[
    "Black",
    "Blue",
    "Green",
    "DarkGreen",
    "Cyan",
    "Red",
    "Magenta",
    "Orange",
    "Brown",
    "LightGray",
    "DarkGray",
    "LightBlue",
    "LightGreen",
    "LightCyan",
    "LightRed",
    "LightMagenta",
    "Yellow",
    "White",
];

pub const LANGUAGES: &[&str] = &["EN", "NL", "DL", "F", "PL", "SLO", "OK", "HA"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowId {
    /// Opens the shared colour, zoom, and icon list.
    Theme,
    ColorsInput,
    ColorsMonitor,
    ColorsChannel,
    ColorsEditor,
    ColorsCheck,
    ColorsList,
    ColorsMh,
    ColorsSpy,
    ColorsState,
    ColorsMenu,
    ColorsClock,
    Prompt,
    Remote,
    Backup,
    Echo,
    RxClick,
    ConnectBell,
    Umlaut,
    InputColumns,
    InsertMon,
    Softscroll,
    Beacon,
    Language,
    /// DOS `Zoom`. Off is JetBrains Mono Regular, on is Bold.
    Zoom,
    Icons,
    PacketLen,
    AutoSave,
    WordWrap,
    EditorBak,
    RefChannel,
    InsertFromEditor,
    FileDrawer,
    /// Comma list of monitor substrings that start a spy (`AutoSpy=`).
    AutoSpy,
    /// Comma list of monitor substrings that block AutoSpy (`NoAutoSpy=`).
    SpyAutoIgnore,
    /// `MAXSPY`, 1..=8. Default 1.
    MaxSpy,
    /// MHeard only. Drop stations last heard more than this many days ago.
    /// `0` is off, `365` is a year. The label says days so the number is obvious.
    MheardClean,
    /// MHeard only. Drop every station on the list.
    MheardClear,
    Use,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Yellow row that opens another box.
    Submenu,
    Toggle,
    Number,
    /// Yellow row that runs once.
    Action,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: RowId,
    pub kind: RowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSlot {
    Input,
    /// Monitor text (`monitor_*`), including Info / Error.
    Monitor,
    /// Connected channel text (`connect_*`).
    Channel,
    Editor,
    /// BBS-Menu (`check_*`).
    Check,
    List,
    Mh,
    Spy,
    State,
    Menu,
    /// Monitor clock. Background only (`clock_background`).
    Clock,
}

impl ColorSlot {
    pub fn title(self) -> &'static str {
        match self {
            Self::Input => "Colours of Input window",
            Self::Monitor => "Colours of Monitor window",
            Self::Channel => "Colours of Channel window",
            Self::Editor => "Colours of Editor window",
            Self::Check => "Colours of BBS window",
            Self::List => "Colours of Box-List window",
            Self::Mh => "Colours of MHeard window",
            Self::Spy => "Colours of Spy window",
            Self::State => "Colours of State window",
            Self::Menu => "Colours of Menu window",
            Self::Clock => "Colours of Clock",
        }
    }
}

impl RowId {
    pub fn label(self) -> &'static str {
        if let Some(title) = self.color_slot().map(ColorSlot::title) {
            return title;
        }
        match self {
            Self::Theme => "Theme Setup",
            Self::Prompt => "Remote control prompt",
            Self::Remote => "Remote control",
            Self::Backup => "Backup text buffers",
            Self::Echo => "Echo",
            Self::RxClick => "RX-Click",
            Self::ConnectBell => "Connect Bell",
            Self::Umlaut => "Umlaut conversion",
            Self::InputColumns => "Input columns",
            Self::InsertMon => "InsertMonLine",
            Self::Softscroll => "Softscroll",
            Self::Beacon => "Beacon parameters",
            Self::Language => "System language",
            Self::Zoom => "Zoom",
            Self::Icons => "System Icons",
            Self::PacketLen => "Packet length",
            Self::AutoSave => "Code-AutoSave",
            Self::WordWrap => "Word wrapping",
            Self::EditorBak => "Create *.BAK File",
            Self::RefChannel => "Reference channel",
            Self::InsertFromEditor => "Insert list from editor",
            Self::FileDrawer => "File drawer",
            Self::AutoSpy => "Auto Spy",
            Self::SpyAutoIgnore => "Spy Auto Ignore",
            Self::MaxSpy => "Number of spies",
            Self::MheardClean => "Clear MHeard older than (days)",
            Self::MheardClear => "Clear MHeard List",
            Self::Use => "Use these values",
            Self::ColorsInput
            | Self::ColorsMonitor
            | Self::ColorsChannel
            | Self::ColorsEditor
            | Self::ColorsCheck
            | Self::ColorsList
            | Self::ColorsMh
            | Self::ColorsSpy
            | Self::ColorsState
            | Self::ColorsMenu
            | Self::ColorsClock => "",
        }
    }

    pub fn color_slot(self) -> Option<ColorSlot> {
        match self {
            Self::ColorsInput => Some(ColorSlot::Input),
            Self::ColorsMonitor => Some(ColorSlot::Monitor),
            Self::ColorsChannel => Some(ColorSlot::Channel),
            Self::ColorsEditor => Some(ColorSlot::Editor),
            Self::ColorsCheck => Some(ColorSlot::Check),
            Self::ColorsList => Some(ColorSlot::List),
            Self::ColorsMh => Some(ColorSlot::Mh),
            Self::ColorsSpy => Some(ColorSlot::Spy),
            Self::ColorsState => Some(ColorSlot::State),
            Self::ColorsMenu => Some(ColorSlot::Menu),
            Self::ColorsClock => Some(ColorSlot::Clock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditKind {
    Prompt,
    BeaconAddress,
    BeaconText,
    FileDrawer,
    AutoSpy,
    SpyAutoIgnore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Main,
    /// Colours, zoom, and icons. The same list from every window.
    Theme {
        cursor: usize,
    },
    Colors {
        slot: ColorSlot,
        cursor: usize,
        draft: [String; 3],
        saved: [String; 3],
        /// Monitor window only. Info / Error (`monitor_infoerror`).
        info: Option<String>,
        /// Row to highlight again when this box closes.
        theme_cursor: usize,
    },
    Beacon {
        cursor: usize,
        /// Minutes as typed. `0` is off, `1440` is once a day.
        period: String,
    },
    Edit {
        kind: EditKind,
        text: String,
        parent_beacon: bool,
    },
}

pub fn next_name(names: &[&str], current: &str, step: i32) -> String {
    if names.is_empty() {
        return current.to_string();
    }
    let idx = names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    let len = names.len() as i32;
    let next = (idx as i32 + step).rem_euclid(len) as usize;
    names[next].to_string()
}

/// Colours, zoom, and icons. Identical from Monitor, Spy, MHeard, and the rest.
pub fn theme_rows() -> Vec<Row> {
    rows(&[
        (RowId::ColorsInput, RowKind::Submenu),
        (RowId::ColorsMonitor, RowKind::Submenu),
        (RowId::ColorsChannel, RowKind::Submenu),
        (RowId::ColorsEditor, RowKind::Submenu),
        (RowId::ColorsCheck, RowKind::Submenu),
        (RowId::ColorsList, RowKind::Submenu),
        (RowId::ColorsMh, RowKind::Submenu),
        (RowId::ColorsSpy, RowKind::Submenu),
        (RowId::ColorsState, RowKind::Submenu),
        (RowId::ColorsMenu, RowKind::Submenu),
        (RowId::ColorsClock, RowKind::Submenu),
        (RowId::Zoom, RowKind::Toggle),
        (RowId::Icons, RowKind::Submenu),
    ])
}

pub fn rows_for(pane: ActivePane) -> Vec<Row> {
    match pane {
        ActivePane::Channel(_) => rows(&[
            (RowId::Theme, RowKind::Submenu),
            (RowId::Prompt, RowKind::Submenu),
            (RowId::Remote, RowKind::Toggle),
            (RowId::Backup, RowKind::Toggle),
            (RowId::Echo, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::ConnectBell, RowKind::Number),
            (RowId::Umlaut, RowKind::Toggle),
            (RowId::InputColumns, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::PacketLen, RowKind::Number),
            (RowId::AutoSave, RowKind::Toggle),
            (RowId::Language, RowKind::Number),
            (RowId::Use, RowKind::Exit),
        ]),
        ActivePane::Editor => rows(&[
            (RowId::Theme, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::EditorBak, RowKind::Toggle),
            (RowId::WordWrap, RowKind::Number),
            (RowId::Language, RowKind::Number),
            (RowId::Use, RowKind::Exit),
        ]),
        ActivePane::Check => list_rows(false),
        ActivePane::List => list_rows(true),
        ActivePane::Mh => rows(&[
            (RowId::Theme, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::Language, RowKind::Number),
            (RowId::MheardClean, RowKind::Number),
            (RowId::MheardClear, RowKind::Action),
            (RowId::Use, RowKind::Exit),
        ]),
        ActivePane::Spy(_) => rows(&[
            (RowId::Theme, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::MaxSpy, RowKind::Number),
            (RowId::AutoSpy, RowKind::Submenu),
            (RowId::SpyAutoIgnore, RowKind::Submenu),
            (RowId::Language, RowKind::Number),
            (RowId::Use, RowKind::Exit),
        ]),
        _ => rows(&[
            (RowId::Theme, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::Echo, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Umlaut, RowKind::Toggle),
            (RowId::InputColumns, RowKind::Number),
            (RowId::InsertMon, RowKind::Toggle),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::Beacon, RowKind::Submenu),
            (RowId::Language, RowKind::Number),
            (RowId::Use, RowKind::Exit),
        ]),
    }
}

fn list_rows(file_drawer: bool) -> Vec<Row> {
    let mut items = vec![
        (RowId::Theme, RowKind::Submenu),
        (RowId::Backup, RowKind::Toggle),
        (RowId::RxClick, RowKind::Number),
        (RowId::Softscroll, RowKind::Toggle),
        (RowId::RefChannel, RowKind::Number),
    ];
    if file_drawer {
        items.push((RowId::FileDrawer, RowKind::Submenu));
    }
    items.push((RowId::InsertFromEditor, RowKind::Action));
    items.push((RowId::Language, RowKind::Number));
    items.push((RowId::Use, RowKind::Exit));
    rows(&items)
}

fn rows(items: &[(RowId, RowKind)]) -> Vec<Row> {
    items
        .iter()
        .copied()
        .map(|(id, kind)| Row { id, kind })
        .collect()
}
