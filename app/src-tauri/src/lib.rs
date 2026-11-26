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
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder},
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
    toggle: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// Hook, engine and pill. Taken out on quit.
    engine: Mutex<Option<EngineHost>>,
    /// Words learned by undo since the settings window last saved. The window may still hold
    /// settings from before they were learned, so its next save gets them merged back in.
    pending_learned: Mutex<Vec<String>>,
    /// The engine paused itself because another program interferes with keyboard input.
    interference: Mutex<Interference>,
    /// Autocorrect has no working engine session yet.
    hook_unavailable: std::sync::atomic::AtomicBool,
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

/// The Privacy page's "Exactly what's sent". Pretty JSON, in the order it is sent.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SendPreview {
    /// The daily usage stats, or `None` while that switch is off.
    stats: Option<String>,
    /// The headers on update requests (manifest and release files), as they would go out now.
    headers: Vec<(String, String)>,
    /// Crash reports can be sent from this build.
    crash_reports_available: bool,
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
    mut settings: Settings,
    removed: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    forget_removed(
        &mut state.pending_learned.lock().unwrap(),
        &removed.unwrap_or_default(),
    );
    let added = words_added(
        &state.settings.lock().unwrap().personal_words,
        &settings.personal_words,
        &state.pending_learned.lock().unwrap(),
    );
    let mut pending = Vec::new();
    let saved = update_settings(&app, |current| {
        // Read while no other change can land, so a word learned and saved since the window's
        // copy is always in here.
        pending = state.pending_learned.lock().unwrap().clone();
        merge_learned(&mut settings.personal_words, &pending);
        // The window doesn't set the file's schema version.
        settings.schema_version = current.schema_version;
        *current = settings;
    })
    .map_err(|e| e.to_string())?;
    app.state::<Usage>().personal_words_added(added);
    // Saved to disk now; words learned during the save stay pending.
    state
        .pending_learned
        .lock()
        .unwrap()
        .retain(|w| !pending.contains(w));
    Ok(for_window(&saved))
}

#[tauri::command]
fn reset_settings(app: AppHandle) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    state.pending_learned.lock().unwrap().clear();
    update_settings(&app, |current| {
        // A reset is about preferences: the welcome isn't shown again, and usage stats and
        // crash reports stay as the user set them (an off is never turned back on).
        *current = Settings {
            schema_version: current.schema_version,
            onboarded: current.onboarded,
            share_usage_stats: current.share_usage_stats,
            send_crash_reports: current.send_crash_reports,
            extra: std::mem::take(&mut current.extra),
            ..Settings::default()
        };
    })
    .map(|s| for_window(&s))
    .map_err(|e| e.to_string())
}

/// How many of `next` the user added in the window: not in `prev`, and not one of the words
/// learned by undo that are being merged in (ignoring case).
fn words_added(prev: &[String], next: &[String], learned: &[String]) -> u64 {
    let known: std::collections::HashSet<String> = prev
        .iter()
        .chain(learned)
        .map(|w| w.to_lowercase())
        .collect();
    let mut added = std::collections::HashSet::new();
    for w in next {
        let key = w.to_lowercase();
        if !known.contains(&key) {
            added.insert(key);
        }
    }
    added.len() as u64
}

/// Drop from `pending` every word the user removed (ignoring case).
fn forget_removed(pending: &mut Vec<String>, removed: &[String]) {
    pending.retain(|w| {
        let key = w.to_lowercase();
        !removed.iter().any(|r| r.to_lowercase() == key)
    });
}

