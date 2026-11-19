//! Classic Graphic Packet remote surface (`//` commands + `%` macros).
//!
//! Command names recovered from historic language packs and `README.TXT`, with
//! light confirmation against strings in `GP2862K.EXE`. Help text lives in
//! `locales/*.toml` via `crate::i18n`. See `docs/COMPATIBILITY.md`.

pub mod commands;
pub mod external;
pub mod gpi;
pub mod macros;
pub mod mail;
pub mod userdir;
