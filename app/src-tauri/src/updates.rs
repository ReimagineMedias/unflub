//! Updates from dl.rmgn.net: a quiet check a day, a background download when automatic
//! updates are on, and an install only when the user restarts or quits unflub.
//!
//! - **Check.** Fetch the signed manifest for the channel (`manifest.rs`, `remote.rs`), then
//!   decide here, with the local rollout bucket, whether a release applies
//!   ([`manifest::eligible`]). Only then is the Tauri updater pointed at that release's static
//!   file (`dl.rmgn.net/unflub/{version}/tauri-{target}-{arch}.json`), and its download URL and
//!   signature must match the manifest's. Nothing identifying is sent; the bucket never leaves
//!   the computer. The first check runs about two minutes after launch, then every 24 hours
//!   plus up to 90 random minutes. Automatic checks respect the manifest interval on startup.
//!   A failed check retries sooner. "Check now" refreshes even within the automatic interval.
//! - **Who updates.** Only copies from the setup update themselves (`install.rs`); winget,
//!   Chocolatey, Scoop, Store and portable copies are told to update with their tool. A machine policy
//!   (`managed.rs`) can turn update checks off entirely.
//! - **Download.** With automatic updates on, a release build downloads the update in the
//!   background, verifies it against the public key in `tauri.conf.json` and the SHA-256 and
//!   size in the signed manifest, and keeps it on disk (`%LOCALAPPDATA%\ca.rmgn.unflub\updates`),
//!   not in memory. Off: the user is only told.
//! - **Install.** "Restart now" (About page, tray) or quitting from the tray runs the installer.
//!   Nothing restarts on its own and nothing interrupts typing. A downloaded update survives a
//!   restart of unflub only while the last verified manifest still offers it; once it stops
//!   (paused, blocked, or this copy moved past it) it is deleted.
//!
//! Nothing here shows a dialog: the settings UI reads [`UpdateStatus`] through `update_status`
//! and the [`STATUS_EVENT`] event, and the tray menu follows the same status.
//!
//! The usage stats count the funnel here: an update offered (once per version per run),
//! downloaded, its installer started, and failed downloads or installs. "Dismissed" comes from
//! the settings window (`usage_event`).

use crate::{
    links,
    manifest::{self, Expected, Offer},
    net,
    remote::{RefreshError, Remote},
    usage::{UpdateStep, Usage},
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
const MAX_RETRY_SECS: u64 = 40 * 60;
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
    Updated {
        version: String,
    },
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
        phase: UpdatePhase,
        message: String,
        retry: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    Check,
    Install,
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
    retry: Mutex<Option<Retry>>,
    /// Downloaded updates.
    cache_dir: PathBuf,
    /// The last version a check offered this run, so each counts once in the usage stats.
    offered: Mutex<Option<String>>,
}

impl Updates {
    pub fn new(config_dir: PathBuf, local_data_dir: PathBuf, version: &str) -> Self {
        let status = if take_install_notice(&local_data_dir, version) {
            UpdateStatus::Updated {
                version: version.to_string(),
            }
        } else {
            UpdateStatus::Idle
        };
        Self {
            status: Mutex::new(status),
            update: Mutex::new(None),
            downloaded: Mutex::new(None),
            busy: AtomicBool::new(false),
            stamp: config_dir.join("last-update-check"),
            retry: Mutex::new(None),
            cache_dir: local_data_dir.join("updates"),
            offered: Mutex::new(None),
        }
    }

