//! Product identity — single place for UI banners, clock badge, and window title.

/// Short product name (Monitor banner / window).
pub const APP_NAME: &str = "Graphic Packet NG";

/// Operator-facing version (e.g. clock `GP V2.01`, banner `v2.01`).
pub const APP_VERSION: &str = "2.01";

/// Modern authors shown on the Monitor startup credits line.
pub const APP_AUTHORS: &str = "MichTronics (PD4MV) and Jara Lowel";

/// Classic GP attribution on the credits line.
pub const CLASSIC_CREDIT: &str = "1994 Graphic Packet (GP),
 the classic MS-Dos AX.25 terminal by Ulf Saran (DH1DAE)";

/// Clock / compact badge text.
pub fn version_badge() -> String {
    format!("GP V{APP_VERSION}")
}

/// Credits on the startup screen (two lines; the break is in `CLASSIC_CREDIT`).
pub fn startup_credits() -> String {
    format!("{APP_NAME} v{APP_VERSION} by {APP_AUTHORS}, based on {CLASSIC_CREDIT}")
}

/// Second Monitor startup line (station call from Config).
pub fn startup_local_call(mycall: &str) -> String {
    format!("Local call set to {mycall}")
}

/// Window / taskbar title.
pub fn window_title() -> String {
    format!("{APP_NAME} v{APP_VERSION}")
}
