//! Product identity — single place for UI banners, clock badge, and window title.

/// Short product name (Monitor banner / window).
pub const APP_NAME: &str = "Graphic Packet NG";

/// Operator-facing version (e.g. clock `GP V2.01`, banner `v2.01`).
pub const APP_VERSION: &str = "2.01";

/// Modern authors shown on the Monitor startup credits line.
pub const APP_AUTHORS: &str = "Mitch and Jara Lowel";

/// Classic GP attribution on the credits line.
pub const CLASSIC_CREDIT: &str = "Graphic Packet 1.63b 1994 by Ulf Saran";

/// Clock / compact badge text.
pub fn version_badge() -> String {
    format!("GP V{APP_VERSION}")
}

/// First Monitor startup line (credits).
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
