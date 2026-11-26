#[cfg(windows)]
mod control_cli;
#[cfg(windows)]
pub use control_cli::dispatch as dispatch_cli;
mod controls;
mod crash;
mod diagnostics;
mod engine;
mod facts;
mod files;
mod install;
mod keys;
mod links;
mod managed;
mod manifest;
mod migrations;
mod net;
mod remote;
mod settings;
mod stats;
mod strings;
mod system;
mod tour;
mod undo_counts;
mod updates;
mod usage;

use crash::Crashes;
use engine::{EngineHost, Observed};
use install::InstallSource;
use managed::Managed;
use remote::{Remote, RemoteMessage};
use settings::{Settings, Store};
use stats::{Stats, StatsView};
use std::sync::{Arc, Mutex};
use tauri::{
    image::Image,
    menu::{
        CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder,
    },
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_opener::OpenerExt;
use updates::{UpdateStatus, Updates};
use usage::{Feature, Usage};

const SETTINGS_WINDOW: &str = "settings";
const TRAY_ID: &str = "main";
/// Passed by the start-at-login entry so a login launch stays in the tray.
const BACKGROUND_ARG: &str = "--background";
/// Asks an open settings window to switch page.
const OPEN_PAGE_EVENT: &str = "open-page";
/// `--perf-idle <dir>`: for perf/idle.ps1. Settings and data live in `<dir>`, every network
/// feature is off as if by machine policy, no other copy of unflub is contacted (no single
/// instance), no window opens, nothing registers at login, and the engine runs `--dry-run`
/// (its hook only watches: nothing is injected or swallowed).
const PERF_IDLE_ARG: &str = "--perf-idle";

struct AppState {
    store: Store,
    settings: Mutex<Settings>,
    settings_file_error: Mutex<Option<String>>,
    toggle: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    controls: controls::Controls,
    /// Hook, engine and pill. Taken out on quit.
    engine: Mutex<Option<EngineHost>>,
    /// Words learned by undo since the settings window last saved. The window may still hold
    /// settings from before they were learned, so its next save gets them merged back in.
    pending_learned: Mutex<Vec<String>>,
    /// The engine paused itself because another program interferes with keyboard input.
    interference: Mutex<Interference>,
    /// Autocorrect has no working engine session yet.
    hook_unavailable: std::sync::atomic::AtomicBool,
    /// The user typed into an administrator window since unflub started, where it can't fix
    /// anything: the tray tooltip says so for the rest of the session (#132).
    elevated_seen: std::sync::atomic::AtomicBool,
    stopping: std::sync::atomic::AtomicBool,
}

/// See [`AppState::interference`]. Each start is also counted in the anonymous usage stats as
/// `health.self_pauses`.
#[derive(Debug, Default)]
struct Interference {
    active: bool,
    /// Times it started since unflub started.
    times: u64,
}

/// Emitted with `true`/`false` when the engine pauses itself or resumes.
const INTERFERENCE_EVENT: &str = "input-interference";

/// Facts about this copy of unflub, fixed at startup.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Who updates it (`install.rs`).
    pub install_source: InstallSource,
    /// Machine policy (`managed.rs`).
    pub managed: Managed,
    /// Runs from an installed location, so it may add itself to start at login
    /// (`install::login_allowed`).
    pub login_allowed: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemLook {
    accent: Option<String>,
    platform: &'static str,
}

/// The report bodies shown in Privacy.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SendPreview {
    stats: Option<String>,
    last_stats: Option<String>,
}

#[derive(serde::Serialize)]
struct CrashPreview {
    json: String,
    pending: bool,
}

/// Settings as the window sees them.
fn for_window(settings: &Settings) -> serde_json::Value {
    serde_json::to_value(settings).unwrap_or_default()
}

fn emit_settings(app: &AppHandle, settings: &Settings) {
    let _ = app.emit("settings-changed", for_window(settings));
}

#[tauri::command]
fn get_settings(app: AppHandle) -> serde_json::Value {
    // Task Manager may have turned the login entry off since the window last looked.
    sync_login_truth(&app);
    for_window(&current_settings(&app))
}