/// Add each of `learned` to `words` unless it is already there (ignoring case).
fn merge_learned(words: &mut Vec<String>, learned: &[String]) {
    for word in learned {
        let key = word.to_lowercase();
        if !words.iter().any(|w| w.to_lowercase() == key) {
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
    let stats = settings.share_usage_stats.then(|| {
        pretty(
            &app.state::<Usage>()
                .payload(&settings, &version, &env_facts(&app)),
        )
    });
    let headers = updates::request_headers(&app)
        .into_iter()
        .map(|(k, v)| {
            (
                canonical_header(k.as_str()),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    SendPreview {
        stats,
        headers,
        crash_reports_available: crash::enabled(),
    }
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

/// `user-agent` as `User-Agent`, for reading.
fn canonical_header(name: &str) -> String {
    match name {
        "user-agent" => "User-Agent".into(),
        other => other.into(),
    }
}

#[tauri::command]
fn tour_reset(app: AppHandle) {
    app.state::<tour::Tour>().reset(&current_settings(&app));
}

/// Async so it runs on a runtime worker, not the main thread: a panic in the practice engine
/// then keeps only its location (`crash.rs` keeps messages from `main`).
#[tauri::command]
async fn tour_key(app: AppHandle, key: String) -> tour::TourAction {
    app.state::<tour::Tour>().key(&current_settings(&app), &key)
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

    if prev.start_at_login != next.start_at_login {
        sync_autostart(app, next.start_at_login);
    }
    if prev.enabled != next.enabled {
        app.state::<Usage>().set_paused(!next.enabled);
        if !next.enabled {
            // Pausing ends the engine, and with it the interference.
            set_interference(app, false);
        }
        refresh_tray(app);
        queue_ui_refresh(app, |app| {
            refresh_window_icon(app, current_settings(app).enabled)
        });
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
    // Another change may have landed since ours: the engine and the windows get the latest.
    let latest = current_settings(app);
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        engine.apply(&latest);
    }
    emit_settings(app, &latest);
    Ok(next)
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
    let key = word.to_lowercase();
    let known = |s: &Settings| s.personal_words.iter().any(|w| w.to_lowercase() == key);
    if known(&current_settings(app)) {
        return;
    }
    let learned = update_settings(app, |s| {
        if !known(s) {
            s.personal_words.push(word);
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

fn tooltip(enabled: bool, interference: bool, hook_unavailable: bool) -> &'static str {
    match (enabled, interference, hook_unavailable) {
        (false, _, _) => strings::TOOLTIP_PAUSED,
        (true, _, true) => strings::TOOLTIP_HOOK_UNAVAILABLE,
        (true, true, false) => strings::TOOLTIP_INTERFERENCE,
        (true, false, false) => strings::TOOLTIP,
    }
}

fn refresh_tray(app: &AppHandle) {
    queue_ui_refresh(app, refresh_tray_on_main_thread);
}

fn refresh_tray_on_main_thread(app: &AppHandle) {
    let enabled = current_settings(app).enabled;
    let state = app.state::<AppState>();
    let interference = state.interference.lock().unwrap().active;
    let hook_unavailable = state
        .hook_unavailable
        .load(std::sync::atomic::Ordering::Relaxed);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // Paused by itself, or not working yet, looks paused.
        let _ = tray.set_icon(Some(tray_image(
            enabled && !interference && !hook_unavailable,
        )));
        let _ = tray.set_tooltip(Some(tooltip(enabled, interference, hook_unavailable)));
    }
    if let Some(item) = app.state::<AppState>().toggle.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
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
            if !current_settings(app).enabled {
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
    let enabled = current_settings(app).enabled;
    let toggle = CheckMenuItemBuilder::with_id("toggle", strings::MENU_AUTOCORRECT)
        .checked(enabled)
        .build(app)?;
    let mut menu = MenuBuilder::new(app).item(&toggle).separator();
    match app.state::<Updates>().status() {
        UpdateStatus::Ready { version, .. } => {
            menu = menu.text("update", strings::menu_restart_to_update(&version));
        }
        UpdateStatus::Available {
            version, critical, ..
        } => {
            menu = menu.text("update", strings::menu_update_available(&version, critical));
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
        .separator()
        .text("quit", strings::MENU_QUIT)
        .build()?;
    *app.state::<AppState>().toggle.lock().unwrap() = Some(toggle);
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
    // A downloaded update installs on the way out; the installer closes unflub itself.
    if updates::install_on_quit(app) {
        return;
    }
    stop_on_exit(app);
    app.exit(0);
}

fn build_tray(app: &AppHandle, enabled: bool) -> tauri::Result<()> {
    let menu = tray_menu(app)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(enabled && !cfg!(windows)))
        .tooltip(tooltip(enabled, false, cfg!(windows)))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => {
                let _ = update_settings(app, |s| s.enabled = !s.enabled);
            }
            "settings" => open_settings(app, None),
            "update" => match app.state::<Updates>().status() {
                UpdateStatus::Ready { .. } => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = updates::install(&app, true).await {
                            eprintln!("update: {e}");
                        }
                    });
                }
                _ => open_settings(app, Some("about")),
            },
            "crash" | "message" => open_settings(app, Some("about")),
            "tour" => open_settings(app, Some("tour")),
            "support" => open_url(app, links::SPONSOR_URL),
            "star" => open_url(app, links::REPO_URL),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_settings(tray.app_handle(), None);
            }
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
            app.manage(Updates::new(local_dir.clone(), local_dir.clone()));
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
            app.manage(Stats::new(local_dir));
            app.manage(tour::Tour::default());
            let enabled = settings.enabled;
            let start_at_login = settings.start_at_login;
            app.manage(AppState {
                store,
                settings: Mutex::new(settings.clone()),
                toggle: Mutex::new(None),
                engine: Mutex::new(None),
                pending_learned: Mutex::new(Vec::new()),
                interference: Mutex::default(),
                hook_unavailable: Default::default(),
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
            if first_run {
                let _ = app.state::<AppState>().store.save(&settings);
                sync_autostart(handle, start_at_login);
            } else {
                sync_login_truth(handle);
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
            updates::spawn_background(handle.clone());
            usage::spawn(handle.clone());
            let background = std::env::args().any(|a| a == BACKGROUND_ARG);
            let welcome = !current_settings(handle).onboarded;
            if perf_idle.is_none() && (first_run || welcome || !background) {
                open_settings(handle, None);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            reset_settings,
            system_look,
            update_status,
            check_for_updates,
            install_update,
            send_preview,
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
        assert_eq!(tooltip(true, false, false), strings::TOOLTIP);
        assert_eq!(tooltip(true, true, false), strings::TOOLTIP_INTERFERENCE);
        // Review finding 7: a hook that couldn't be installed shows, until it works.
        assert_eq!(
            tooltip(true, false, true),
            strings::TOOLTIP_HOOK_UNAVAILABLE
        );
        assert_eq!(tooltip(true, true, true), strings::TOOLTIP_HOOK_UNAVAILABLE);
        // The user's pause wins.
        assert_eq!(tooltip(false, true, false), strings::TOOLTIP_PAUSED);
        assert_eq!(tooltip(false, false, true), strings::TOOLTIP_PAUSED);
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