    /// True the first time this run that `version` is offered.
    fn first_offer(&self, version: &str) -> bool {
        let mut offered = self.offered.lock().unwrap();
        if offered.as_deref() == Some(version) {
            return false;
        }
        *offered = Some(version.to_string());
        true
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

    fn background_due(&self, now: u64, interval: u64) -> bool {
        match *self.retry.lock().unwrap() {
            Some(retry) => waited(Some(retry.failed_at), now, retry.delay),
            None => waited(self.last_check(), now, interval),
        }
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

#[derive(Clone, Copy)]
struct Retry {
    failed_at: u64,
    delay: u64,
}

impl Retry {
    fn after_failure(previous: Option<Self>, now: u64) -> Self {
        Self {
            failed_at: now,
            delay: previous.map_or(POLL.as_secs(), |retry| {
                (retry.delay * 2).min(MAX_RETRY_SECS)
            }),
        }
    }
}

/// The setup writes this marker after copying the new executable, only for a restart update.
fn take_install_notice(local_data_dir: &std::path::Path, version: &str) -> bool {
    let path = local_data_dir.join("updated-version");
    let Ok(installed) = fs::read_to_string(&path) else {
        return false;
    };
    fs::remove_file(path).is_ok() && installed.trim() == version
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
        // Tauri calls this before trying to launch the installer. Leave the engine and
        // windows running if the launch fails. Process exit closes the engine's Job Object.
        .on_before_exit(move || {
            let usage = exit_handle.state::<Usage>();
            usage.update(UpdateStep::Installed);
            usage.flush();
            exit_handle.state::<crate::Stats>().flush();
        })
        .build()
        .map_err(|e| e.to_string())
}

/// Fetch the channel's manifest and apply its flags and message when they changed. Skipped
/// when both `DisableUpdates` and `DisableRemoteConfig` are set. A manifest that isn't believed
/// leaves the last good one in place and reports the failed refresh.
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
            Err(format!("{why}; keeping the last good release list."))
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
/// time; a second caller gets the current status. The automatic interval never blocks a manual
/// check.
pub async fn check(app: &AppHandle, manual: bool) -> UpdateStatus {
    let updates = app.state::<Updates>();
    if app.state::<crate::AppInfo>().managed.disable_updates {
        return set_status(
            app,
            UpdateStatus::Error {
                phase: UpdatePhase::Check,
                retry: false,
                message: MANAGED.into(),
            },
        );
    }
    let min_secs = u64::from(app.state::<Remote>().flags().min_check_interval_h) * HOUR_SECS;
    if !manual && !updates.background_due(now_secs(), min_secs) {
        // Restarted within the hour: the background check waits for the next round.
        return updates.status();
    }
    if updates.busy.swap(true, Ordering::AcqRel) {
        return updates.status();
    }
    let current = updates.status();
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
    {
        let mut retry = updates.retry.lock().unwrap();
        if result.is_ok() {
            updates.mark_checked();
            *retry = None;
        } else {
            *retry = Some(Retry::after_failure(*retry, now_secs()));
        }
    }
    let status = match result {
        Ok(found) => found_update(app, found, &current).await,
        Err(message) if quiet => {
            eprintln!("update check: {message}");
            current
        }
        Err(message) => UpdateStatus::Error {
            phase: UpdatePhase::Check,
            retry: false,
            message,
        },
    };
    let status = set_status(app, status);
    updates.busy.store(false, Ordering::Release);
    status
}

/// What a successful check found. Downloads it when automatic updates are on.
async fn found_update(
    app: &AppHandle,
    found: Option<Found>,
    previous: &UpdateStatus,
) -> UpdateStatus {
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
    if updates.first_offer(&version) {
        app.state::<Usage>().update(UpdateStep::Offered);
    }
    let already = updates
        .downloaded
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|(_, hash)| *hash == found.expected.sha256)
        && matches!(previous, UpdateStatus::Ready { version: v, .. } if *v == version);
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
            app.state::<Usage>().update(UpdateStep::Downloaded);
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
            app.state::<Usage>().update(UpdateStep::Failed);
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

/// A download kept from an earlier run may only be installed while the last verified manifest
/// still offers this version exactly that file. Without this, a copy that moved past it another
/// way (a newer setup run by hand) would install the older download when quit from the tray,
/// and so would a release paused or blocked since, until the first check of the run.
fn still_offered(
    manifest: Option<&manifest::Manifest>,
    current: &semver::Version,
    bucket: u8,
    pending: &PendingFile,
    now: time::OffsetDateTime,
) -> bool {
    let Some(manifest) = manifest.filter(|m| !m.expired(now)) else {
        return false;
    };
    let Some(offer) = manifest::eligible(manifest, current, bucket, &manifest::platform_key())
    else {
        return false;
    };
    let same_version = match (
        semver::Version::parse(&offer.release.version),
        semver::Version::parse(&pending.version),
    ) {
        (Ok(offered), Ok(downloaded)) => offered == downloaded,
        _ => false,
    };
    same_version
        && offer.platform.size == pending.size
        && manifest::parse_hex32(&offer.platform.sha256)
            .is_some_and(|hash| Some(hash) == manifest::parse_hex32(&pending.sha256))
}

/// Pick up an update downloaded before unflub last closed. Deletes it if the last verified
/// manifest no longer offers it ([`still_offered`]), if it is no longer accepted, or if it fails
/// verification. The next check deletes it too if the manifest stops offering it.
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
        let current = app.package_info().version.clone();
        let channel = crate::current_settings(app).channel.as_str();
        let offered = still_offered(
            app.state::<Remote>().manifest(channel).as_ref(),
            &current,
            app.state::<net::Rollout>().bucket(&current.to_string()),
            &pending,
            time::OffsetDateTime::now_utc(),
        );
        if !offered {
            return None;
        }
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
        Some((path, hash)) => {
            let bytes = read_verified(&path, &hash);
            if bytes.is_err() {
                updates.discard_download();
            }
            bytes
        }
        None => fetch_verified(&found).await,
    };
    let result = bytes.and_then(|bytes| found.update.install(bytes).map_err(|e| e.to_string()));
    if let Err(message) = result {
        app.state::<Usage>().update(UpdateStep::Failed);
        *updates.update.lock().unwrap() = Some(found);
        set_status(
            app,
            UpdateStatus::Error {
                phase: UpdatePhase::Install,
                message: message.clone(),
                retry: true,
            },
        );
        updates.busy.store(false, Ordering::Release);
        return Err(message);
    }
    // The installer ran; don't offer the same file again.
    updates.discard_download();
    updates.busy.store(false, Ordering::Release);
    if restart {
        // Only reached on macOS and Linux; Windows has exited by now.
        app.restart()
    }
    Ok(())
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
                if app
                    .state::<Updates>()
                    .background_due(now_secs(), DAY_SECS + jitter)
                {
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

    #[test]
    fn installer_notice_is_consumed_once_and_requires_the_running_version() {
        let dir = std::env::temp_dir().join(format!("unflub-notice-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("updated-version");
        fs::write(&path, "0.2.0").unwrap();
        assert!(!take_install_notice(&dir, "0.1.0"));
        assert!(!path.exists());
        fs::write(&path, "0.2.0").unwrap();
        assert!(take_install_notice(&dir, "0.2.0"));
        assert!(!take_install_notice(&dir, "0.2.0"));
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn failed_checks_retry_before_the_daily_interval() {
        let mut retry = None;
        for delay in [600, 1200, 2400, 2400] {
            let failure = Retry::after_failure(retry, 100);
            assert_eq!(failure.delay, delay);
            assert!(!waited(
                Some(failure.failed_at),
                100 + delay - 1,
                failure.delay
            ));
            assert!(waited(Some(failure.failed_at), 100 + delay, failure.delay));
            retry = Some(failure);
        }
    }

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

    /// A download kept from an earlier run is installed only while the manifest still offers
    /// that exact file to this version.
    #[test]
    fn a_kept_download_must_still_be_offered() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        let hash = "ab".repeat(32);
        let listing = |rollout: u32, blocked: &str, expires: &str| -> manifest::Manifest {
            let releases = format!(
                r#""releases":[{{"version":"0.2.0","rollout_percent":{rollout},"platforms":{{"{}":
                    {{"url":"https://dl.rmgn.net/unflub/0.2.0/x.exe","signature":"s","sha256":"{hash}","size":3}}}}}}]"#,
                manifest::platform_key()
            );
            let json = manifest::tests::manifest_json("2026-11-17T12:00:00Z", "")
                .replace(r#""releases":[]"#, &releases)
                .replace(r#""blocked":[]"#, &format!(r#""blocked":[{blocked}]"#))
                .replace("2999-01-01T00:00:00Z", expires);
            serde_json::from_str(&json).unwrap()
        };
        let live = listing(100, "", "2999-01-01T00:00:00Z");
        let pending = |version: &str, sha256: &str| PendingFile {
            version: version.into(),
            file: format!("unflub-{version}.update"),
            raw_json: serde_json::Value::Null,
            sha256: sha256.into(),
            size: 3,
            critical: false,
        };
        let now = time::OffsetDateTime::now_utc();
        let kept = pending("0.2.0", &hash);
        assert!(still_offered(Some(&live), &v("0.1.0"), 50, &kept, now));
        assert!(
            !still_offered(Some(&live), &v("0.3.0"), 50, &kept, now),
            "a newer copy was installed by hand: never go back to the download"
        );
        assert!(
            !still_offered(
                Some(&listing(0, "", "2999-01-01T00:00:00Z")),
                &v("0.1.0"),
                50,
                &kept,
                now
            ),
            "paused since the download"
        );
        assert!(
            !still_offered(
                Some(&live),
                &v("0.1.0"),
                50,
                &pending("0.2.0", &"cd".repeat(32)),
                now
            ),
            "another file than the one the manifest names"
        );
        assert!(
            !still_offered(None, &v("0.1.0"), 50, &kept, now),
            "no verified manifest"
        );
        assert!(
            !still_offered(
                Some(&listing(100, "", "2020-01-01T00:00:00Z")),
                &v("0.1.0"),
                50,
                &kept,
                now
            ),
            "an expired manifest offers nothing"
        );
        assert!(
            still_offered(
                Some(&listing(100, r#""0.3.0""#, "2999-01-01T00:00:00Z")),
                &v("0.3.0"),
                50,
                &kept,
                now
            ),
            "a rollback the manifest chose survives a restart"
        );
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
                phase: UpdatePhase::Check,
                message: "offline".into(),
                retry: false,
            }),
            r#"{"state":"error","phase":"check","message":"offline","retry":false}"#
        );
    }
}
