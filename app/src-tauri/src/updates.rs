//! Updates from dl.rmgn.net: a quiet check a day, a background download when automatic
//! updates are on, and an install only when the user restarts or quits unflub.
//!
//! - **Check.** Fetch the signed manifest for the channel (`manifest.rs`, `remote.rs`), then
//!   decide here, with the local rollout bucket, whether a release applies
//!   ([`manifest::eligible`]). Only then is the Tauri updater pointed at that release's static
//!   file (`dl.rmgn.net/unflub/{version}/tauri-{target}-{arch}.json`), and its download URL and
//!   signature must match the manifest's. Nothing identifying is sent; the bucket never leaves
//!   the computer. The first check runs about two minutes after launch, then every 24 hours
//!   plus up to 90 random minutes, and also on "Check now". Never more than once an hour (or the
//!   manifest's `min_check_interval_h`).
//! - **Who updates.** Only copies from the setup update themselves (`install.rs`); winget,
//!   Chocolatey, Scoop, Store and portable copies are told to update with their tool. A machine policy
//!   (`managed.rs`) can turn update checks off entirely.
//! - **Download.** With automatic updates on, a release build downloads the update in the
//!   background, verifies it against the public key in `tauri.conf.json` and the SHA-256 and
//!   size in the signed manifest, and keeps it on disk (`%LOCALAPPDATA%\ca.rmgn.unflub\updates`),
//!   not in memory. Off: the user is only told.
//! - **Install.** "Restart now" (About page, tray) or quitting from the tray runs the installer.
//!   Nothing restarts on its own and nothing interrupts typing. A downloaded update survives a
//!   restart of unflub; if the manifest stops offering it (paused or blocked) it is deleted.
//!
//! Nothing here shows a dialog: the settings UI reads [`UpdateStatus`] through `update_status`
//! and the [`STATUS_EVENT`] event, and the tray menu follows the same status.

use crate::{
    links,
    manifest::{self, Expected, Offer},
    net,
    remote::{RefreshError, Remote},
};
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
const MISMATCH: &str = "The update file doesn't match the signed release list.";

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
    /// The release's Tauri file, for `Updater::restore_update`.
    raw_json: serde_json::Value,
    /// From the signed manifest: hex SHA-256 and size of the installer.
    sha256: String,
    size: u64,
    critical: bool,
}

/// An update the manifest offers, with what its installer must hash to.
struct Found {
    update: Update,
    expected: Expected,
    notes: String,
    critical: bool,
}

pub struct Updates {
    status: Mutex<UpdateStatus>,
    /// The update found by the last check (or restored from disk).
    update: Mutex<Option<Found>>,
    /// Its verified installer on disk, when downloaded, and the SHA-256 of the bytes that were
    /// verified. The file is only installed if it still hashes the same.
    downloaded: Mutex<Option<(PathBuf, [u8; 32])>>,
    busy: AtomicBool,
    /// Holds the time of the last check, so restarts don't check more than once an hour.
    stamp: PathBuf,
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

/// Accept exactly the version the manifest chose: newer, or older when the current one was
/// blocked (a rollback). Tauri's own "newer only" rule would refuse the rollback.
fn accepts(chosen: &semver::Version, current: &semver::Version, offered: &semver::Version) -> bool {
    offered == chosen && offered != current
}

/// The release's static Tauri file must point at the installer and signature the signed
/// manifest names: it comes over TLS but is not signed itself. The installer must be a GitHub
/// Release asset or a dl.rmgn.net address (which redirects to one).
fn matches_offer(download_url: &tauri::Url, signature: &str, offer: &Offer) -> Result<(), String> {
    let url = tauri::Url::parse(&offer.platform.url).map_err(|e| e.to_string())?;
    if !links::installer_url_allowed(download_url) || !links::installer_url_allowed(&url) {
        return Err("The update server sent a download address unflub doesn't trust.".into());
    }
    if *download_url != url || signature.trim() != offer.platform.signature.trim() {
        return Err(MISMATCH.into());
    }
    Ok(())
}

fn set_status(app: &AppHandle, status: UpdateStatus) -> UpdateStatus {
    *app.state::<Updates>().status.lock().unwrap() = status.clone();
    let _ = app.emit(STATUS_EVENT, &status);
    crate::refresh_tray_menu(app);
    status
}

/// The headers on every manifest and release file request, as they would go out now.
pub fn request_headers(app: &AppHandle) -> net::Headers {
    net::headers(&app.package_info().version.to_string())
}

/// Downloads follow redirects only to GitHub's release hosts ([`links::redirect_allowed`]),
/// at most five, and only over https.
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else if links::redirect_allowed(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("redirected to an address unflub doesn't trust")
        }
    })
}