/// `removed`: personal words the user removed in the window since its last save. A word the
/// engine learned and the window showed is then not merged back in.
#[tauri::command]
fn set_settings(
    app: AppHandle,
    mut settings: serde_json::Map<String, serde_json::Value>,
    removed: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let removed = removed.unwrap_or_default();
    let mut pending = Vec::new();
    let mut added = 0;
    // Where each word came from is the app's record, never the window's.
    settings.remove("wordSources");
    let changes_words = settings.contains_key("personalWords");
    let saved = update_settings_checked(&app, |current| {
        let before = changes_words.then(|| current.clone());
        current.patch(settings, false)?;
        if let Some(before) = before {
            pending = state.pending_learned.lock().unwrap().clone();
            added = words_added(&before.personal_words, &current.personal_words, &pending);
            forget_removed(&mut pending, &removed);
            merge_learned(&mut current.personal_words, &pending);
            sync_word_sources(&before, current, &pending);
        }
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    app.state::<Usage>().personal_words_added(added);
    // Saved to disk now; words learned during the save stay pending.
    if changes_words {
        let mut learned = state.pending_learned.lock().unwrap();
        forget_removed(&mut learned, &removed);
        learned.retain(|w| !pending.contains(w));
    }
    Ok(for_window(&saved))
}

#[tauri::command]
fn reset_settings(app: AppHandle) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let info = *app.state::<AppInfo>();
    let saved = update_settings_checked(&app, |current| {
        // A reset is about preferences: the welcome isn't shown again, and usage stats and
        // crash reports stay as the user set them (an off is never turned back on).
        let mut defaults = Settings::default();
        if info.managed.disable_updates {
            defaults.auto_update = current.auto_update;
            defaults.channel = current.channel;
        }
        if !info.login_allowed {
            defaults.start_at_login = current.start_at_login;
        }
        *current = Settings {
            schema_version: current.schema_version,
            onboarded: current.onboarded,
            share_usage_stats: current.share_usage_stats,
            send_crash_reports: current.send_crash_reports,
            extra: std::mem::take(&mut current.extra),
            ..defaults
        };
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    state.pending_learned.lock().unwrap().clear();
    Ok(for_window(&saved))
}

#[tauri::command]
fn pause_controls(app: AppHandle) -> controls::Status {
    controls::status(&app)
}

#[tauri::command]
fn resume_autocorrect(app: AppHandle) {
    controls::set_pause(&app, controls::Pause::Running);
}

#[tauri::command]
fn set_pause_shortcut(app: AppHandle, binding: String) -> Result<(), &'static str> {
    let normalised = controls::normalise_shortcut(&binding).ok_or("invalid")?;
    let retry = current_settings(&app).pause_shortcut == normalised;
    update_settings_checked(&app, |s| {
        s.pause_shortcut = normalised;
        Ok(())
    })
    .map_err(|_| "save")?;
    #[cfg(windows)]
    if retry {
        queue_ui_refresh(&app, controls::sync_shortcut);
    }
    #[cfg(not(windows))]
    let _ = retry;
    Ok(())
}

fn sync_word_sources(current: &Settings, next: &mut Settings, pending: &[String]) {
    use settings::word_key;
    let known: std::collections::HashSet<_> =
        current.personal_words.iter().map(|w| word_key(w)).collect();
    let learned: std::collections::HashSet<_> = pending.iter().map(|w| word_key(w)).collect();
    let today = stats::today();
    let present: std::collections::HashSet<_> =
        next.personal_words.iter().map(|w| word_key(w)).collect();
    next.word_sources = current.word_sources.clone();
    // A removed word's source goes with it (a live save must already be clean).
    next.word_sources.retain(|key, _| present.contains(key));
    for word in &next.personal_words {
        let key = word_key(word);
        if !known.contains(&key) {
            let day = learned.contains(&key).then_some(today);
            next.word_sources.insert(key, day);
        }
    }
}

/// How many of `next` the user added in the window: not in `prev`, and not one of the words
/// learned by undo that are being merged in (ignoring case).
fn words_added(prev: &[String], next: &[String], learned: &[String]) -> u64 {
    let known: std::collections::HashSet<String> = prev
        .iter()
        .chain(learned)
        .map(|w| settings::word_key(w))
        .collect();
    let mut added = std::collections::HashSet::new();
    for w in next {
        let key = settings::word_key(w);
        if !known.contains(&key) {
            added.insert(key);
        }
    }
    added.len() as u64
}

/// Drop from `pending` every word the user removed (ignoring case).
fn forget_removed(pending: &mut Vec<String>, removed: &[String]) {
    pending.retain(|w| {
        let key = settings::word_key(w);
        !removed.iter().any(|r| settings::word_key(r) == key)
    });
}

/// Add each of `learned` to `words` unless it is already there (ignoring case).
fn merge_learned(words: &mut Vec<String>, learned: &[String]) {
    for word in learned {
        let key = settings::word_key(word);
        if !words.iter().any(|w| settings::word_key(w) == key) {
            words.push(word.clone());
        }
    }
}

#[tauri::command]
fn update_status(app: AppHandle) -> UpdateStatus {
    app.state::<Updates>().status()
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> UpdateStatus {
    updates::check(&app, true).await
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    updates::install(&app, true).await
}

#[tauri::command]
fn system_look() -> SystemLook {
    SystemLook {
        accent: system::accent_color(),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
fn send_preview(app: AppHandle) -> SendPreview {
    let settings = effective_settings(&app);
    let version = app.package_info().version.to_string();
    let usage = app.state::<Usage>();
    SendPreview {
        stats: settings
            .share_usage_stats
            .then(|| pretty(&usage.payload(&settings, &version, &env_facts(&app)))),
        last_stats: settings
            .share_usage_stats
            .then(|| usage.last_payload().as_ref().map(pretty))
            .flatten(),
    }
}

#[tauri::command]
fn crash_preview(app: AppHandle) -> Result<CrashPreview, &'static str> {
    let (event, pending) = crash::preview(&app.state::<Crashes>())?;
    Ok(CrashPreview {
        json: pretty(&event),
        pending,
    })
}

/// What the settings window reports for the usage stats. A closed list: anything else is
/// refused before it reaches this function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum UsageEvent {
    /// The Stats page was opened.
    StatsPageView,
    /// The tour was started again (not the first-run one).
    TourReplay,
    /// The first-run tour was finished, or skipped (kept as `env.tour`).
    TourCompleted,
    TourSkipped,
    /// The user put off an offered update ("Later" or closing its notice).
    UpdateDismissed,
}

/// `invoke("usage_event", { event: "stats_page_view" })`. Counted only while usage stats are
/// on; the tour's outcome is kept either way (`facts.rs`).
#[tauri::command]
fn usage_event(app: AppHandle, event: UsageEvent) {
    let usage = app.state::<Usage>();
    match event {
        UsageEvent::StatsPageView => usage.feature(Feature::StatsPageView),
        UsageEvent::TourReplay => usage.feature(Feature::TourReplay),
        UsageEvent::TourCompleted => app
            .state::<facts::Facts>()
            .set_tour(facts::TourState::Completed),
        UsageEvent::TourSkipped => app
            .state::<facts::Facts>()
            .set_tour(facts::TourState::Skipped),
        UsageEvent::UpdateDismissed => usage.update(usage::UpdateStep::Dismissed),
    }
}

/// The facts the usage stats' `env` group is bucketed from, as they are now.
pub(crate) fn env_facts(app: &AppHandle) -> usage::EnvFacts {
    let today = facts::day_of(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    );
    let facts = app.state::<facts::Facts>();
    usage::EnvFacts {
        install_source: app.state::<AppInfo>().install_source,
        days_since_install: facts.days_since_install(today),
        windows_build: system::windows_build(),
        keyboard_langid: system::keyboard_langid(),
        cpu_cores: std::thread::available_parallelism().ok().map(|n| n.get()),
        ram_gb: system::installed_ram_gb(),
        tour: facts.tour(),
    }
}

/// Best-effort idle sample. Avoid an open Settings window and do not wait for a busy supervisor.
fn sample_idle_memory(app: &AppHandle) -> Option<u64> {
    if app.get_webview_window(SETTINGS_WINDOW).is_some() || !system::input_idle() {
        return None;
    }
    #[cfg(windows)]
    {
        let receiver = app
            .state::<AppState>()
            .engine
            .lock()
            .ok()?
            .as_ref()?
            .sample_memory();
        let sample = receiver
            .recv_timeout(std::time::Duration::from_millis(100))
            .ok()?;
        if app.get_webview_window(SETTINGS_WINDOW).is_none() && system::input_idle() {
            return sample;
        }
    }
    None
}

#[tauri::command]
fn tour_reset(app: AppHandle) {
    app.state::<tour::Tour>().reset();
}

/// Async so it runs on a runtime worker, not the main thread: a panic in the practice engine
/// then keeps only its location (`crash.rs` keeps messages from `main`).
#[tauri::command]
async fn tour_key(app: AppHandle, key: String) -> tour::TourAction {
    app.state::<tour::Tour>().key(&key)
}

#[tauri::command]
fn app_info(app: AppHandle) -> AppInfo {
    *app.state::<AppInfo>()
}

fn pretty<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

#[tauri::command]
fn stats_view(app: AppHandle) -> StatsView {
    app.state::<Stats>().view()
}

#[tauri::command]
fn clear_stats(app: AppHandle) -> StatsView {
    let stats = app.state::<Stats>();
    stats.clear();
    // The undo counts are typed words too: gone from disk here, and from the running engine
    // with the next settings it gets (a new epoch).
    app.state::<Arc<undo_counts::UndoCounts>>().clear();
    if let Some(engine) = app.state::<AppState>().engine.lock().unwrap().as_ref() {
        engine.apply(&current_settings(&app));
    }
    stats.view()
}

#[tauri::command]
fn crash_report_pending(app: AppHandle) -> bool {
    app.state::<Crashes>().pending()
}

#[tauri::command]
fn answer_crash_report(app: AppHandle, answer: crash::Answer) {
    crash::answer(&app, answer);
}

#[tauri::command]
fn input_interference(app: AppHandle) -> bool {
    app.state::<AppState>().interference.lock().unwrap().active
}

/// The engine paused itself (`active`) or works again: tray, settings window.
fn set_interference(app: &AppHandle, active: bool) {
    let state = app.state::<AppState>();
    {
        let mut i = state.interference.lock().unwrap();
        if i.active == active {
            return;
        }
        i.active = active;
        if active {
            i.times += 1;
            eprintln!(
                "autocorrect: paused itself, another program is interfering with keyboard input ({} times)",
                i.times
            );
        }
    }
    refresh_tray(app);
    let _ = app.emit(INTERFERENCE_EVENT, active);
}

#[tauri::command]
fn remote_message(app: AppHandle) -> Option<RemoteMessage> {
    app.state::<Remote>().message()
}

#[tauri::command]
fn dismiss_message(app: AppHandle, id: String) {
    app.state::<Remote>().dismiss(&id);
    let _ = app.emit(remote::MESSAGE_EVENT, Option::<RemoteMessage>::None);
    refresh_tray_menu(&app);
}

fn current_settings(app: &AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
}

/// The settings as they take effect, with machine policy applied.
fn effective_settings(app: &AppHandle) -> Settings {
    app.state::<AppInfo>()
        .managed
        .effective(current_settings(app))
}

/// Change the settings and save them, sync side effects (login item, tray, privacy switches),
/// then tell every window. `change` gets the latest settings, and changes never overlap
/// (`Store::update`), so a word learned while the window saves isn't lost.
fn update_settings(
    app: &AppHandle,
    change: impl FnOnce(&mut Settings),
) -> std::io::Result<Settings> {
    let state = app.state::<AppState>();
    let (prev, next) = state
        .store
        .update(&state.settings, change)
        .inspect_err(|e| {
            diagnostics::record(diagnostics::Event::SettingsSaveFailed(e.kind()));
        })?;

    settings_changed(app, &prev, &next);
    Ok(next)
}

fn update_settings_checked(
    app: &AppHandle,
    change: impl FnOnce(&mut Settings) -> std::io::Result<()>,
) -> std::io::Result<Settings> {
    let state = app.state::<AppState>();
    let (prev, next) = state
        .store
        .update_checked(&state.settings, *app.state::<AppInfo>(), change)
        .inspect_err(|e| diagnostics::record(diagnostics::Event::SettingsSaveFailed(e.kind())))?;
    settings_changed(app, &prev, &next);
    Ok(next)
}

fn settings_changed(app: &AppHandle, prev: &Settings, next: &Settings) {
    if prev.personal_words != next.personal_words {
        let before: std::collections::HashSet<_> = prev
            .personal_words
            .iter()
            .map(|w| settings::word_key(w))
            .collect();
        let after: std::collections::HashSet<_> = next
            .personal_words
            .iter()
            .map(|w| settings::word_key(w))
            .collect();
        app.state::<AppState>()
            .pending_learned
            .lock()
            .unwrap()
            .retain(|word| {
                let key = settings::word_key(word);
                !before.contains(&key) || after.contains(&key)
            });
    }
    if prev.start_at_login != next.start_at_login {
        sync_autostart(app, next.start_at_login);
    }
    #[cfg(windows)]
    if prev.pause_shortcut != next.pause_shortcut {
        queue_ui_refresh(app, controls::sync_shortcut);
    }
    if prev.share_usage_stats != next.share_usage_stats {
        let managed = app.state::<AppInfo>().managed;
        app.state::<Usage>()
            .set_on(managed.effective(next.clone()).share_usage_stats);
    }
    if prev.theme != next.theme {
        app.state::<Usage>().feature(Feature::ThemeChange);
    }
    if prev.stats_keep_words && !next.stats_keep_words {
        app.state::<Stats>().forget_words();
    }
    let counts = app.state::<Arc<undo_counts::UndoCounts>>();
    if prev.stats_keep_words != next.stats_keep_words {
        counts.set_persist(next.stats_keep_words);
    }
    if prev.learn_after_undos > 0 && next.learn_after_undos == 0 {
        // Learning off: the counts have no use left (the engine forgets its own below).
        counts.clear();
    }
    if prev.enabled != next.enabled {
        controls::set_pause(app, controls::Pause::Running);
    } else {
        apply_correction(app);
    }
    emit_settings(app, &current_settings(app));
}

const SETTINGS_FILE_EVENT: &str = "settings-file-error";

#[tauri::command]
fn settings_file_error(app: AppHandle) -> Option<String> {
    app.state::<AppState>()
        .settings_file_error
        .lock()
        .unwrap()
        .clone()
}

fn report_settings_file_error(app: &AppHandle, error: Option<String>) {
    let state = app.state::<AppState>();
    let mut current = state.settings_file_error.lock().unwrap();
    if *current == error {
        return;
    }
    if let Some(error) = &error {
        eprintln!("settings: {error}");
    }
    current.clone_from(&error);
    drop(current);
    let _ = app.emit(SETTINGS_FILE_EVENT, error);
}

fn watch_settings(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let state = app.state::<AppState>();
            if state.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            match state.store.reload_pending() {
                Ok(true) => queue_ui_refresh(&app, |app| {
                    let state = app.state::<AppState>();
                    if state.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    match state.store.reload(&state.settings, *app.state::<AppInfo>()) {
                        Ok(Some((prev, next))) => {
                            if prev != next {
                                settings_changed(app, &prev, &next);
                            }
                            report_settings_file_error(app, None);
                        }
                        Ok(None) => {}
                        Err(e) => report_settings_file_error(app, Some(e.to_string())),
                    }
                }),
                Ok(false) => {}
                Err(e) => report_settings_file_error(
                    &app,
                    Some(format!("Couldn't check settings.json. {e}")),
                ),
            }
        }
    });
}

