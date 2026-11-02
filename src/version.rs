//! Product identity — single place for UI banners, clock badge, and window title.

/// Short product name (Monitor banner / window).
pub const APP_NAME: &str = "Graphic Packet NG";

/// Operator-facing version (e.g. clock `GP V2.01`, banner `v 2.01`).
pub const APP_VERSION: &str = "2.01";

/// Clock / compact badge text.
pub fn version_badge() -> String {
    format!("GP V{APP_VERSION}")
}

/// Startup Monitor line: `Graphic Packet NG v 2.01- MYCALL Set to CALL`.
pub fn startup_banner(mycall: &str) -> String {
    format!("{APP_NAME} v {APP_VERSION}- MYCALL Set to {mycall}")
}

/// Window / taskbar title.
pub fn window_title() -> String {
    format!("{APP_NAME} v{APP_VERSION}")
}
