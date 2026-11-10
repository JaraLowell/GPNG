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
    ColorsInput,
    ColorsOutput,
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
    /// MHeard only. Days before a station is dropped. `0` is off, `365` is a year.
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
    Output,
    State,
    Menu,
    /// Monitor clock column. Background only (`clock_background`).
    Clock,
}

impl RowId {
    pub fn label(self) -> &'static str {
        match self {
            Self::ColorsInput => "Colours of input window",
            Self::ColorsOutput => "Colours of output window",
            Self::ColorsState => "Colours of state window",
            Self::ColorsMenu => "Colours of menu window",
            Self::ColorsClock => "Colours of Clock",
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
            Self::MheardClean => "Auto Clean",
            Self::MheardClear => "Clear MHeard List",
            Self::Use => "Use these values",
        }
    }

    pub fn color_slot(self) -> Option<ColorSlot> {
        match self {
            Self::ColorsInput => Some(ColorSlot::Input),
            Self::ColorsOutput => Some(ColorSlot::Output),
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
    Colors {
        slot: ColorSlot,
        cursor: usize,
        draft: [String; 3],
        saved: [String; 3],
        /// Monitor output only. Info / Error (`monitor_infoerror`).
        info: Option<String>,
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

pub fn rows_for(pane: ActivePane) -> Vec<Row> {
    match pane {
        ActivePane::Channel(_) => rows(&[
            (RowId::ColorsInput, RowKind::Submenu),
            (RowId::ColorsOutput, RowKind::Submenu),
            (RowId::ColorsState, RowKind::Submenu),
            (RowId::ColorsMenu, RowKind::Submenu),
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
            (RowId::Zoom, RowKind::Toggle),
            (RowId::Icons, RowKind::Submenu),
            (RowId::Use, RowKind::Exit),
        ]),
        ActivePane::Editor => rows(&[
            (RowId::ColorsOutput, RowKind::Submenu),
            (RowId::ColorsState, RowKind::Submenu),
            (RowId::ColorsMenu, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::EditorBak, RowKind::Toggle),
            (RowId::WordWrap, RowKind::Number),
            (RowId::Language, RowKind::Number),
            (RowId::Zoom, RowKind::Toggle),
            (RowId::Icons, RowKind::Submenu),
            (RowId::Use, RowKind::Exit),
        ]),
        ActivePane::Check => list_rows(false),
        ActivePane::List => list_rows(true),
        ActivePane::Mh => rows(&[
            (RowId::ColorsOutput, RowKind::Submenu),
            (RowId::ColorsState, RowKind::Submenu),
            (RowId::ColorsMenu, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::Language, RowKind::Number),
            (RowId::Zoom, RowKind::Toggle),
            (RowId::Icons, RowKind::Submenu),
            (RowId::MheardClean, RowKind::Number),
            (RowId::MheardClear, RowKind::Action),
            (RowId::Use, RowKind::Exit),
        ]),
        ActivePane::Spy(_) => rows(&[
            (RowId::ColorsOutput, RowKind::Submenu),
            (RowId::ColorsState, RowKind::Submenu),
            (RowId::ColorsMenu, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::MaxSpy, RowKind::Number),
            (RowId::AutoSpy, RowKind::Submenu),
            (RowId::SpyAutoIgnore, RowKind::Submenu),
            (RowId::Language, RowKind::Number),
            (RowId::Zoom, RowKind::Toggle),
            (RowId::Icons, RowKind::Submenu),
            (RowId::Use, RowKind::Exit),
        ]),
        _ => rows(&[
            (RowId::ColorsInput, RowKind::Submenu),
            (RowId::ColorsOutput, RowKind::Submenu),
            (RowId::ColorsState, RowKind::Submenu),
            (RowId::ColorsMenu, RowKind::Submenu),
            (RowId::ColorsClock, RowKind::Submenu),
            (RowId::Backup, RowKind::Toggle),
            (RowId::Echo, RowKind::Toggle),
            (RowId::RxClick, RowKind::Number),
            (RowId::Umlaut, RowKind::Toggle),
            (RowId::InputColumns, RowKind::Number),
            (RowId::InsertMon, RowKind::Toggle),
            (RowId::Softscroll, RowKind::Toggle),
            (RowId::Beacon, RowKind::Submenu),
            (RowId::Language, RowKind::Number),
            (RowId::Zoom, RowKind::Toggle),
            (RowId::Icons, RowKind::Submenu),
            (RowId::Use, RowKind::Exit),
        ]),
    }
}

fn list_rows(file_drawer: bool) -> Vec<Row> {
    let mut items = vec![
        (RowId::ColorsOutput, RowKind::Submenu),
        (RowId::ColorsState, RowKind::Submenu),
        (RowId::ColorsMenu, RowKind::Submenu),
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
    items.push((RowId::Zoom, RowKind::Toggle));
    items.push((RowId::Icons, RowKind::Submenu));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_has_beacon_and_channel_has_packet_length() {
        let mon = rows_for(ActivePane::Monitor);
        let lang = mon.iter().position(|r| r.id == RowId::Language).unwrap();
        assert_eq!(mon[lang + 1].id, RowId::Zoom);
        assert_eq!(mon[lang + 1].kind, RowKind::Toggle);
        assert_eq!(mon[lang + 2].id, RowId::Icons);
        for pane in [
            ActivePane::Monitor,
            ActivePane::Channel(1),
            ActivePane::Editor,
            ActivePane::Check,
            ActivePane::List,
            ActivePane::Mh,
            ActivePane::Spy(1),
        ] {
            let rows = rows_for(pane);
            let lang = rows.iter().position(|r| r.id == RowId::Language).unwrap();
            assert_eq!(rows[lang + 1].id, RowId::Zoom, "{pane:?}");
        }
        assert!(mon.iter().any(|r| r.id == RowId::ColorsClock));
        assert!(!rows_for(ActivePane::Channel(1))
            .iter()
            .any(|r| r.id == RowId::ColorsClock));
        assert!(mon.iter().any(|r| r.id == RowId::Beacon));
        assert!(mon.iter().any(|r| r.id == RowId::InsertMon));
        let ch = rows_for(ActivePane::Channel(1));
        assert!(ch.iter().any(|r| r.id == RowId::ConnectBell));
        assert!(!mon.iter().any(|r| r.id == RowId::ConnectBell));
        assert!(ch.iter().any(|r| r.id == RowId::PacketLen));
        assert!(ch.iter().any(|r| r.id == RowId::AutoSave));
        assert!(!ch.iter().any(|r| r.id == RowId::Beacon));
    }

    #[test]
    fn editor_and_lists_drop_the_input_colours() {
        assert!(!rows_for(ActivePane::Editor)
            .iter()
            .any(|r| r.id == RowId::ColorsInput));
        assert!(rows_for(ActivePane::List)
            .iter()
            .any(|r| r.id == RowId::FileDrawer));
        assert!(!rows_for(ActivePane::Check)
            .iter()
            .any(|r| r.id == RowId::FileDrawer));
    }

    #[test]
    fn spy_setup_has_auto_spy_rows_mheard_does_not() {
        let spy = rows_for(ActivePane::Spy(1));
        assert!(spy.iter().any(|r| r.id == RowId::MaxSpy));
        assert!(!rows_for(ActivePane::Monitor)
            .iter()
            .any(|r| r.id == RowId::MaxSpy));
        assert!(spy.iter().any(|r| r.id == RowId::AutoSpy));
        assert!(spy.iter().any(|r| r.id == RowId::SpyAutoIgnore));
        let mh = rows_for(ActivePane::Mh);
        assert!(!mh.iter().any(|r| r.id == RowId::AutoSpy));
        assert!(!mh.iter().any(|r| r.id == RowId::SpyAutoIgnore));
    }

    #[test]
    fn mheard_setup_ends_with_clean_and_clear() {
        let mh = rows_for(ActivePane::Mh);
        let use_at = mh.iter().position(|r| r.id == RowId::Use).unwrap();
        assert_eq!(mh[use_at - 2].id, RowId::MheardClean);
        assert_eq!(mh[use_at - 2].kind, RowKind::Number);
        assert_eq!(mh[use_at - 1].id, RowId::MheardClear);
        assert_eq!(mh[use_at - 1].kind, RowKind::Action);
        assert!(!rows_for(ActivePane::Monitor)
            .iter()
            .any(|r| r.id == RowId::MheardClean || r.id == RowId::MheardClear));
        assert!(!rows_for(ActivePane::Spy(1))
            .iter()
            .any(|r| r.id == RowId::MheardClean || r.id == RowId::MheardClear));
    }

    #[test]
    fn colour_names_wrap() {
        assert_eq!(next_name(VGA_NAMES, "White", 1), "Black");
        assert_eq!(next_name(VGA_NAMES, "Black", -1), "White");
    }
}