fn correction_enabled(app: &AppHandle) -> bool {
    current_settings(app).enabled && !controls::paused(app)
}

/// Take the engine lock before reading the latest settings so two callers cannot send those
/// settings out of order. The temporary pause changes the engine's copy only.
fn apply_correction(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        let mut settings = current_settings(app);
        settings.enabled &= !controls::paused(app);
        engine.apply(&settings);
    };
}

fn refresh_correction_ui(app: &AppHandle) {
    let enabled = correction_enabled(app);
    app.state::<Usage>().set_paused(!enabled);
    if !enabled {
        set_interference(app, false);
    }
    queue_ui_refresh(app, |app| {
        refresh_tray_on_main_thread(app);
        refresh_window_icon(app, correction_enabled(app));
        controls::emit(app);
    });
}

fn sync_correction(app: &AppHandle) {
    apply_correction(app);
    refresh_correction_ui(app);
}

/// The manifest's flags or message changed: pass the flags on and show the message.
fn apply_remote(app: &AppHandle) {
    let remote = app.state::<Remote>();
    if let Some(engine) = app.state::<AppState>().engine.lock().unwrap().as_ref() {
        engine.set_flags(remote.flags());
    }
    let _ = app.emit(remote::MESSAGE_EVENT, remote.message());
    refresh_tray_menu(app);
}

