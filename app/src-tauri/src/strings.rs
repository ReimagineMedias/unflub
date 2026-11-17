//! Text the Rust side shows: the tray menu and tooltip. English only for now; the settings
//! window's strings are in `app/src/i18n/en.ts`.

pub const TOOLTIP: &str = "unflub";
pub const TOOLTIP_PAUSED: &str = "unflub (paused)";
/// The engine paused itself: its own input stopped coming back (`InputInterference`).
pub const TOOLTIP_INTERFERENCE: &str =
    "unflub paused itself: another app is interfering with keyboard input";

pub const MENU_AUTOCORRECT: &str = "Autocorrect";
pub const MENU_SETTINGS: &str = "Settings…";
pub const MENU_TOUR: &str = "Take the tour";
pub const MENU_SUPPORT: &str = "Support unflub";
pub const MENU_STAR: &str = "Star on GitHub";
pub const MENU_QUIT: &str = "Quit unflub";
/// A crash report from the last run is waiting for the user's answer.
pub const MENU_CRASH: &str = "unflub closed unexpectedly…";

/// The update is downloaded and verified.
pub fn menu_restart_to_update(version: &str) -> String {
    format!("Restart to update to v{version}")
}

/// A newer version exists but isn't downloaded (automatic updates are off).
pub fn menu_update_available(version: &str, critical: bool) -> String {
    if critical {
        format!("Important update: v{version}…")
    } else {
        format!("Update to v{version}…")
    }
}
