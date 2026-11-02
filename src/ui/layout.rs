//! Logical pane geometry matching the GP V1.63 VGA screenshot (640×480).
//!
//! ```text
//! ┌──────────────────────────── channel grid (2×6) ─────────────┬─ clock ─┐
//! │ 1: …  2: …  3: …  4: …  5: …  6: …                          │ analog  │
//! │ 7: …  8: …  9: … 10: …  BBS-Menu  Box-List                  │ date/ver│
//! ├─────────────────────────────────────────────────────────────┴─────────┤
//! │ MAGENTA title: Monitor │ Mode IUSC │ Buffers │ … │ Global Mycall     │
//! ├───────────────────────────────────────────────────────────────────────┤
//! │                                                                       │
//! │  PRIMARY RX  (Monitor / Connect)                         scrollbar    │
//! │                                                                       │
//! ├─ optional split: Monitor ─────────────────────────────────────────────┤
//! ├─ QRG / CT / Echo / ScrLock status ────────────────────────────────────┤
//! ├─ INPUT (pre-write) ───────────────────────────────────────────────────┤
//! └─ ICONS.GP strip: MHEARD EDIT SETUP MONITOR DOS SAVE MYCALL EXIT ──────┘
//! ```

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use super::icons::TOOLBAR_LABELS;

/// Which primary content surface is focused (mirrors F-keys / ALT-E / ALT-M).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Monitor,
    Channel(u8),
    Editor,
    Check,
    List,
    Mh,
    Spy,
}

impl ActivePane {
    pub fn label(self) -> String {
        match self {
            Self::Monitor => "Monitor".into(),
            Self::Channel(n) => format!("{n}"),
            Self::Editor => "Editor".into(),
            Self::Check => "BBS-Menu".into(),
            Self::List => "Box-List".into(),
            Self::Mh => "MHeard".into(),
            Self::Spy => "Spy".into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SplitState {
    pub enabled: bool,
    pub upper_ratio: f32,
}

impl Default for SplitState {
    fn default() -> Self {
        Self {
            enabled: false,
            upper_ratio: 0.55,
        }
    }
}

impl SplitState {
    pub fn nudge(&mut self, delta: f32) {
        self.upper_ratio = (self.upper_ratio + delta).clamp(0.2, 0.8);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaneRects {
    pub channel_grid: Rect,
    pub clock: Rect,
    pub title: Rect,
    pub primary: Rect,
    pub secondary: Option<Rect>,
    pub session: Rect,
    pub input: Rect,
    pub icons: Rect,
}

/// Allocate regions approximating the VGA composition (terminal cells, not pixels).
pub fn compute(area: Rect, split: SplitState) -> PaneRects {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // channel grid + clock row
            Constraint::Length(1), // magenta Monitor title bar
            Constraint::Min(5),    // RX text
            Constraint::Length(1), // QRG / Echo / ScrLock
            Constraint::Length(3), // pre-write
            Constraint::Length(1), // icon labels (bitmaps → future GUI)
        ])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(14)])
        .split(vertical[0]);

    let (primary, secondary) = if split.enabled {
        let upper = ((vertical[2].height as f32) * split.upper_ratio).round() as u16;
        let upper = upper.clamp(2, vertical[2].height.saturating_sub(2));
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(upper), Constraint::Min(2)])
            .split(vertical[2]);
        (parts[0], Some(parts[1]))
    } else {
        (vertical[2], None)
    };

    PaneRects {
        channel_grid: top[0],
        clock: top[1],
        title: vertical[1],
        primary,
        secondary,
        session: vertical[3],
        input: vertical[4],
        icons: vertical[5],
    }
}

/// Labels for the bottom strip until bitmaps are blitted in a pixel UI.
pub fn toolbar_caption() -> String {
    TOOLBAR_LABELS.join(" │ ")
}