/// Undoing a fix taught the engine `word`: keep it in the personal dictionary for next time.
fn learn_word(app: &AppHandle, word: String) {
    app.state::<Usage>().learned_pair();
    // Learned: its undo count has done its job.
    app.state::<Arc<undo_counts::UndoCounts>>().forget(&word);
    let state = app.state::<AppState>();
    merge_learned(
        &mut state.pending_learned.lock().unwrap(),
        std::slice::from_ref(&word),
    );
    let key = settings::word_key(&word);
    let known = |s: &Settings| {
        s.personal_words
            .iter()
            .any(|w| settings::word_key(w) == key)
    };
    if known(&current_settings(app)) {
        return;
    }
    let learned = update_settings(app, |s| {
        if !known(s) {
            s.personal_words.push(word);
            s.word_sources.insert(key.clone(), Some(stats::today()));
        }
    });
    if let Err(e) = learned {
        eprintln!("saving a learned word: {e}");
    }
}

/// Fixes and undos from the engine thread go to the local stats and the usage counts. Words
/// only reach the local stats (and only with "Keep a list of fixed words" on).
fn observe(app: &AppHandle, event: Observed) {
    let usage = app.state::<Usage>();
    match event {
        Observed::Corrected {
            original,
            corrected,
            user_rule,
            decide,
            inject,
            pill,
            keep_word,
            trigger,
            source,
            category,
        } => {
            let settings = current_settings(app);
            usage.correction(&usage::Fix {
                decide,
                inject,
                pill,
                trigger,
                source,
                category,
            });
            app.state::<Stats>().correction(
                &original,
                &corrected,
                keep_word_in_stats(settings.stats_keep_words, user_rule, keep_word),
            );
        }
        Observed::Undone {
            original,
            corrected,
            user_rule,
            keep_word,
        } => {
            usage.undo();
            app.state::<Stats>().undo(&original, &corrected);
            if records_undo(
                current_settings(app).learn_after_undos,
                user_rule,
                keep_word,
            ) {
                app.state::<Arc<undo_counts::UndoCounts>>()
                    .record(&original, &corrected);
            }
        }
        Observed::UndoArmed => usage.undo_armed(),
        Observed::CorrectorPanicked => {
            diagnostics::record(diagnostics::Event::CorrectorPanicked);
            usage.crash();
        }
        Observed::Diagnostic(event) => diagnostics::record(diagnostics::Event::Engine(event)),
        Observed::HookReinstalled { reason } => {
            diagnostics::record(diagnostics::Event::HookReinstalled(reason));
            usage.hook_reinstall(reason);
        }
        Observed::HookStartFailed => usage.hook_unavailable(),
        Observed::UsageCounts(counts) => usage.engine_counts(&counts),
        Observed::InputInterference { active } => {
            diagnostics::record(diagnostics::Event::InputInterference(active));
            if active {
                usage.self_pause();
            }
            set_interference(app, active);
        }
        Observed::HookUnavailable { active } => {
            if active {
                set_interference(app, false);
            }
            // Each hook failure is counted separately; this state only drives the tray.
            app.state::<AppState>()
                .hook_unavailable
                .store(active, std::sync::atomic::Ordering::Relaxed);
            refresh_tray(app);
        }
        Observed::ElevatedWindow => {
            // Once per session: the tooltip changes, nothing pops up.
            let first = !app
                .state::<AppState>()
                .elevated_seen
                .swap(true, std::sync::atomic::Ordering::Relaxed);
            if first {
                refresh_tray(app);
            }
        }
        Observed::EngineCrashed => {
            set_interference(app, false);
            usage.crash();
            // A panic left a marker for the opt-in crash report; other exits leave none.
            crash::take_engine_report(app);
        }
    }
}

/// Is an undone pair counted toward learning? Only while learning is on, never for the user's
/// own shortcuts, and never where the engine says the word may be a password (terminals).
fn records_undo(learn_after_undos: u8, user_rule: bool, engine_allows: bool) -> bool {
    learn_after_undos > 0 && !user_rule && engine_allows
}

/// Does the Stats page keep a fix's typo and word? Only with "Keep a list" on, never for the
/// user's own shortcuts, and never where the engine says the word may be a password (terminals).
fn keep_word_in_stats(keep_words_setting: bool, user_rule: bool, engine_allows: bool) -> bool {
    keep_words_setting && !user_rule && engine_allows
}

