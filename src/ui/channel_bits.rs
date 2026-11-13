//! Compact ON/OFF masks for session-chrome toggles.
//!
//! One `u32` per setting. Bit index = slot:
//!
//! | Bit | Slot |
//! |-----|------|
//! | 0 | Monitor |
//! | 1..=10 | QSO channels |
//! | 11..=20 | Spy 1..=10 |
//! | 21 | BBS-Menu |
//! | 22 | Box-List |
//!
//! Each setting has an apply mask: only those bits are meaningful. Monitor
//! toggles write every bit in the mask; a channel/spy/BBS/List toggle flips
//! that slot alone.

use super::layout::ActivePane;

/// Monitor pane (bit 0).
pub const SLOT_MONITOR: u8 = 0;
/// First QSO channel bit.
pub const SLOT_CHANNEL_FIRST: u8 = 1;
/// Last QSO channel bit (classic GP max).
pub const SLOT_CHANNEL_LAST: u8 = 10;
/// Spy 1 lives at bit 11 (`10 + spy_number`).
pub const SLOT_SPY_BASE: u8 = 10;
pub const SLOT_BBS: u8 = 21;
pub const SLOT_LIST: u8 = 22;

const fn bit(slot: u8) -> u32 {
    1u32 << slot
}

const fn channel_bits() -> u32 {
    let mut m = 0u32;
    let mut s = SLOT_CHANNEL_FIRST;
    while s <= SLOT_CHANNEL_LAST {
        m |= bit(s);
        s += 1;
    }
    m
}

const fn spy_bits() -> u32 {
    let mut m = 0u32;
    let mut s = 1u8;
    while s <= 10 {
        m |= bit(SLOT_SPY_BASE + s);
        s += 1;
    }
    m
}

/// Bits 1..=10.
pub const MASK_CHANNELS: u32 = channel_bits();
/// Bits 11..=20.
pub const MASK_SPY: u32 = spy_bits();
pub const MASK_MONITOR: u32 = bit(SLOT_MONITOR);
pub const MASK_BBS: u32 = bit(SLOT_BBS);
pub const MASK_LIST: u32 = bit(SLOT_LIST);

/// Softscroll: Monitor + channels + spy + BBS + List.
pub const MASK_SOFTSCROLL: u32 = MASK_MONITOR | MASK_CHANNELS | MASK_SPY | MASK_BBS | MASK_LIST;
/// Umlaut: Monitor + channels + BBS + List.
pub const MASK_UMLAUT: u32 = MASK_MONITOR | MASK_CHANNELS | MASK_BBS | MASK_LIST;
/// Echo: Monitor + channels.
pub const MASK_ECHO: u32 = MASK_MONITOR | MASK_CHANNELS;
/// Bell / 7+ / Remote: QSO channels only.
pub const MASK_BELL: u32 = MASK_CHANNELS;
pub const MASK_SEVEN_PLUS: u32 = MASK_CHANNELS;
pub const MASK_REMOTE: u32 = MASK_CHANNELS;

/// Compact per-slot ON/OFF flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelBits(pub u32);

impl ChannelBits {
    pub const fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub fn get(self, slot: u8) -> bool {
        if slot >= 32 {
            return false;
        }
        self.0 & bit(slot) != 0
    }

    pub fn set(&mut self, slot: u8, on: bool) {
        if slot >= 32 {
            return;
        }
        if on {
            self.0 |= bit(slot);
        } else {
            self.0 &= !bit(slot);
        }
    }

