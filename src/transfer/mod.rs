//! File-transfer paths compatible with classic Graphic Packet.
//!
//! - [`autobin`] — `#BIN#` binary protocol (SP / THP / DieBox / GP)
//! - [`sevenplus`] — 7Plus Code-AutoSave sniffer (`go_7+.` / `stop_7+.`)
//!
//! See `docs/TRANSFER.md`. **7Plus ≠ 7-Zip.**

pub mod autobin;
pub mod crc;
pub mod sevenplus;
