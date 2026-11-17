//! Updates from api.rmgn.ca: a quiet check a day, a background download when automatic
//! updates are on, and an install only when the user restarts or quits unflub.
//!
//! - **Check.** `GET /unflub/v1/update/{target}/{arch}/{current_version}?channel=…` with the
//!   headers in `net.rs` (no install ID: a rollout bucket and `X-First`). The first check runs
//!   about two minutes after launch, then every 24
//!   hours plus up to 90 random minutes, and also on "Check now". Never more than once an hour
//!   (or the remote config's `min_check_interval_h`). The server decides who gets a release
//!   (staged rollout) and answers 204 when nothing applies. Each check also refreshes the
//!   remote config.
//! - **Who updates.** Only copies from the setup or winget update themselves (`install.rs`);
//!   Scoop, Store and portable copies are told to update with their tool. A machine policy
//!   (`managed.rs`) can turn update checks off entirely.
//! - **Download.** With automatic updates on, a release build downloads the update in the
//!   background, verifies it against the public key in `tauri.conf.json`, and keeps it on disk
//!   (`%LOCALAPPDATA%\ca.rmgn.unflub\updates`), not in memory. Off: the user is only told.
//! - **Install.** "Restart now" (About page, tray) or quitting from the tray runs the installer.
//!   Nothing restarts on its own and nothing interrupts typing. A downloaded update survives a
//!   restart of unflub; if the server stops offering it (paused or blocked) it is deleted.
//!
//! Nothing here shows a dialog: the settings UI reads [`UpdateStatus`] through `update_status`
//! and the [`STATUS_EVENT`] event, and the tray menu follows the same status.

use crate::{links, net, remote::Remote};
use serde::{Deserialize, Serialize};
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
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

/// Emitted with an [`UpdateStatus`] whenever it changes.
pub const STATUS_EVENT: &str = "update-status";

const HOUR_SECS: u64 = 60 * 60;
const DAY_SECS: u64 = 24 * HOUR_SECS;
/// Leave the first check until login has settled.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(2 * 60);
/// How often the background thread wakes to see whether a check is due.
const POLL: Duration = Duration::from_secs(10 * 60);
/// Spread daily checks so installs don't all arrive at the same minute.
const MAX_JITTER: Duration = Duration::from_secs(90 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MANAGED: &str = "Updates are managed by your organization.";

/// Mirrors `UpdateStatus` in `app/src/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum UpdateStatus {
    /// Not checked since unflub started.
    Idle,
    Checking,
    UpToDate,
    /// A newer version that isn't downloaded (automatic updates are off). "Restart now"
    /// downloads and installs it.
    Available {
        version: String,
        notes: String,
        critical: bool,
    },
    /// Downloading and verifying in the background.
    Downloading {
        version: String,
    },
    /// Downloaded and verified. Installs on "Restart now" or when unflub quits.
    Ready {
        version: String,
        notes: String,
        critical: bool,
    },
    /// Starting the installer.
    Installing {
        version: String,
    },
    Error {
        message: String,
    },
}

/// A downloaded update, kept beside its file so it survives a restart.
#[derive(Debug, Serialize, Deserialize)]
struct PendingFile {
    version: String,
    file: String,
    /// The server's answer, for `Updater::restore_update`.
    raw_json: serde_json::Value,
}

pub struct Updates {
    status: Mutex<UpdateStatus>,
    /// The update found by the last check (or restored from disk).
    update: Mutex<Option<Update>>,
    /// Its verified installer on disk, when downloaded, and the SHA-256 of the bytes that were
    /// verified. The file is only installed if it still hashes the same.
    downloaded: Mutex<Option<(PathBuf, [u8; 32])>>,
    busy: AtomicBool,
    /// Holds the time of the last check, so restarts don't check more than once an hour.
    stamp: PathBuf,
    /// Holds the time of the last check the server answered, for `X-First`.
    heartbeat: PathBuf,
    /// Downloaded updates.
    cache_dir: PathBuf,
}