/// The directory after `--perf-idle`, if given.
fn perf_idle_dir(args: impl IntoIterator<Item = String>) -> Option<std::path::PathBuf> {
    let mut args = args.into_iter();
    args.find(|a| a == PERF_IDLE_ARG)?;
    args.next().filter(|d| !d.is_empty()).map(Into::into)
}

/// Unhook and stop the engine. Safe to call more than once.
fn stop_engine(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Take it out first: the engine thread may call update_settings while it winds down.
    let engine = state.engine.lock().unwrap().take();
    if let Some(engine) = engine {
        engine.shutdown();
    }
    // Keep today's counts.
    if let Some(stats) = app.try_state::<Stats>() {
        stats.flush();
    }
    if let Some(usage) = app.try_state::<Usage>() {
        usage.flush();
    }
}

fn stop_on_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    stop_engine(app);
}

fn sync_autostart(app: &AppHandle, on: bool) {
    // A dev build must never register itself to run at login, and neither must a copy outside
    // an installed location (a release build in a checkout, a portable copy).
    if cfg!(debug_assertions) || !app.state::<AppInfo>().login_allowed {
        eprintln!("start at login: skipped outside an installed copy (would be {on})");
        return;
    }
    let launcher = app.autolaunch();
    let result = if on {
        launcher.enable()
    } else {
        launcher.disable()
    };
    if let Err(e) = result {
        eprintln!("start at login: {e}");
    }
}

/// Make `start_at_login` say what the OS will do at the next login. The OS is the truth: an
/// entry removed outside unflub, or switched off in Task Manager or Settings > Apps > Startup
/// (`StartupApproved\Run`, which `is_enabled` reads), is the user's choice and is never written
/// back, at startup or later; only the toggle turns it on again. Dev builds and copies outside an
/// installed location never register, so they leave the setting alone.
fn sync_login_truth(app: &AppHandle) {
    if cfg!(debug_assertions) || !app.state::<AppInfo>().login_allowed {
        return;
    }
    let wanted = current_settings(app).start_at_login;
    let actual = match app.autolaunch().is_enabled() {
        Ok(on) => on,
        Err(e) => {
            eprintln!("start at login: {e}");
            return;
        }
    };
    if actual == wanted {
        return;
    }
    // Record the OS's answer without writing it back, so a Task Manager "Disable" stays
    // re-enableable from Task Manager.
    let state = app.state::<AppState>();
    match state
        .store
        .update(&state.settings, |s| s.start_at_login = actual)
    {
        Ok((_, next)) => emit_settings(app, &next),
        Err(e) => eprintln!("start at login: {e}"),
    }
}

fn tray_image(enabled: bool) -> Image<'static> {
    let dark = system::taskbar_is_dark();
    let bytes: &'static [u8] = match (enabled, dark) {
        (true, true) => include_bytes!("../icons/tray/tray-on-dark.png"),
        (true, false) => include_bytes!("../icons/tray/tray-on-light.png"),
        (false, true) => include_bytes!("../icons/tray/tray-paused-dark.png"),
        (false, false) => include_bytes!("../icons/tray/tray-paused-light.png"),
    };
    Image::from_bytes(bytes).expect("bundled tray icon is a valid PNG")
}

fn tooltip(
    enabled: bool,
    interference: bool,
    hook_unavailable: bool,
    elevated_seen: bool,
) -> &'static str {
    match (enabled, interference, hook_unavailable) {
        (false, _, _) => strings::TOOLTIP_PAUSED,
        (true, _, true) => strings::TOOLTIP_HOOK_UNAVAILABLE,
        (true, true, false) => strings::TOOLTIP_INTERFERENCE,
        (true, false, false) if elevated_seen => strings::TOOLTIP_ELEVATED,
        (true, false, false) => strings::TOOLTIP,
    }
}

fn refresh_tray(app: &AppHandle) {
    queue_ui_refresh(app, refresh_tray_on_main_thread);
}

fn refresh_tray_on_main_thread(app: &AppHandle) {
    let enabled = correction_enabled(app);
    let state = app.state::<AppState>();
    let interference = state.interference.lock().unwrap().active;
    let hook_unavailable = state
        .hook_unavailable
        .load(std::sync::atomic::Ordering::Relaxed);
    let elevated_seen = state
        .elevated_seen
        .load(std::sync::atomic::Ordering::Relaxed);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // Paused by itself, or not working yet, looks paused.
        let _ = tray.set_icon(Some(tray_image(
            enabled && !interference && !hook_unavailable,
        )));
        let _ = tray.set_tooltip(Some(tooltip(
            enabled,
            interference,
            hook_unavailable,
            elevated_seen,
        )));
    }
    if let Some(item) = state.toggle.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
    if let Some(menu) = state.controls.pause_menu.lock().unwrap().as_ref() {
        let _ = menu.set_enabled(current_settings(app).enabled);
    };
}

/// The settings window's taskbar icon: the app icon, in greys while paused.
fn window_icon(enabled: bool) -> Image<'static> {
    let bytes: &'static [u8] = if enabled {
        include_bytes!("../icons/128x128@2x.png")
    } else {
        include_bytes!("../icons/icon-paused.png")
    };
    Image::from_bytes(bytes).expect("bundled window icon is a valid PNG")
}

fn refresh_window_icon(app: &AppHandle, enabled: bool) {
    if let Some(win) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = win.set_icon(window_icon(enabled));
    }
}

/// Open the settings window, on `page` if given (`?page=` for a new window, an event for an
/// open one).
fn open_settings(app: &AppHandle, page: Option<&str>) {
    app.state::<Usage>().feature(Feature::SettingsOpen);
    if let Some(win) = app.get_webview_window(SETTINGS_WINDOW) {
        if let Some(page) = page {
            let _ = win.emit(OPEN_PAGE_EVENT, page);
        }
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let url = match page {
        Some(page) => format!("index.html?page={page}"),
        None => "index.html".into(),
    };
    let builder = WebviewWindowBuilder::new(app, SETTINGS_WINDOW, WebviewUrl::App(url.into()))
        .title("unflub")
        .inner_size(880.0, 620.0)
        .min_inner_size(680.0, 520.0)
        .center();
    #[cfg(windows)]
    let builder = builder
        .transparent(true)
        .effects(tauri::utils::config::WindowEffectsConfig {
            effects: vec![tauri::window::Effect::Mica],
            ..Default::default()
        });
    match builder.build() {
        Ok(win) => {
            if !correction_enabled(app) {
                let _ = win.set_icon(window_icon(false));
            }
        }
        Err(e) => eprintln!("settings window: {e}"),
    }
}

fn open_url(app: &AppHandle, url: &str) {
    if !links::openable(url) {
        return;
    }
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        eprintln!("open {url}: {e}");
    }
}