    /// Set or clear every bit in `mask`.
    pub fn set_masked(&mut self, mask: u32, on: bool) {
        if on {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
    }

    /// Keep only bits that belong to this setting.
    pub fn clamp(self, mask: u32) -> Self {
        Self(self.0 & mask)
    }

    /// Monitor (slot 0): set every bit in `mask` to the opposite of the
    /// current masked state (all-on → all-off, otherwise all-on).
    /// Other slots: flip that one bit when it sits in `mask`.
    /// Returns the new ON/OFF value for the active slot (after broadcast,
    /// every masked bit matches).
    pub fn toggle(&mut self, slot: u8, mask: u32) -> bool {
        if slot == SLOT_MONITOR {
            let on = self.0 & mask != mask;
            self.set_masked(mask, on);
            return on;
        }
        if mask & bit(slot) == 0 {
            return self.get(slot);
        }
        let on = !self.get(slot);
        self.set(slot, on);
        on
    }

    /// Lit state for the session bar / setup. Monitor shows all-on for `mask`;
    /// other panes show their own bit (even when that bit is outside `mask`,
    /// so a mis-routed pane still reads false rather than panicking).
    pub fn display(self, pane: ActivePane, mask: u32) -> bool {
        if is_broadcast(pane) {
            mask != 0 && self.0 & mask == mask
        } else {
            self.get(slot_for(pane))
        }
    }
}

/// Slot for reading a per-pane setting. Editor / MHeard use Monitor bit 0.
pub fn slot_for(pane: ActivePane) -> u8 {
    match pane {
        ActivePane::Monitor | ActivePane::Editor | ActivePane::Mh => SLOT_MONITOR,
        ActivePane::Channel(n) => n.clamp(SLOT_CHANNEL_FIRST, SLOT_CHANNEL_LAST),
        ActivePane::Spy(n) => SLOT_SPY_BASE + n.clamp(1, 10),
        ActivePane::Check => SLOT_BBS,
        ActivePane::List => SLOT_LIST,
    }
}

/// True when a toggle on this pane should broadcast across the apply mask.
pub fn is_broadcast(pane: ActivePane) -> bool {
    matches!(pane, ActivePane::Monitor)
}

/// Expand a legacy boolean into a full apply mask (`true` → all bits in mask).
pub fn from_legacy_bool(on: bool, mask: u32) -> ChannelBits {
    ChannelBits(if on { mask } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_toggle_leaves_neighbours() {
        let mut bits = ChannelBits(MASK_CHANNELS);
        assert!(bits.get(10));
        assert!(!bits.toggle(10, MASK_ECHO));
        assert!(!bits.get(10));
        assert!(bits.get(2));
    }

    #[test]
    fn monitor_display_is_all_on_for_channel_only_mask() {
        let mut bits = ChannelBits(0);
        assert!(!bits.display(ActivePane::Monitor, MASK_BELL));
        bits.set_masked(MASK_BELL, true);
        assert!(bits.display(ActivePane::Monitor, MASK_BELL));
        bits.set(3, false);
        assert!(!bits.display(ActivePane::Monitor, MASK_BELL));
        assert!(!bits.display(ActivePane::Channel(3), MASK_BELL));
        assert!(bits.display(ActivePane::Channel(4), MASK_BELL));
    }

    #[test]
    fn monitor_broadcast_sets_every_apply_bit() {
        let mut bits = ChannelBits(0);
        assert!(bits.toggle(SLOT_MONITOR, MASK_ECHO));
        assert_eq!(bits.raw() & MASK_ECHO, MASK_ECHO);
        assert!(!bits.toggle(SLOT_MONITOR, MASK_ECHO));
        assert_eq!(bits.raw() & MASK_ECHO, 0);
    }

    #[test]
    fn seven_plus_mask_ignores_spy() {
        let mut bits = ChannelBits(0);
        bits.toggle(SLOT_MONITOR, MASK_SEVEN_PLUS);
        assert_eq!(bits.raw() & MASK_SPY, 0);
        assert_eq!(bits.raw() & MASK_SEVEN_PLUS, MASK_SEVEN_PLUS);
        bits.toggle(5, MASK_SEVEN_PLUS);
        assert!(!bits.get(5));
        assert!(bits.get(4));
    }

    #[test]
    fn slot_for_maps_panes() {
        assert_eq!(slot_for(ActivePane::Monitor), 0);
        assert_eq!(slot_for(ActivePane::Channel(3)), 3);
        assert_eq!(slot_for(ActivePane::Spy(2)), 12);
        assert_eq!(slot_for(ActivePane::Check), SLOT_BBS);
        assert_eq!(slot_for(ActivePane::List), SLOT_LIST);
        assert_eq!(slot_for(ActivePane::Mh), SLOT_MONITOR);
    }
}