impl Updates {
    pub fn new(config_dir: PathBuf, local_data_dir: PathBuf) -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle),
            update: Mutex::new(None),
            downloaded: Mutex::new(None),
            busy: AtomicBool::new(false),
            stamp: config_dir.join("last-update-check"),
            heartbeat: config_dir.join("last-heartbeat"),
            cache_dir: local_data_dir.join("updates"),
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

    fn last_heartbeat(&self) -> Option<u64> {
        fs::read_to_string(&self.heartbeat)
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn mark_heartbeat(&self) {
        let _ = fs::write(&self.heartbeat, now_secs().to_string());
    }

    fn pending_path(&self) -> PathBuf {
        self.cache_dir.join("pending.json")
    }

    /// Delete any downloaded update.
    fn discard_download(&self) {
        *self.downloaded.lock().unwrap() = None;
        let _ = fs::remove_dir_all(&self.cache_dir);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// At least `min_secs` since the last check (or none yet, or the clock went backwards).
fn waited(last: Option<u64>, now: u64, min_secs: u64) -> bool {
    match last {
        Some(last) => now < last || now - last >= min_secs,
        None => true,
    }
}

/// The update endpoint for a channel, with the updater's own `{{…}}` variables left in.
pub fn endpoint(channel: &str) -> String {
    format!(
        "{}/update/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}?channel={channel}",
        links::API_BASE
    )
}

/// An older version offered by the server is a rollback: the current version was blocked.
/// The response has no critical field, so that is the one case unflub can call critical.
fn is_critical(update: &Update) -> bool {
    let older = |current: &str, offered: &str| match (
        semver::Version::parse(current),
        semver::Version::parse(offered),
    ) {
        (Ok(c), Ok(o)) => o < c,
        _ => false,
    };
    older(&update.current_version, &update.version)
}

/// Updates only ever come over https; a plain-http URL in a response is refused.
fn check_url(update: &Update) -> Result<(), String> {
    if update.download_url.scheme() == "https" {
        Ok(())
    } else {
        Err("The update server sent a download address that isn't https.".into())
    }
}

fn set_status(app: &AppHandle, status: UpdateStatus) -> UpdateStatus {
    *app.state::<Updates>().status.lock().unwrap() = status.clone();
    let _ = app.emit(STATUS_EVENT, &status);
    crate::refresh_tray_menu(app);
    status
}

/// The headers for the update check and remote config, as they would go out now.
pub fn api_headers(app: &AppHandle) -> net::Headers {
    let version = app.package_info().version.to_string();
    let first = net::first_periods(app.state::<Updates>().last_heartbeat(), now_secs());
    net::update_headers(
        &version,
        &crate::system::os_version(),
        app.state::<net::Rollout>().bucket(&version),
        &first,
    )
}

/// The server decides which version this copy should run: newer, or older when the current one
/// was blocked (a rollback). So any version other than the current one is accepted.
fn accept_server_choice(current: &semver::Version, offered: &semver::Version) -> bool {
    offered != current
}

fn updater(app: &AppHandle) -> Result<Updater, String> {
    let settings = crate::current_settings(app);
    let url = tauri::Url::parse(&endpoint(settings.channel.as_str())).map_err(|e| e.to_string())?;
    let headers = api_headers(app).into_iter().collect();
    let exit_handle = app.clone();
    app.updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .version_comparator(|current, release| accept_server_choice(&current, &release.version))
        .headers(headers)
        .timeout(REQUEST_TIMEOUT)
        // Windows installers need unflub closed: unhook and remove the tray icon first.
        .on_before_exit(move || {
            crate::stop_engine(&exit_handle);
            exit_handle.cleanup_before_exit();
        })
        .build()
        .map_err(|e| e.to_string())
}

/// Refresh the remote config (version headers only), and apply its flags. Skipped under the
/// `DisableRemoteConfig` policy.
pub async fn refresh_remote(app: &AppHandle) {
    if app.state::<crate::AppInfo>().managed.disable_remote_config {
        return;
    }
    let settings = crate::current_settings(app);
    let version = app.package_info().version.to_string();
    let Ok(client) = net::client(api_headers(app)) else {
        return;
    };
    let remote = app.state::<Remote>();
    if remote
        .refresh(&client, settings.channel.as_str(), &version)
        .await
    {
        crate::apply_remote(app);
    }
}

/// Check now (`manual`: the user pressed "Check now"). One check, download or install runs at a
/// time; a second caller gets the current status. Within the minimum interval since the last
/// check, the last answer stands, unless there is none yet or it was an error.
pub async fn check(app: &AppHandle, manual: bool) -> UpdateStatus {
    let updates = app.state::<Updates>();
    if app.state::<crate::AppInfo>().managed.disable_updates {
        return set_status(
            app,
            UpdateStatus::Error {
                message: MANAGED.into(),
            },
        );
    }
    let min_secs = u64::from(app.state::<Remote>().flags().min_check_interval_h) * HOUR_SECS;
    let current = updates.status();
    let recent = !waited(updates.last_check(), now_secs(), min_secs);
    if recent && !matches!(current, UpdateStatus::Idle | UpdateStatus::Error { .. }) {
        return current;
    }
    if !manual && recent {
        // Restarted within the hour: the background check waits for the next round.
        return current;
    }
    if updates.busy.swap(true, Ordering::AcqRel) {
        return current;
    }
    refresh_remote(app).await;
    // A background check doesn't hide an update that is waiting for the user.
    let quiet = !manual
        && matches!(
            current,
            UpdateStatus::Ready { .. } | UpdateStatus::Available { .. }
        );
    if !quiet {
        set_status(app, UpdateStatus::Checking);
    }
    let result = match updater(app) {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    updates.mark_checked();
    if result.is_ok() {
        updates.mark_heartbeat();
    }
    let status = match result {
        Ok(found) => found_update(app, found).await,
        Err(message) if quiet => {
            eprintln!("update check: {message}");
            current
        }
        Err(message) => UpdateStatus::Error { message },
    };
    updates.busy.store(false, Ordering::Release);
    set_status(app, status)
}

/// What a successful check found. Downloads it when automatic updates are on.
async fn found_update(app: &AppHandle, found: Option<Update>) -> UpdateStatus {
    let updates = app.state::<Updates>();
    let Some(update) = found else {
        // Paused, blocked or replaced on the server: don't install what we downloaded.
        *updates.update.lock().unwrap() = None;
        updates.discard_download();
        return UpdateStatus::UpToDate;
    };
    if let Err(message) = check_url(&update) {
        *updates.update.lock().unwrap() = None;
        updates.discard_download();
        return UpdateStatus::Error { message };
    }
    let version = update.version.clone();
    let notes = update.body.clone().unwrap_or_default();
    let critical = is_critical(&update);
    if critical {
        eprintln!(
            "update: rollback from {} to {version} offered by the server",
            update.current_version
        );
    }
    let already = updates.downloaded.lock().unwrap().is_some()
        && matches!(updates.status(), UpdateStatus::Ready { version: ref v, .. } if *v == version);
    if already {
        *updates.update.lock().unwrap() = Some(update);
        return UpdateStatus::Ready {
            version,
            notes,
            critical,
        };
    }
    updates.discard_download();
    let info = app.state::<crate::AppInfo>();
    let auto = info
        .managed
        .effective(crate::current_settings(app))
        .auto_update
        && info.install_source.self_updates();
    if !auto || cfg!(debug_assertions) {
        *updates.update.lock().unwrap() = Some(update);
        return UpdateStatus::Available {
            version,
            notes,
            critical,
        };
    }
    set_status(
        app,
        UpdateStatus::Downloading {
            version: version.clone(),
        },
    );
    match download(&updates, &update).await {
        Ok(path) => {
            *updates.downloaded.lock().unwrap() = Some(path);
            *updates.update.lock().unwrap() = Some(update);
            UpdateStatus::Ready {
                version,
                notes,
                critical,
            }
        }
        Err(e) => {
            eprintln!("update download: {e}");
            updates.discard_download();
            *updates.update.lock().unwrap() = Some(update);
            UpdateStatus::Available {
                version,
                notes,
                critical,
            }
        }
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// Read the cached installer and check it is byte for byte what was verified. Another program
/// running as the same user could replace the file while it waits; then nothing is installed.
fn read_verified(path: &std::path::Path, expected: &[u8; 32]) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if sha256(&bytes) != *expected {
        return Err("The downloaded update changed on disk, so it wasn't installed.".into());
    }
    Ok(bytes)
}

/// Download and verify, then write the installer and its metadata to the cache folder:
/// `%LOCALAPPDATA%\ca.rmgn.unflub\updates`, which inherits the profile's ACL (the user,
/// SYSTEM and Administrators only). Returns the file and the hash of the verified bytes.
async fn download(updates: &Updates, update: &Update) -> Result<(PathBuf, [u8; 32]), String> {
    // `download` checks the signature against the public key before returning the bytes.
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    fs::create_dir_all(&updates.cache_dir).map_err(|e| e.to_string())?;
    let file = format!("unflub-{}.update", update.version);
    let path = updates.cache_dir.join(&file);
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let pending = PendingFile {
        version: update.version.clone(),
        file,
        raw_json: update.raw_json.clone(),
    };
    let json = serde_json::to_vec_pretty(&pending).map_err(|e| e.to_string())?;
    fs::write(updates.pending_path(), json).map_err(|e| e.to_string())?;
    Ok((path, sha256(&bytes)))
}

/// Pick up an update downloaded before unflub last closed. Deletes it if it is no longer newer
/// than this version or fails verification.
fn restore_download(app: &AppHandle) {
    let updates = app.state::<Updates>();
    let Ok(text) = fs::read_to_string(updates.pending_path()) else {
        return;
    };
    let restore = || -> Option<(Update, (PathBuf, [u8; 32]))> {
        let pending: PendingFile = serde_json::from_str(&text).ok()?;
        let update = updater(app).ok()?.restore_update(pending.raw_json).ok()??;
        let path = updates.cache_dir.join(&pending.file);
        let bytes = fs::read(&path).ok()?;
        update.verify(&bytes).ok()?;
        Some((update, (path, sha256(&bytes))))
    };
    let Some((update, file)) = restore() else {
        updates.discard_download();
        return;
    };
    let status = UpdateStatus::Ready {
        version: update.version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        critical: is_critical(&update),
    };
    *updates.downloaded.lock().unwrap() = Some(file);
    *updates.update.lock().unwrap() = Some(update);
    set_status(app, status);
}

/// Install now: the downloaded update, or download one first. On Windows the installer closes
/// unflub and, with `restart`, opens it again when done, so this only returns on failure.
pub async fn install(app: &AppHandle, restart: bool) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("Dev builds don't install updates.".into());
    }
    let info = app.state::<crate::AppInfo>();
    if info.managed.disable_updates {
        return Err(MANAGED.into());
    }
    if !info.install_source.self_updates() {
        return Err("This copy of unflub is updated by the tool that installed it.".into());
    }
    let updates = app.state::<Updates>();
    if updates.busy.swap(true, Ordering::AcqRel) {
        return Err("An update check is running. Try again in a moment.".into());
    }
    let Some(update) = updates.update.lock().unwrap().take() else {
        updates.busy.store(false, Ordering::Release);
        return Err("No update is available.".into());
    };
    let update = update.restart_after_install(restart);
    set_status(
        app,
        UpdateStatus::Installing {
            version: update.version.clone(),
        },
    );
    let downloaded = updates.downloaded.lock().unwrap().clone();
    // A cached installer must hash the same as the bytes verified at download, and `install`
    // checks those exact bytes against the public key again before it writes and runs them.
    // Without a cached file, `download_and_install` verifies what it downloads.
    let result = match downloaded {
        Some((path, hash)) => read_verified(&path, &hash)
            .and_then(|bytes| update.install(bytes).map_err(|e| e.to_string())),
        None => update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string()),
    };
    updates.busy.store(false, Ordering::Release);
    if let Err(message) = result {
        updates.discard_download();
        set_status(
            app,
            UpdateStatus::Error {
                message: message.clone(),
            },
        );
        return Err(message);
    }
    // The installer ran; don't offer the same file again.
    updates.discard_download();
    if restart {
        // Only reached on macOS and Linux; Windows has exited by now.
        app.restart()
    }
    Ok(())
}

/// Quitting from the tray: install a downloaded update on the way out, without reopening
/// unflub. True when the installer took over (on Windows the process has exited by then).
pub fn install_on_quit(app: &AppHandle) -> bool {
    let updates = app.state::<Updates>();
    if cfg!(debug_assertions) || updates.downloaded.lock().unwrap().is_none() {
        return false;
    }
    match tauri::async_runtime::block_on(install(app, false)) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("install on quit: {e}");
            false
        }
    }
}

