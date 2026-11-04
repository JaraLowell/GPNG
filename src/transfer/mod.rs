//! File-transfer paths compatible with classic Graphic Packet.
//!
//! - [`autobin`] — `#BIN#` binary protocol (SP / THP / DieBox / GP)
//! - [`sevenplus`] — 7Plus Code-AutoSave sniffer (`go_7+.` / `stop_7+.`)
//! - [`sevenplus_codec`] — 7PLUS 2.2 encode/decode (Rust port, own licence)
//!
//! See `docs/TRANSFER.md`. **7Plus ≠ 7-Zip.**

pub mod autobin;
pub mod crc;
pub mod sevenplus;
pub mod sevenplus_codec;
