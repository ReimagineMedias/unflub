//! Session-only pause and the opt-in Windows shortcut.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{
    menu::{MenuItem, Submenu},
    AppHandle, Emitter, Manager, Wry,
};

use crate::{current_settings, queue_ui_refresh, strings, update_settings, AppState};

const EVENT: &str = "pause-controls";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pause {
    #[default]
    Running,
    Until(Instant),
    UntilRestart,
}

impl Pause {
    pub fn active(self) -> bool {
        self != Self::Running
    }

    fn expire(&mut self, now: Instant) -> bool {
        if matches!(self, Self::Until(deadline) if *deadline <= now) {
            *self = Self::Running;
            return true;
        }
        false
    }
}

#[derive(Default)]
pub struct Controls {
    pause: Mutex<Pause>,
    timer: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    #[cfg(windows)]
    registered: Mutex<Option<tauri_plugin_global_shortcut::Shortcut>>,
    pub pause_menu: Mutex<Option<Submenu<Wry>>>,
    pub exclude_item: Mutex<Option<MenuItem<Wry>>>,
    target: Mutex<Option<String>>,
}

#[derive(serde::Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    paused: bool,
    shortcut_unavailable: bool,
}

pub fn paused(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .controls
        .pause
        .lock()
        .unwrap()
        .active()
}

pub fn status(app: &AppHandle) -> Status {
    Status {
        paused: paused(app),
        shortcut_unavailable: shortcut_unavailable(app),
    }
}

pub fn emit(app: &AppHandle) {
    let _ = app.emit(EVENT, status(app));
}

pub fn set_pause(app: &AppHandle, pause: Pause) {
    let state = app.state::<AppState>();
    let mut timer = state.controls.timer.lock().unwrap();
    if let Some(task) = timer.take() {
        task.abort();
    }
    *state.controls.pause.lock().unwrap() = pause;
    if let Pause::Until(deadline) = pause {
        let app = app.clone();
        *timer = Some(tauri::async_runtime::spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            queue_ui_refresh(&app, |app| {
                let state = app.state::<AppState>();
                if state.controls.pause.lock().unwrap().expire(Instant::now()) {
                    crate::sync_correction(app);
                }
            });
        }));
    }
    drop(timer);
    crate::sync_correction(app);
}

pub fn pause_for(app: &AppHandle, minutes: u64) {
    if current_settings(app).enabled {
        set_pause(
            app,
            Pause::Until(Instant::now() + Duration::from_secs(minutes * 60)),
        );
    }
}

pub fn toggle_tray(app: &AppHandle) {
    if paused(app) {
        set_pause(app, Pause::Running);
    } else if update_settings(app, |s| s.enabled = !s.enabled).is_err() {
        // Native check menu items toggle themselves before the callback.
        crate::refresh_tray(app);
    }
}

#[cfg(windows)]
pub fn toggle_shortcut(app: &AppHandle) {
    if current_settings(app).enabled {
        let pause = if paused(app) {
            Pause::Running
        } else {
            Pause::UntilRestart
        };
        set_pause(app, pause);
    }
}

/// An empty binding means off. Require two modifiers with a letter or digit so a shortcut
/// cannot intercept ordinary typing or a single-modifier editing command.
pub fn normalise_shortcut(binding: &str) -> Option<String> {
    if binding.trim().is_empty() {
        return Some(String::new());
    }
    if binding.len() > 32 {
        return None;
    }
    let mut parts = binding.rsplit('+');
    let key = parts.next()?.trim();
    if key.len() != 1 || !key.as_bytes()[0].is_ascii_alphanumeric() {
        return None;
    }
    let mut modifiers = [false; 3];
    for part in parts {
        let index = match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" => 0,
            "alt" => 1,
            "shift" => 2,
            _ => return None,
        };
        if std::mem::replace(&mut modifiers[index], true) {
            return None;
        }
    }
    if modifiers.iter().filter(|&&on| on).count() < 2 {
        return None;
    }
    let mut chord = String::new();
    for (label, on) in ["Ctrl+", "Alt+", "Shift+"].into_iter().zip(modifiers) {
        if on {
            chord.push_str(label);
        }
    }
    chord.push_str(&key.to_ascii_uppercase());
    Some(chord)
}

