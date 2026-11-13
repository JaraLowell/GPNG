//! Map classic GP VGA colour *names* onto ratatui / ANSI colours.
//!
//! Source of names: `CONFIG.GP` / `README.TXT` (Black, Blue, … White).
//! Original rendering used BGI palette indices; we only need perceptual
//! equivalents for a terminal or future GPU UI.

use ratatui::style::Color;

use crate::config::ColorConfig;

/// Resolved theme for every named pane in classic GP.
#[derive(Debug, Clone)]
pub struct Theme {
    pub menu_fg: Color,
    pub menu_bg: Color,
    pub menu_alert: Color, // MenuInversAttr — unread channel flash
    pub state_fg: Color,
    pub state_bg: Color,
    pub monitor_fg: Color,
    pub monitor_bg: Color,
    pub monitor_header: Color,
    pub connect_fg: Color,
    pub connect_bg: Color,
    pub connect_tx: Color, // ConnectInversAttr — locally sent text
    pub input_fg: Color,
    pub input_bg: Color,
    pub editor_fg: Color,
    pub editor_bg: Color,
    pub editor_cursor: Color,
    pub check_fg: Color,
    pub check_bg: Color,
    pub check_select: Color,
    pub list_fg: Color,
    pub list_bg: Color,
    pub list_select: Color,
    pub mh_fg: Color,
    pub mh_bg: Color,
    pub mh_select: Color,
    pub spy_fg: Color,
    pub spy_bg: Color,
    pub spy_select: Color,
    pub border: Color,
    pub clock_bg: Color,
}

impl Theme {
    pub fn from_config(c: &ColorConfig) -> Self {
        Self {
            menu_fg: vga(&c.menu_foreground),
            menu_bg: vga(&c.menu_background),
            menu_alert: vga(&c.menu_invers_attr),
            state_fg: vga(&c.state_foreground),
            state_bg: vga(&c.state_background),
            monitor_fg: vga(&c.monitor_foreground),
            monitor_bg: vga(&c.monitor_background),
            monitor_header: vga(&c.monitor_invers_attr),
            connect_fg: vga(&c.connect_foreground),
            connect_bg: vga(&c.connect_background),
            connect_tx: vga(&c.connect_invers_attr),
            input_fg: vga(&c.input_foreground),
            input_bg: vga(&c.input_background),
            editor_fg: vga(&c.editor_foreground),
            editor_bg: vga(&c.editor_background),
            editor_cursor: vga(&c.editor_invers_attr),
            check_fg: vga(&c.check_foreground),
            check_bg: vga(&c.check_background),
            check_select: vga(&c.check_invers_attr),
            list_fg: vga(&c.list_foreground),
            list_bg: vga(&c.list_background),
            list_select: vga(&c.list_invers_attr),
            mh_fg: vga(&c.mh_foreground),
            mh_bg: vga(&c.mh_background),
            mh_select: vga(&c.mh_invers_attr),
            spy_fg: vga(&c.spy_foreground),
            spy_bg: vga(&c.spy_background),
            spy_select: vga(&c.spy_invers_attr),
            border: vga(&c.border_attr),
            clock_bg: vga(&c.clock_background),
        }
    }
}

/// Parse a classic GP / Borland-style colour name (case-insensitive, spaces ok).
pub fn vga(name: &str) -> Color {
    let n: String = name
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    match n.as_str() {
        "black" => Color::Black,
        "blue" => Color::Blue,
        "green" => Color::Green,
        "cyan" | "turquoise" | "tuerkis" | "türkis" => Color::Cyan,
        "red" => Color::Red,
        "magenta" | "magentarot" => Color::Magenta,
        "orange" => Color::Rgb(255, 165, 0),
        "brown" | "dunkelbraun" => Color::Rgb(170, 85, 0),
        "darkgreen" | "dunkelgruen" | "dunkelgrün" => Color::Rgb(0, 85, 0),
        "lightgray" | "lightgrey" | "hellgrau" => Color::Gray,
        "darkgray" | "darkgrey" | "dunkelgrau" => Color::DarkGray,
        "lightblue" | "hellblau" => Color::LightBlue,
        "lightgreen" | "hellgruen" | "hellgrün" => Color::LightGreen,
        "lightcyan" | "helltuerkis" | "helltürkis" => Color::LightCyan,
        "lightred" | "hellrot" => Color::LightRed,
        "lightmagenta" => Color::LightMagenta,
        "yellow" | "gelb" => Color::Yellow,
        "white" | "weiss" | "weiß" => Color::White,
        _ => Color::White,
    }
}