/// The tray menu for the current state: an update or crash report waiting adds one item.
fn tray_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let enabled = correction_enabled(app);
    let toggle = CheckMenuItemBuilder::with_id("toggle", strings::MENU_AUTOCORRECT)
        .checked(enabled)
        .build(app)?;
    let pause = SubmenuBuilder::with_id(app, "pause", strings::MENU_PAUSE)
        .enabled(current_settings(app).enabled)
        .text("pause-15", strings::MENU_PAUSE_15)
        .text("pause-60", strings::MENU_PAUSE_60)
        .text("pause-restart", strings::MENU_PAUSE_RESTART)
        .build()?;
    let exclude = MenuItem::with_id(
        app,
        "exclude-app",
        strings::MENU_EXCLUDE_APP,
        false,
        None::<&str>,
    )?;
    let mut menu = MenuBuilder::new(app)
        .item(&toggle)
        .item(&pause)
        .item(&exclude)
        .separator();
    let update = app.state::<Updates>().status();
    match &update {
        UpdateStatus::Ready { version, .. } => {
            menu = menu.text("update", strings::menu_restart_to_update(version));
        }
        UpdateStatus::Available {
            version, critical, ..
        } => {
            menu = menu.text("update", strings::menu_update_available(version, *critical));
        }
        UpdateStatus::Error {
            phase: updates::UpdatePhase::Install,
            ..
        } => {
            menu = menu.text("update", strings::MENU_UPDATE_ERROR);
        }
        _ => {}
    }
    if app.state::<Crashes>().pending() {
        menu = menu.text("crash", strings::MENU_CRASH);
    }
    if let Some(message) = app.state::<Remote>().message() {
        menu = menu.text("message", strings::menu_message(&message.text));
    }
    let menu = menu
        .text("settings", strings::MENU_SETTINGS)
        .text("tour", strings::MENU_TOUR)
        .separator()
        .text("support", strings::MENU_SUPPORT)
        .text("star", strings::MENU_STAR)
        .separator();
    let menu = match update {
        UpdateStatus::Ready { .. } => menu.text("quit-update", strings::MENU_QUIT_UPDATE),
        UpdateStatus::Installing { .. } => menu.item(
            &MenuItemBuilder::with_id("installing", strings::MENU_INSTALLING)
                .enabled(false)
                .build(app)?,
        ),
        _ => menu.text("quit", strings::MENU_QUIT),
    }
    .build()?;
    let state = app.state::<AppState>();
    *state.toggle.lock().unwrap() = Some(toggle);
    *state.controls.pause_menu.lock().unwrap() = Some(pause);
    *state.controls.exclude_item.lock().unwrap() = Some(exclude);
    Ok(menu)
}

/// Rebuild the tray menu after the update or crash-report state changed.
fn refresh_tray_menu(app: &AppHandle) {
    queue_ui_refresh(app, rebuild_tray_menu);
}

fn queue_ui_refresh(app: &AppHandle, refresh: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = handle.state::<AppState>();
        if !state.stopping.load(std::sync::atomic::Ordering::Relaxed) {
            refresh(&handle);
        }
    });
}

fn rebuild_tray_menu(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match tray_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => eprintln!("tray menu: {e}"),
    }
}

fn quit(app: &AppHandle) {
    stop_on_exit(app);
    app.exit(0);
}

fn tray_install(app: &AppHandle, restart: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match updates::install(&app, restart).await {
            // The Windows updater exits after launching the installer.
            Ok(()) if !restart => quit(&app),
            Ok(()) => {}
            Err(e) => {
                eprintln!("update: {e}");
                open_settings(&app, Some("about"));
            }
        }
    });
}