/// An updater for one release: its static Tauri file is the only endpoint, and only its
/// version is accepted.
fn updater(app: &AppHandle, version: &str) -> Result<Updater, String> {
    let chosen = semver::Version::parse(version).map_err(|e| e.to_string())?;
    let url = tauri::Url::parse(&manifest::tauri_url(
        version,
        net::target(),
        std::env::consts::ARCH,
    ))
    .map_err(|e| e.to_string())?;
    let headers = request_headers(app).into_iter().collect();
    let exit_handle = app.clone();
    app.updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .version_comparator(move |current, release| accepts(&chosen, &current, &release.version))
        .headers(headers)
        .configure_client(|client| client.https_only(true).redirect(redirect_policy()))
        .timeout(REQUEST_TIMEOUT)
        // Windows installers need unflub closed: unhook and remove the tray icon first.
        .on_before_exit(move || {
            crate::stop_engine(&exit_handle);
            exit_handle.cleanup_before_exit();
        })
        .build()
        .map_err(|e| e.to_string())
}

/// Fetch the channel's manifest and apply its flags and message when they changed. Skipped
/// when both `DisableUpdates` and `DisableRemoteConfig` are set. A manifest that isn't believed
/// is logged and the last good one stays; only a network error is an error.
pub async fn refresh_remote(app: &AppHandle) -> Result<(), String> {
    let managed = app.state::<crate::AppInfo>().managed;
    if managed.disable_updates && managed.disable_remote_config {
        return Ok(());
    }
    let channel = crate::current_settings(app).channel.as_str();
    let client = net::client(request_headers(app)).map_err(|e| e.to_string())?;
    match app.state::<Remote>().refresh(&client, channel).await {
        Ok(changed) => {
            if changed {
                crate::apply_remote(app);
            }
            Ok(())
        }
        Err(RefreshError::Rejected(why)) => {
            eprintln!("manifest: {why}; keeping the last good one");
            Ok(())
        }
        Err(RefreshError::Network(e)) => Err(e),
    }
}

