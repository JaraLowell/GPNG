//! Decoder for classic `ICONS.GP` (Graphic Packet ≥ 1.50 icon library).
//!
//! Format documented in `docs/ICONS.md`. Stock libraries hold seventeen
//! 70×40 VGA icons in Borland-style row-interleaved 4-bitplane form.

use std::fs;
use std::path::Path;

use thiserror::Error;

/// Standard VGA / EGA 16-colour palette (6-bit style expanded to 8-bit).
#[allow(dead_code)] // used by PNG exporters / future pixel UI
pub const VGA16_RGB: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (0, 0, 170),
    (0, 170, 0),
    (0, 170, 170),
    (170, 0, 0),
    (170, 0, 170),
    (170, 85, 0),
    (170, 170, 170),
    (85, 85, 85),
    (85, 85, 255),
    (85, 255, 85),
    (85, 255, 255),
    (255, 85, 85),
    (255, 85, 255),
    (255, 255, 85),
    (255, 255, 255),
];

pub const ICON_WIDTH: usize = 70;
pub const ICON_HEIGHT: usize = 40;
const ICON_RECORD: usize = 1482;
const ICON_PAYLOAD: usize = 4 + ICON_HEIGHT * 9 * 4; // header + planes
const DATA_BASE: usize = 0x82; // 130 decimal — after banner + u16 fingerprint

#[derive(Debug, Error)]
pub enum IconError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an ICONS.GP library: {0}")]
    Format(&'static str),
}

/// One decoded icon as VGA colour indices `0..15`, row-major.
#[derive(Debug, Clone)]
pub struct GpIcon {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>, // len = width*height
}

impl GpIcon {
    pub fn rgb_pixels(&self) -> Vec<(u8, u8, u8)> {
        self.pixels
            .iter()
            .map(|&i| VGA16_RGB[(i & 15) as usize])
            .collect()
    }

    /// RGBA8 buffer for egui / GPU upload.
    pub fn rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for &idx in &self.pixels {
            let (r, g, b) = VGA16_RGB[(idx & 15) as usize];
            out.extend_from_slice(&[r, g, b, 255]);
        }
        out
    }
}

/// Full icon library.
#[derive(Debug, Clone)]
pub struct IconLibrary {
    pub fingerprint: u16,
    pub icons: Vec<GpIcon>,
}

impl IconLibrary {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, IconError> {
        Self::parse(&fs::read(path)?)
    }

    pub fn parse(raw: &[u8]) -> Result<Self, IconError> {
        if raw.len() < DATA_BASE + ICON_RECORD {
            return Err(IconError::Format("file too small"));
        }
        if !raw.starts_with(b"Graphic Packet  Icon Libary") {
            return Err(IconError::Format("missing banner"));
        }
        let fingerprint = u16::from_le_bytes([raw[0x80], raw[0x81]]);
        let available = (raw.len() - DATA_BASE) / ICON_RECORD;
        if available == 0 {
            return Err(IconError::Format("no icon records"));
        }
        let mut icons = Vec::with_capacity(available);
        for i in 0..available {
            let off = DATA_BASE + i * ICON_RECORD;
            icons.push(decode_record(&raw[off..off + ICON_RECORD])?);
        }
        Ok(Self {
            fingerprint,
            icons,
        })
    }
}

fn decode_record(chunk: &[u8]) -> Result<GpIcon, IconError> {
    if chunk.len() < ICON_PAYLOAD {
        return Err(IconError::Format("truncated icon record"));
    }
    let width = u16::from_le_bytes([chunk[0], chunk[1]]) as usize;
    let height = u16::from_le_bytes([chunk[2], chunk[3]]) as usize;
    if width == 0 || height == 0 || width > 128 || height > 128 {
        return Err(IconError::Format("implausible icon dimensions"));
    }
    let bpr = (width + 7) / 8;
    let need = 4 + height * bpr * 4;
    if chunk.len() < need {
        return Err(IconError::Format("icon pixel data truncated"));
    }
    let data = &chunk[4..need];
    let mut pixels = vec![0u8; width * height];
    // GP/GP-Paint stores 4 bitplanes per row as plane0..plane3 in file order, but
    // plane0 is the *MSB* (bit 3). Treating plane0 as LSB swapped LightGray↔Yellow
    // (and other pairs) vs DOSBox / GPPAINT. Verified against V1.63 screenshots.
    for y in 0..height {
        let row = &data[y * bpr * 4..(y + 1) * bpr * 4];
        for plane in 0..4 {
            let plane_bytes = &row[plane * bpr..(plane + 1) * bpr];
            let color_bit = 3 - plane;
            for x in 0..width {
                let bit = 7 - (x % 8);
                if plane_bytes[x / 8] & (1 << bit) != 0 {
                    pixels[y * width + x] |= 1 << color_bit;
                }
            }
        }
    }
    Ok(GpIcon {
        width,
        height,
        pixels,
    })
}