#[cfg(windows)]
pub fn sync_shortcut(app: &AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let wanted = current_settings(app).pause_shortcut.parse().ok();
    let state = app.state::<AppState>();
    let mut current = state.controls.registered.lock().unwrap();
    if *current != wanted {
        let shortcuts = app.global_shortcut();
        if let Some(previous) = *current {
            if shortcuts.unregister(previous).is_err() {
                drop(current);
                emit(app);
                return;
            }
            *current = None;
        }
        if let Some(shortcut) = wanted {
            if shortcuts
                .on_shortcut(shortcut, |app, _, event| {
                    if event.state() == ShortcutState::Pressed {
                        queue_ui_refresh(app, toggle_shortcut);
                    }
                })
                .is_ok()
            {
                *current = Some(shortcut);
            }
        }
    }
    drop(current);
    emit(app);
}

fn shortcut_unavailable(app: &AppHandle) -> bool {
    #[cfg(windows)]
    {
        let wanted = current_settings(app).pause_shortcut;
        wanted.parse().ok() != *app.state::<AppState>().controls.registered.lock().unwrap()
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
}

/// Capture the foreground app before the native tray popup takes focus.
pub fn capture_foreground(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = current_settings(app);
    let target = crate::system::foreground_exe().filter(|exe| {
        settings.excluded_apps.len() < crate::settings::MAX_EXCLUDED_APPS
            && !settings.excluded_apps.contains(exe)
    });
    if let Some(item) = state.controls.exclude_item.lock().unwrap().as_ref() {
        let text = target.as_deref().map_or_else(
            || strings::MENU_EXCLUDE_APP.to_owned(),
            strings::menu_exclude_app,
        );
        let _ = item.set_text(text);
        let _ = item.set_enabled(target.is_some());
    }
    *state.controls.target.lock().unwrap() = target;
}

pub fn exclude_foreground(app: &AppHandle) {
    let target = app
        .state::<AppState>()
        .controls
        .target
        .lock()
        .unwrap()
        .take();
    if let Some(exe) = target {
        if let Err(e) = update_settings(app, |s| {
            if s.excluded_apps.len() < crate::settings::MAX_EXCLUDED_APPS
                && !s.excluded_apps.contains(&exe)
            {
                s.excluded_apps.push(exe);
            }
        }) {
            eprintln!("app exclusion: {e}");
        }
    }
    capture_foreground(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_pause_expires_only_when_due() {
        let now = Instant::now();
        let end = now + Duration::from_secs(60);
        let mut pause = Pause::Until(end);
        assert!(!pause.expire(now));
        assert!(pause.active());
        assert!(pause.expire(end));
        assert!(!pause.active());
        assert!(!pause.expire(end));
        assert!(!Pause::UntilRestart.expire(end));
    }

    #[test]
    fn shortcut_requires_two_distinct_modifiers_and_a_letter_or_digit() {
        assert_eq!(
            normalise_shortcut("shift + ctrl + u").as_deref(),
            Some("Ctrl+Shift+U")
        );
        assert_eq!(
            normalise_shortcut("Ctrl+Alt+1").as_deref(),
            Some("Ctrl+Alt+1")
        );
        assert_eq!(normalise_shortcut(" ").as_deref(), Some(""));
        for bad in [
            "A",
            "Ctrl+A",
            "Ctrl+Ctrl+A",
            "Ctrl+Alt+F4",
            "Ctrl++Alt+A",
            "Ctrl+Alt+é",
        ] {
            assert_eq!(normalise_shortcut(bad), None, "{bad}");
        }
    }
}