/// Refresh the manifest and decide what it offers this copy.
async fn find_update(app: &AppHandle) -> Result<Option<Found>, String> {
    refresh_remote(app).await?;
    let channel = crate::current_settings(app).channel.as_str();
    let manifest = app
        .state::<Remote>()
        .manifest(channel)
        .ok_or_else(|| "The update server has no valid release list.".to_string())?;
    // A frozen copy of an old manifest must not pin this app to what it offered.
    if manifest.expired(time::OffsetDateTime::now_utc()) {
        return Err("unflub will try again later.".into());
    }
    let current = app.package_info().version.clone();
    let bucket = app.state::<net::Rollout>().bucket(&current.to_string());
    let Some(offer) = manifest::eligible(&manifest, &current, bucket, &manifest::platform_key())
    else {
        return Ok(None);
    };
    let expected = Expected::from_platform(&offer.platform)?;
    let update = updater(app, &offer.release.version)?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| MISMATCH.to_string())?;
    matches_offer(&update.download_url, &update.signature, &offer)?;
    if offer.critical {
        eprintln!(
            "update: {} offered as critical over {current}",
            offer.release.version
        );
    }
    Ok(Some(Found {
        notes: offer
            .release
            .notes
            .clone()
            .or_else(|| update.body.clone())
            .unwrap_or_default(),
        update,
        expected,
        critical: offer.critical,
    }))
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
    // A background check doesn't hide an update that is waiting for the user.
    let quiet = !manual
        && matches!(
            current,
            UpdateStatus::Ready { .. } | UpdateStatus::Available { .. }
        );
    if !quiet {
        set_status(app, UpdateStatus::Checking);
    }
    let result = find_update(app).await;
    updates.mark_checked();
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
async fn found_update(app: &AppHandle, found: Option<Found>) -> UpdateStatus {
    let updates = app.state::<Updates>();
    let Some(found) = found else {
        // Paused, blocked or replaced in the manifest: don't install what we downloaded.
        *updates.update.lock().unwrap() = None;
        updates.discard_download();
        return UpdateStatus::UpToDate;
    };
    let version = found.update.version.clone();
    let notes = found.notes.clone();
    let critical = found.critical;
    let already = updates
        .downloaded
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|(_, hash)| *hash == found.expected.sha256)
        && matches!(updates.status(), UpdateStatus::Ready { version: ref v, .. } if *v == version);
    if already {
        *updates.update.lock().unwrap() = Some(found);
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
        *updates.update.lock().unwrap() = Some(found);
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
    match download(&updates, &found).await {
        Ok(path) => {
            *updates.downloaded.lock().unwrap() = Some(path);
            *updates.update.lock().unwrap() = Some(found);
            UpdateStatus::Ready {
                version,
                notes,
                critical,
            }
        }
        Err(e) => {
            eprintln!("update download: {e}");
            updates.discard_download();
            *updates.update.lock().unwrap() = Some(found);
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

/// Download, then verify against the public key and the manifest's SHA-256 and size.
async fn fetch_verified(found: &Found) -> Result<Vec<u8>, String> {
    // `download` checks the signature against the public key before returning the bytes.
    let bytes = found
        .update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    found.expected.check(&bytes)?;
    Ok(bytes)
}

/// Download and verify, then write the installer and its metadata to the cache folder:
/// `%LOCALAPPDATA%\ca.rmgn.unflub\updates`, which inherits the profile's ACL (the user,
/// SYSTEM and Administrators only). Returns the file and the hash of the verified bytes.
async fn download(updates: &Updates, found: &Found) -> Result<(PathBuf, [u8; 32]), String> {
    let bytes = fetch_verified(found).await?;
    fs::create_dir_all(&updates.cache_dir).map_err(|e| e.to_string())?;
    let file = format!("unflub-{}.update", found.update.version);
    let path = updates.cache_dir.join(&file);
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let pending = PendingFile {
        version: found.update.version.clone(),
        file,
        raw_json: found.update.raw_json.clone(),
        sha256: found.expected.sha256_hex(),
        size: found.expected.size,
        critical: found.critical,
    };
    let json = serde_json::to_vec_pretty(&pending).map_err(|e| e.to_string())?;
    fs::write(updates.pending_path(), json).map_err(|e| e.to_string())?;
    Ok((path, sha256(&bytes)))
}

/// Pick up an update downloaded before unflub last closed. Deletes it if it is no longer
/// accepted or fails verification. The next check deletes it too if the manifest no longer
/// offers it.
fn restore_download(app: &AppHandle) {
    let updates = app.state::<Updates>();
    let Ok(text) = fs::read_to_string(updates.pending_path()) else {
        return;
    };
    // Downloaded before a package manager took this copy over: it is that tool's to update.
    if !app.state::<crate::AppInfo>().install_source.self_updates() {
        updates.discard_download();
        return;
    }
    let restore = || -> Option<(Found, (PathBuf, [u8; 32]))> {
        let pending: PendingFile = serde_json::from_str(&text).ok()?;
        let update = updater(app, &pending.version)
            .ok()?
            .restore_update(pending.raw_json)
            .ok()??;
        let expected = Expected {
            sha256: manifest::parse_hex32(&pending.sha256)?,
            size: pending.size,
        };
        let path = updates.cache_dir.join(&pending.file);
        let bytes = fs::read(&path).ok()?;
        expected.check(&bytes).ok()?;
        update.verify(&bytes).ok()?;
        let found = Found {
            notes: update.body.clone().unwrap_or_default(),
            update,
            expected,
            critical: pending.critical,
        };
        Some((found, (path, sha256(&bytes))))
    };
    let Some((found, file)) = restore() else {
        updates.discard_download();
        return;
    };
    let status = UpdateStatus::Ready {
        version: found.update.version.clone(),
        notes: found.notes.clone(),
        critical: found.critical,
    };
    *updates.downloaded.lock().unwrap() = Some(file);
    *updates.update.lock().unwrap() = Some(found);
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
    let Some(mut found) = updates.update.lock().unwrap().take() else {
        updates.busy.store(false, Ordering::Release);
        return Err("No update is available.".into());
    };
    found.update = found.update.restart_after_install(restart);
    set_status(
        app,
        UpdateStatus::Installing {
            version: found.update.version.clone(),
        },
    );
    let downloaded = updates.downloaded.lock().unwrap().clone();
    // A cached installer must hash the same as the bytes verified at download, and `install`
    // checks those exact bytes against the public key again before it writes and runs them.
    // Without a cached file, the download is checked against the key and the manifest first.
    let bytes = match downloaded {
        Some((path, hash)) => read_verified(&path, &hash),
        None => fetch_verified(&found).await,
    };
    let result = bytes.and_then(|bytes| found.update.install(bytes).map_err(|e| e.to_string()));
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
    fn only_the_version_the_manifest_chose_is_accepted() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        assert!(accepts(&v("0.3.0"), &v("0.2.0"), &v("0.3.0")));
        assert!(accepts(&v("0.1.4"), &v("0.2.0"), &v("0.1.4")), "rollback");
        assert!(
            !accepts(&v("0.3.0"), &v("0.2.0"), &v("0.2.5")),
            "not the chosen one"
        );
        assert!(!accepts(&v("0.2.0"), &v("0.2.0"), &v("0.2.0")));
    }

    /// The unsigned Tauri file can't swap in another installer or signature.
    #[test]
    fn tauri_file_must_match_the_manifest() {
        let offer = Offer {
            release: manifest::Release {
                version: "0.3.0".into(),
                pub_date: None,
                notes: None,
                rollout_percent: 100,
                platforms: Default::default(),
            },
            platform: manifest::Platform {
                url: "https://dl.rmgn.net/unflub/0.3.0/unflub_0.3.0_x64-setup.exe".into(),
                signature: "sig-0.3.0".into(),
                sha256: "00".repeat(32),
                size: 1,
            },
            critical: false,
        };
        let url = |s: &str| tauri::Url::parse(s).unwrap();
        let good = url("https://dl.rmgn.net/unflub/0.3.0/unflub_0.3.0_x64-setup.exe");
        assert_eq!(matches_offer(&good, "sig-0.3.0\n", &offer), Ok(()));
        let older = url("https://dl.rmgn.net/unflub/0.1.0/unflub_0.1.0_x64-setup.exe");
        assert_eq!(
            matches_offer(&older, "sig-0.3.0", &offer),
            Err(MISMATCH.into())
        );
        assert_eq!(
            matches_offer(&good, "sig-0.1.0", &offer),
            Err(MISMATCH.into())
        );
        for bad in [
            "http://dl.rmgn.net/unflub/0.3.0/unflub_0.3.0_x64-setup.exe",
            "https://evil.example/unflub_0.3.0_x64-setup.exe",
        ] {
            let bad_offer = Offer {
                platform: manifest::Platform {
                    url: bad.into(),
                    ..offer.platform.clone()
                },
                ..offer.clone()
            };
            assert!(matches_offer(&url(bad), "sig-0.3.0", &bad_offer)
                .unwrap_err()
                .contains("doesn't trust"));
        }
        // The GitHub Release asset itself is fine too.
        let github = "https://github.com/ReimagineMedias/unflub/releases/download/v0.3.0/unflub_0.3.0_x64-setup.exe";
        let gh_offer = Offer {
            platform: manifest::Platform {
                url: github.into(),
                ..offer.platform.clone()
            },
            ..offer.clone()
        };
        assert_eq!(matches_offer(&url(github), "sig-0.3.0", &gh_offer), Ok(()));
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

    /// The updater has no fixed endpoint: each check points it at the release the signed
    /// manifest chose.
    #[test]
    fn config_has_no_endpoint() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &conf["plugins"]["updater"];
        assert!(updater["pubkey"].as_str().is_some_and(|k| !k.is_empty()));
        assert!(updater.get("endpoints").is_none());
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
