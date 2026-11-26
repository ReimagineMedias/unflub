//! Text the Rust side shows: the tray menu and tooltip. English only for now; the settings
//! window's strings are in `app/src/i18n/en.ts`.

pub const TOOLTIP: &str = "unflub";
pub const TOOLTIP_PAUSED: &str = "unflub (paused)";
/// The engine paused itself: its own input stopped coming back (`InputInterference`).
pub const TOOLTIP_INTERFERENCE: &str =
    "unflub paused itself: another app is interfering with keyboard input";
/// Autocorrect has no working engine session; the supervisor keeps trying.
pub const TOOLTIP_HOOK_UNAVAILABLE: &str =
    "unflub can't watch the keyboard right now and keeps trying";

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

/// The manifest's message, as a calm row that opens About. Long text is cut at a word.
pub fn menu_message(text: &str) -> String {
    const MAX: usize = 60;
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_messages_are_cut_at_a_word() {
        assert_eq!(
            menu_message(" Update when you can. "),
            "Update when you can."
        );
        let long = "0.3.1 fixes a crash in Slack and a slow start on older laptops, so update when you can.";
        let row = menu_message(long);
        assert!(row.chars().count() <= 61, "{row}");
        assert!(row.ends_with('…'));
        assert!(long.starts_with(row.trim_end_matches('…')));
    }
}