/// Labels drawn into `ICONS.GP` slots 0..16.
pub const ICON_SLOT_LABELS: [&str; 17] = [
    "CONNECT",    // 0
    "DISCONNECT", // 1
    "DOS",        // 2
    "EDIT",       // 3
    "EXIT",       // 4
    "LOAD",       // 5
    "MONITOR",    // 6
    "MYCALL",     // 7
    "EMPTY",      // 8 — blank grey chrome
    "SAVE",       // 9
    "SETUP",      // 10
    "FILETRANS",  // 11
    "MHEARD",     // 12
    "SORT",       // 13
    "SPY",        // 14
    "KILL SPY",   // 15
    "FIND",       // 16
];

/// Monitor strip (V1.63 screenshot).
pub const TOOLBAR_MONITOR: &[usize] = &[12, 3, 10, 6, 2, 9, 7, 4];
/// Channel / connect strip (idle).
pub const TOOLBAR_CHANNEL: &[usize] = &[0, 3, 10, 6, 2, 9, 7, 4];
/// Channel strip while linked — DISCONNECT replaces CONNECT.
pub const TOOLBAR_CHANNEL_DISC: &[usize] = &[1, 3, 10, 6, 2, 9, 7, 4];
/// MHeard strip — SPY + CONNECT (connect selected call on first free channel).
pub const TOOLBAR_MHEARD: &[usize] = &[12, 3, 10, 6, 2, 14, 0, 4];
/// Spy strip — SPY sets call, KILL SPY clears it.
pub const TOOLBAR_SPY: &[usize] = &[12, 3, 10, 6, 2, 14, 15, 4];
/// Editor strip.
pub const TOOLBAR_EDITOR: &[usize] = &[12, 3, 10, 6, 2, 9, 5, 4];
/// BBS-Menu strip.
pub const TOOLBAR_BBS_MENU: &[usize] = &[12, 3, 10, 6, 2, 13, 16, 4];
/// Box-List strip.
pub const TOOLBAR_BOX_LIST: &[usize] = &[12, 3, 10, 6, 2, 8, 16, 4];

/// Default / Monitor labels (kept for TUI).
pub const TOOLBAR_LABELS: &[&str] = &[
    "MHEARD", "EDIT", "SETUP", "MONITOR", "DOS", "SAVE", "MYCALL", "EXIT",
];

pub fn toolbar_for(pane: super::layout::ActivePane) -> &'static [usize] {
    use super::layout::ActivePane::*;
    match pane {
        Monitor => TOOLBAR_MONITOR,
        Channel(_) => TOOLBAR_CHANNEL,
        Mh => TOOLBAR_MHEARD,
        Editor => TOOLBAR_EDITOR,
        Check => TOOLBAR_BBS_MENU,
        List => TOOLBAR_BOX_LIST,
        Spy(_) => TOOLBAR_SPY,
    }
}

pub fn toolbar_for_channel(linked: bool) -> &'static [usize] {
    if linked {
        TOOLBAR_CHANNEL_DISC
    } else {
        TOOLBAR_CHANNEL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_stock_library_if_present() {
        let path = crate::paths::data_file("ICONS.GP");
        if !path.is_file() {
            return;
        }
        let lib = IconLibrary::load(&path).expect("data/ICONS.GP");
        assert_eq!(lib.icons.len(), 17);
        assert_eq!(lib.icons[0].width, ICON_WIDTH);
        assert_eq!(lib.icons[0].height, ICON_HEIGHT);
        // Icons use more than a flat colour.
        let uniq: std::collections::HashSet<_> = lib.icons[0].pixels.iter().copied().collect();
        assert!(uniq.len() > 1);
        // MSB-first planes: paper is LightGray (7), not Yellow (14).
        assert_eq!(lib.icons[6].pixels[0] & 15, 7, "MONITOR chrome should be LightGray");
    }
}
