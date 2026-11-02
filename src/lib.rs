//! Graphic Packet NG — library surface for the async AX.25 terminal.
//!
//! The binary (`main.rs`) is a thin Tokio entrypoint; keep reusable protocol
//! and UI code here so `pub` APIs are not flagged as dead code.

pub mod config;
pub mod remote;
pub mod tnc;
pub mod transfer;
pub mod ui;
pub mod version;

pub use version::{APP_NAME, APP_VERSION};