/// The `unflub-update` thread. Release builds only: a dev build would find every release newer
/// than itself.
pub fn spawn_background(app: AppHandle) {
    if cfg!(debug_assertions) || app.state::<crate::AppInfo>().managed.disable_updates {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("unflub-update".into())
        .spawn(move || {
            restore_download(&app);
            std::thread::sleep(FIRST_CHECK_DELAY);
            // The first check of this run only waits out the minimum interval.
            tauri::async_runtime::block_on(check(&app, false));
            let jitter = net::jitter(MAX_JITTER).as_secs();
            loop {
                std::thread::sleep(POLL);
                let last = app.state::<Updates>().last_check();
                if waited(last, now_secs(), DAY_SECS + jitter) {
                    tauri::async_runtime::block_on(check(&app, false));
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("update thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cached installer that changed after verification is refused before anything runs.
    #[test]
    fn tampered_download_is_refused() {
        let dir = std::env::temp_dir().join(format!("unflub-upd-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("unflub-0.2.0.update");
        fs::write(&path, b"the verified installer").unwrap();
        let hash = sha256(b"the verified installer");
        assert_eq!(
            read_verified(&path, &hash).unwrap(),
            b"the verified installer"
        );
        fs::write(&path, b"something else entirely").unwrap();
        let err = read_verified(&path, &hash).unwrap_err();
        assert!(err.contains("changed on disk"), "{err}");
        fs::remove_file(&path).unwrap();
        assert!(
            read_verified(&path, &hash).is_err(),
            "a missing file is refused too"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn any_version_the_server_picks_is_accepted() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        assert!(accept_server_choice(&v("0.2.0"), &v("0.3.0")));
        assert!(accept_server_choice(&v("0.2.0"), &v("0.1.4")), "rollback");
        assert!(!accept_server_choice(&v("0.2.0"), &v("0.2.0")));
    }

    #[test]
    fn waits_out_the_interval() {
        let now = 1_800_000_000;
        assert!(waited(None, now, HOUR_SECS));
        assert!(!waited(Some(now - 60), now, HOUR_SECS));
        assert!(!waited(Some(now - HOUR_SECS + 1), now, HOUR_SECS));
        assert!(waited(Some(now - HOUR_SECS), now, HOUR_SECS));
        assert!(!waited(Some(now - HOUR_SECS), now, DAY_SECS));
        // A clock set backwards shouldn't silence checks for days.
        assert!(waited(Some(now + 3600), now, HOUR_SECS));
    }

    /// The contract's path, with the updater's template variables for it to fill in.
    #[test]
    fn endpoint_follows_the_contract() {
        assert_eq!(
            endpoint("stable"),
            "https://api.rmgn.ca/unflub/v1/update/{{target}}/{{arch}}/{{current_version}}?channel=stable"
        );
        assert!(endpoint("beta").ends_with("?channel=beta"));
        // Parses, and the template survives (percent-encoded in the path, which the updater
        // turns back into values).
        let url = tauri::Url::parse(&endpoint("stable")).unwrap();
        assert!(url.as_str().contains("%7B%7Btarget%7D%7D"), "{url}");
        assert_eq!(url.query(), Some("channel=stable"));
    }

    /// tauri.conf.json carries the same endpoint (stable), so the config and the code agree.
    #[test]
    fn config_endpoint_matches() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["plugins"]["updater"]["endpoints"][0].as_str(),
            Some(endpoint("stable").as_str())
        );
    }

    /// The settings UI switches on `state`; keep in step with `UpdateStatus` in types.ts.
    #[test]
    fn status_json_shape() {
        let json = |s: UpdateStatus| serde_json::to_string(&s).unwrap();
        assert_eq!(json(UpdateStatus::Idle), r#"{"state":"idle"}"#);
        assert_eq!(json(UpdateStatus::UpToDate), r#"{"state":"up-to-date"}"#);
        assert_eq!(
            json(UpdateStatus::Available {
                version: "0.2.0".into(),
                notes: "Fixes".into(),
                critical: true,
            }),
            r#"{"state":"available","version":"0.2.0","notes":"Fixes","critical":true}"#
        );
        assert_eq!(
            json(UpdateStatus::Downloading {
                version: "0.2.0".into()
            }),
            r#"{"state":"downloading","version":"0.2.0"}"#
        );
        assert_eq!(
            json(UpdateStatus::Ready {
                version: "0.2.0".into(),
                notes: String::new(),
                critical: false,
            }),
            r#"{"state":"ready","version":"0.2.0","notes":"","critical":false}"#
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
