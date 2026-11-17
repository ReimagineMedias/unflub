//! Updates: a quiet check once a day, and a download and install only when the user presses
//! "Restart" on the About page.
//!
//! The endpoint and public key live in `tauri.conf.json` (`plugins.updater`). Every update is
//! verified against that key when it is downloaded. Nothing is downloaded ahead of time, so a
//! pending update costs no memory while unflub idles in the tray. Nothing here shows a dialog:
//! the settings UI reads [`UpdateStatus`] through `update_status` and the [`STATUS_EVENT`] event.

use serde::Serialize;
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Emitted with an [`UpdateStatus`] whenever it changes.
pub const STATUS_EVENT: &str = "update-status";

const DAY_SECS: u64 = 24 * 60 * 60;
/// Leave the first check until login has settled.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(90);
/// How often the background thread wakes to see whether a day has passed.
const POLL: Duration = Duration::from_secs(60 * 60);

/// Mirrors `UpdateStatus` in `app/src/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum UpdateStatus {
    /// Not checked since unflub started.
    Idle,
    Checking,
    UpToDate,
    /// A newer version; "Restart" downloads and installs it.
    Available {
        version: String,
    },
    /// Downloading, verifying and starting the installer.
    Installing {
        version: String,
    },
    Error {
        message: String,
    },
}

pub struct Updates {
    status: Mutex<UpdateStatus>,
    /// The newer version found by the last check, waiting for the user.
    available: Mutex<Option<Update>>,
    busy: AtomicBool,
    /// Holds the time of the last check, so restarts don't check more than once a day.
    stamp: PathBuf,
}

impl Updates {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle),
            available: Mutex::new(None),
            busy: AtomicBool::new(false),
            stamp: config_dir.join("last-update-check"),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    fn last_check(&self) -> Option<u64> {
        fs::read_to_string(&self.stamp).ok()?.trim().parse().ok()
    }

    fn mark_checked(&self) {
        let _ = fs::write(&self.stamp, now_secs().to_string());
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A day has passed since `last` (or there was no check, or the clock went backwards).
fn check_due(last: Option<u64>, now: u64) -> bool {
    match last {
        Some(last) => now < last || now - last >= DAY_SECS,
        None => true,
    }
}

fn set_status(app: &AppHandle, status: UpdateStatus) -> UpdateStatus {
    *app.state::<Updates>().status.lock().unwrap() = status.clone();
    let _ = app.emit(STATUS_EVENT, &status);
    status
}

/// Check now. One check (or install) runs at a time; a second caller gets the current status.
pub async fn check(app: &AppHandle) -> UpdateStatus {
    let updates = app.state::<Updates>();
    if updates.busy.swap(true, Ordering::AcqRel) {
        return updates.status();
    }
    set_status(app, UpdateStatus::Checking);
    let status = match fetch(app).await {
        Ok(status) => status,
        Err(e) => UpdateStatus::Error {
            message: e.to_string(),
        },
    };
    updates.mark_checked();
    updates.busy.store(false, Ordering::Release);
    set_status(app, status)
}

async fn fetch(app: &AppHandle) -> tauri_plugin_updater::Result<UpdateStatus> {
    let exit_handle = app.clone();
    let updater = app
        .updater_builder()
        // Windows installers need unflub closed: unhook and remove the tray icon first.
        .on_before_exit(move || {
            crate::stop_engine(&exit_handle);
            exit_handle.cleanup_before_exit();
        })
        .build()?;
    let update = updater.check().await?;
    let status = match &update {
        Some(update) => UpdateStatus::Available {
            version: update.version.clone(),
        },
        None => UpdateStatus::UpToDate,
    };
    *app.state::<Updates>().available.lock().unwrap() = update;
    Ok(status)
}

/// Download, verify and install the available update, then restart. On Windows the installer
/// closes unflub and opens it again when it is done, so this only returns on failure.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("Dev builds don't install updates.".into());
    }
    let updates = app.state::<Updates>();
    if updates.busy.swap(true, Ordering::AcqRel) {
        return Err("An update check is running. Try again in a moment.".into());
    }
    let Some(update) = updates.available.lock().unwrap().take() else {
        updates.busy.store(false, Ordering::Release);
        return Err("No update is available.".into());
    };
    set_status(
        app,
        UpdateStatus::Installing {
            version: update.version.clone(),
        },
    );
    // The download is checked against the public key before anything runs.
    let result = update.download_and_install(|_, _| {}, || {}).await;
    updates.busy.store(false, Ordering::Release);
    if let Err(e) = result {
        let message = e.to_string();
        set_status(
            app,
            UpdateStatus::Error {
                message: message.clone(),
            },
        );
        return Err(message);
    }
    // Only reached on macOS and Linux; Windows has exited by now.
    app.restart()
}

/// Check once a day while `autoUpdate` is on. Release builds only: a dev build would find
/// every release newer than itself.
pub fn spawn_daily(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("unflub-update".into())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK_DELAY);
            loop {
                let on = crate::current_settings(&app).auto_update;
                if on && check_due(app.state::<Updates>().last_check(), now_secs()) {
                    tauri::async_runtime::block_on(check(&app));
                }
                std::thread::sleep(POLL);
            }
        });
    if let Err(e) = spawned {
        eprintln!("update thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_once_a_day() {
        let now = 1_800_000_000;
        assert!(check_due(None, now));
        assert!(!check_due(Some(now - 60), now));
        assert!(!check_due(Some(now - DAY_SECS + 1), now));
        assert!(check_due(Some(now - DAY_SECS), now));
        // A clock set backwards shouldn't silence checks for days.
        assert!(check_due(Some(now + 3600), now));
    }

    /// The settings UI switches on `state`; keep in step with `UpdateStatus` in types.ts.
    #[test]
    fn status_json_shape() {
        let json = |s: UpdateStatus| serde_json::to_string(&s).unwrap();
        assert_eq!(json(UpdateStatus::Idle), r#"{"state":"idle"}"#);
        assert_eq!(json(UpdateStatus::UpToDate), r#"{"state":"up-to-date"}"#);
        assert_eq!(
            json(UpdateStatus::Available {
                version: "0.2.0".into()
            }),
            r#"{"state":"available","version":"0.2.0"}"#
        );
        assert_eq!(
            json(UpdateStatus::Installing {
                version: "0.2.0".into()
            }),
            r#"{"state":"installing","version":"0.2.0"}"#
        );
        assert_eq!(
            json(UpdateStatus::Error {
                message: "offline".into()
            }),
            r#"{"state":"error","message":"offline"}"#
        );
    }
}