fn build_tray(app: &AppHandle, enabled: bool) -> tauri::Result<()> {
    let menu = tray_menu(app)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(enabled && !cfg!(windows)))
        .tooltip(tooltip(enabled, false, cfg!(windows), false))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => controls::toggle_tray(app),
            "pause-15" => controls::pause_for(app, 15),
            "pause-60" => controls::pause_for(app, 60),
            "pause-restart" if current_settings(app).enabled => {
                controls::set_pause(app, controls::Pause::UntilRestart);
            }
            "exclude-app" => controls::exclude_foreground(app),
            "settings" => open_settings(app, None),
            "update" => match app.state::<Updates>().status() {
                UpdateStatus::Ready { .. } => tray_install(app, true),
                _ => open_settings(app, Some("about")),
            },
            "crash" | "message" => open_settings(app, Some("about")),
            "tour" => open_settings(app, Some("tour")),
            "support" => open_url(app, links::SPONSOR_URL),
            "star" => open_url(app, links::REPO_URL),
            "quit-update" => tray_install(app, false),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => open_settings(tray.app_handle(), None),
            TrayIconEvent::Enter { .. }
            | TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Down,
                ..
            } => controls::capture_foreground(tray.app_handle()),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    unflub_engine::disable_windows_crash_reporting();
    let perf_idle = perf_idle_dir(std::env::args());
    let mut builder = tauri::Builder::default();
    #[cfg(windows)]
    {
        builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
    }
    if perf_idle.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_settings(app, None)
        }));
    }
    let app = builder
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let (config_dir, data_dir, local_dir) = match &perf_idle {
                Some(dir) => (dir.clone(), dir.clone(), dir.clone()),
                None => (
                    app.path().app_config_dir()?,
                    app.path().app_data_dir()?,
                    app.path().app_local_data_dir()?,
                ),
            };
            let version = app.package_info().version.to_string();
            let install_source = InstallSource::detect();
            let info = match perf_idle {
                Some(_) => AppInfo {
                    install_source,
                    managed: Managed {
                        disable_updates: true,
                        disable_usage_stats: true,
                        disable_crash_reports: true,
                        disable_remote_config: true,
                    },
                    login_allowed: false,
                },
                None => AppInfo {
                    install_source,
                    managed: Managed::read(),
                    login_allowed: install::login_allowed(install_source),
                },
            };
            app.manage(info);
            // First, so a panic anywhere after this leaves a report.
            let (crashes, crashed_last_time) =
                crash::install(&data_dir, &version, info.managed.disable_crash_reports);
            app.manage(crashes);
            diagnostics::init(&local_dir);
            // Before 0.2 these were in the roaming folder with the settings (#120).
            files::move_machine_state(&config_dir, &local_dir, files::MACHINE_STATE);
            if data_dir != config_dir {
                files::move_machine_state(&data_dir, &local_dir, files::MACHINE_STATE);
            }
            app.manage(Updates::new(local_dir.clone(), local_dir.clone(), &version));
            app.manage(net::Rollout::new(local_dir.clone()));
            let store = Store::new(config_dir.clone());
            let first_run = !store.exists();
            let settings = store.load();
            app.manage(Remote::new(
                local_dir.clone(),
                info.managed.disable_remote_config,
                settings.channel.as_str(),
                app.package_info().version.clone(),
                app.state::<net::Rollout>().bucket(&version),
            ));
            let usage = Usage::new(
                local_dir.clone(),
                info.managed.effective(settings.clone()).share_usage_stats,
            );
            if crashed_last_time {
                usage.crash();
            }
            usage.start_paused(!settings.enabled);
            app.manage(usage);
            let today = facts::day_of(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            );
            app.manage(facts::Facts::load(
                local_dir.clone(),
                facts::first_run_day(first_run, &config_dir, today),
            ));
            let counts = Arc::new(undo_counts::UndoCounts::new(
                &local_dir,
                settings.stats_keep_words,
            ));
            app.manage(counts.clone());
            app.manage(Stats::new(local_dir.clone()));
            app.manage(tour::Tour::default());
            let enabled = settings.enabled;
            let start_at_login = settings.start_at_login;
            app.manage(AppState {
                store,
                settings: Mutex::new(settings.clone()),
                settings_file_error: Mutex::new(None),
                toggle: Mutex::new(None),
                controls: controls::Controls::default(),
                engine: Mutex::new(None),
                pending_learned: Mutex::new(Vec::new()),
                interference: Mutex::default(),
                hook_unavailable: Default::default(),
                elevated_seen: Default::default(),
                stopping: Default::default(),
            });
            let handle = app.handle();
            let learner = handle.clone();
            let observer_app = handle.clone();
            let observer: engine::Observer = Arc::new(move |event| observe(&observer_app, event));
            let launch = engine::Launch::beside_app(
                app.state::<Crashes>().engine_marker(),
                perf_idle.is_some(),
            );
            let engine = EngineHost::start(
                launch,
                counts,
                settings.clone(),
                app.state::<Remote>().flags(),
                app.state::<Stats>().words_checked.clone(),
                observer,
                move |word| learn_word(&learner, word),
            );
            *app.state::<AppState>().engine.lock().unwrap() = Some(engine);
            build_tray(handle, enabled)?;
            #[cfg(windows)]
            {
                controls::sync_shortcut(handle);
                if perf_idle.is_none() {
                    system::track_foreground();
                }
            }
            if first_run {
                let _ = app.state::<AppState>().store.save(&settings);
                sync_autostart(handle, start_at_login);
            } else {
                sync_login_truth(handle);
            }
            if perf_idle.is_none() {
                watch_settings(handle.clone());
                #[cfg(windows)]
                if let Err(e) = control_cli::start(handle, local_dir) {
                    eprintln!("settings CLI: {e}");
                }
            }
            // The engine may have crashed while unflub was closing.
            if crash::take_engine_report(handle) {
                app.state::<Usage>().crash();
            }
            crash::after_start(handle);
            if !cfg!(debug_assertions) {
                let remote_app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = updates::refresh_remote(&remote_app).await {
                        eprintln!("manifest: {e}");
                    }
                });
            }
            let updated = matches!(
                handle.state::<Updates>().status(),
                UpdateStatus::Updated { .. }
            );
            updates::spawn_background(handle.clone());
            usage::spawn(handle.clone());
            let background = std::env::args().any(|a| a == BACKGROUND_ARG);
            let welcome = !current_settings(handle).onboarded;
            if perf_idle.is_none() && (first_run || welcome || !background || updated) {
                open_settings(handle, updated.then_some("about"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            settings_file_error,
            set_settings,
            reset_settings,
            pause_controls,
            resume_autocorrect,
            set_pause_shortcut,
            system_look,
            update_status,
            check_for_updates,
            install_update,
            send_preview,
            crash_preview,
            app_info,
            tour_reset,
            tour_key,
            stats_view,
            clear_stats,
            crash_report_pending,
            answer_crash_report,
            remote_message,
            dismiss_message,
            input_interference,
            usage_event
        ])
        .build(tauri::generate_context!())
        .expect("error while building unflub");

    // Closing the settings window keeps unflub running in the tray.
    app.run(|app, event| match event {
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        RunEvent::Exit => stop_on_exit(app),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_patch_keeps_word_sources_clean_for_the_checked_save() {
        let before = Settings {
            personal_words: vec!["Kept".into(), "Gone".into()],
            word_sources: [("kept".into(), Some(7)), ("gone".into(), None)].into(),
            ..Settings::default()
        };
        let mut next = before.clone();
        let patch = serde_json::json!({ "personalWords": ["Kept", "New"] });
        next.patch(patch.as_object().unwrap().clone(), false)
            .unwrap();
        sync_word_sources(&before, &mut next, &[]);
        // The checked save refuses anything sanitising would change.
        assert_eq!(next.clone().sanitised(), next);
        assert_eq!(
            next.word_sources,
            [("kept".into(), Some(7)), ("new".into(), None)].into()
        );
    }

    #[test]
    fn legacy_words_keep_unknown_source_and_new_words_get_their_source() {
        let current = Settings {
            personal_words: vec!["Old".into(), "Learned".into(), "Removed".into()],
            word_sources: [("learned".into(), Some(42)), ("removed".into(), None)].into(),
            ..Settings::default()
        };
        let mut next = Settings {
            personal_words: vec![
                "old".into(),
                "LEARNED".into(),
                "learned".into(),
                "Manual".into(),
                "Pending".into(),
            ],
            ..Settings::default()
        };
        sync_word_sources(&current, &mut next, &["pending".into()]);
        let next = next.sanitised();
        assert_eq!(next.personal_words, ["old", "LEARNED", "Manual", "Pending"]);
        assert_eq!(next.word_sources.get("old"), None);
        assert_eq!(next.word_sources.get("learned"), Some(&Some(42)));
        assert_eq!(next.word_sources.get("manual"), Some(&None));
        assert_eq!(
            next.word_sources.get("pending"),
            Some(&Some(stats::today()))
        );
        assert_eq!(next.word_sources.get("removed"), None);
    }

    #[test]
    fn a_stale_save_keeps_words_learned_since_the_window_loaded() {
        // The window loaded ["rmgn"]; meanwhile undo taught "unflub" and "Teh".
        let learned = vec!["unflub".to_string(), "Teh".to_string()];
        let mut from_window = vec!["rmgn".to_string()];
        merge_learned(&mut from_window, &learned);
        assert_eq!(from_window, ["rmgn", "unflub", "Teh"]);
    }

    /// Review S4: words fixed in a terminal never reach stats.json.
    #[test]
    fn terminal_words_never_reach_the_stats_page() {
        assert!(keep_word_in_stats(true, false, true));
        assert!(!keep_word_in_stats(true, false, false), "terminal");
        assert!(!keep_word_in_stats(true, true, true), "shortcut");
        assert!(!keep_word_in_stats(false, false, true), "setting off");
    }

    /// Review finding 5: undone pairs are only counted when learning will use them.
    #[test]
    fn undo_pairs_are_recorded_only_for_learning() {
        assert!(records_undo(2, false, true));
        assert!(!records_undo(0, false, true), "learning off");
        assert!(!records_undo(2, true, true), "shortcut");
        assert!(!records_undo(2, false, false), "terminal or unknown app");
    }

    #[test]
    fn the_tray_says_when_unflub_paused_itself() {
        assert_eq!(tooltip(true, false, false, false), strings::TOOLTIP);
        assert_eq!(
            tooltip(true, true, false, false),
            strings::TOOLTIP_INTERFERENCE
        );
        // Review finding 7: a hook that couldn't be installed shows, until it works.
        assert_eq!(
            tooltip(true, false, true, false),
            strings::TOOLTIP_HOOK_UNAVAILABLE
        );
        assert_eq!(
            tooltip(true, true, true, false),
            strings::TOOLTIP_HOOK_UNAVAILABLE
        );
        // The user's pause wins.
        assert_eq!(tooltip(false, true, false, false), strings::TOOLTIP_PAUSED);
        assert_eq!(tooltip(false, false, true, false), strings::TOOLTIP_PAUSED);
    }

    /// #132: after typing in an administrator window the tooltip says why nothing was fixed
    /// there, quietly; every other state still wins.
    #[test]
    fn the_tray_explains_administrator_windows() {
        assert_eq!(tooltip(true, false, false, true), strings::TOOLTIP_ELEVATED);
        assert_eq!(tooltip(false, false, false, true), strings::TOOLTIP_PAUSED);
        assert_eq!(
            tooltip(true, true, false, true),
            strings::TOOLTIP_INTERFERENCE
        );
        assert_eq!(
            tooltip(true, false, true, true),
            strings::TOOLTIP_HOOK_UNAVAILABLE
        );
        assert!(
            strings::TOOLTIP_ELEVATED.chars().count() < 128,
            "Windows cuts tooltips at 127"
        );
    }

    #[test]
    fn perf_idle_takes_a_directory() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            perf_idle_dir(args(&["unflub.exe", "--perf-idle", "C:/tmp/x"])),
            Some("C:/tmp/x".into())
        );
        assert_eq!(perf_idle_dir(args(&["unflub.exe", "--perf-idle"])), None);
        assert_eq!(perf_idle_dir(args(&["unflub.exe", "--background"])), None);
    }

    /// Review #95: removing a word the engine just learned doesn't put it straight back, but a
    /// stale save from a window that never showed it still keeps it.
    #[test]
    fn a_removed_learned_word_stays_removed() {
        let mut pending = vec!["unflub".to_string(), "teh".to_string()];
        forget_removed(&mut pending, &["Unflub".to_string()]);
        assert_eq!(pending, ["teh"]);
        let mut from_window = vec!["rmgn".to_string()];
        merge_learned(&mut from_window, &pending);
        assert_eq!(from_window, ["rmgn", "teh"]);
    }

    #[test]
    fn only_words_the_user_added_count() {
        let s = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(words_added(&s(&["rmgn"]), &s(&["rmgn", "unflub"]), &[]), 1);
        assert_eq!(
            words_added(&s(&["rmgn"]), &s(&["RMGN", "Unflub", "unflub"]), &[]),
            1,
            "case and repeats"
        );
        assert_eq!(
            words_added(&s(&["rmgn"]), &s(&["rmgn", "teh"]), &s(&["Teh"])),
            0,
            "learned by undo"
        );
        assert_eq!(
            words_added(&s(&["rmgn", "x"]), &s(&["x"]), &[]),
            0,
            "removed"
        );
    }

    #[test]
    fn usage_events_are_a_closed_list() {
        for (name, event) in [
            ("stats_page_view", UsageEvent::StatsPageView),
            ("tour_replay", UsageEvent::TourReplay),
            ("tour_completed", UsageEvent::TourCompleted),
            ("tour_skipped", UsageEvent::TourSkipped),
            ("update_dismissed", UsageEvent::UpdateDismissed),
        ] {
            assert_eq!(
                serde_json::from_value::<UsageEvent>(serde_json::json!(name)).unwrap(),
                event
            );
        }
        for bad in ["settings_open", "StatsPageView", "chrome.exe", ""] {
            assert!(serde_json::from_value::<UsageEvent>(serde_json::json!(bad)).is_err());
        }
    }

    #[test]
    fn merging_ignores_words_already_there_in_any_case() {
        let mut words = vec!["Unflub".to_string()];
        merge_learned(&mut words, &["unflub".to_string(), "UNFLUB".to_string()]);
        assert_eq!(words, ["Unflub"]);
        let mut pending = Vec::new();
        merge_learned(&mut pending, &["teh".to_string()]);
        merge_learned(&mut pending, &["TEH".to_string()]);
        assert_eq!(pending, ["teh"]);
    }

    /// Nothing in the settings identifies the install.
    #[test]
    fn settings_hold_no_identifier() {
        let v = for_window(&Settings::default());
        for key in v.as_object().unwrap().keys() {
            let k = key.to_lowercase();
            assert!(
                !k.ends_with("id") && !k.contains("secret") && !k.contains("bucket"),
                "{key}"
            );
        }
    }
}
