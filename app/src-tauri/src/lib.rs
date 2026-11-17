mod crash;
mod engine;
mod install;
mod links;
mod managed;
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
use usage::Usage;

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
}

/// See [`AppState::interference`]. Counted here only: the usage stats contract has no field
/// for it yet.
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
    /// The update check's headers, as they would go out now.
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

#[tauri::command]
fn set_settings(app: AppHandle, mut settings: Settings) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let pending = state.pending_learned.lock().unwrap().clone();
    merge_learned(&mut settings.personal_words, &pending);
    // The window doesn't set the file's schema version.
    settings.schema_version = state.settings.lock().unwrap().schema_version;
    let saved = apply_settings(&app, settings).map_err(|e| e.to_string())?;
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
    let current = current_settings(&app);
    // A reset is about preferences: the welcome isn't shown again.
    let fresh = Settings {
        schema_version: current.schema_version,
        onboarded: current.onboarded,
        extra: current.extra,
        ..Settings::default()
    };
    apply_settings(&app, fresh)
        .map(|s| for_window(&s))
        .map_err(|e| e.to_string())
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
    let stats = settings
        .share_usage_stats
        .then(|| pretty(&app.state::<Usage>().payload(&settings, &version)));
    let headers = updates::api_headers(&app)
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

/// `x-os-version` as `X-OS-Version`, for reading.
fn canonical_header(name: &str) -> String {
    match name {
        "x-os-version" => net::OS_VERSION_HEADER.into(),
        "x-rollout-bucket" => net::ROLLOUT_BUCKET_HEADER.into(),
        "x-first" => net::FIRST_HEADER.into(),
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
    refresh_tray(app, current_settings(app).enabled);
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

/// Persist, sync side effects (login item, tray, privacy switches), then tell every window.
fn apply_settings(app: &AppHandle, next: Settings) -> std::io::Result<Settings> {
    let next = next.sanitised();
    let state = app.state::<AppState>();
    let prev = state.settings.lock().unwrap().clone();
    state.store.save(&next)?;
    *state.settings.lock().unwrap() = next.clone();

    if prev.start_at_login != next.start_at_login {
        sync_autostart(app, next.start_at_login);
    }
    if prev.enabled != next.enabled {
        if !next.enabled {
            // Pausing ends the engine, and with it the interference.
            set_interference(app, false);
        }
        refresh_tray(app, next.enabled);
        refresh_window_icon(app, next.enabled);
    }
    if prev.share_usage_stats != next.share_usage_stats {
        let managed = app.state::<AppInfo>().managed;
        app.state::<Usage>()
            .set_on(managed.effective(next.clone()).share_usage_stats);
    }
    if prev.stats_keep_words && !next.stats_keep_words {
        app.state::<Stats>().forget_words();
    }
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        engine.apply(&next);
    }
    emit_settings(app, &next);
    Ok(next)
}

/// The remote config changed: pass its flags on and show its message.
fn apply_remote(app: &AppHandle) {
    let remote = app.state::<Remote>();
    if let Some(engine) = app.state::<AppState>().engine.lock().unwrap().as_ref() {
        engine.set_flags(remote.flags());
    }
    let _ = app.emit(remote::MESSAGE_EVENT, remote.message());
}

/// Undoing a fix taught the engine `word`: keep it in the personal dictionary for next time.
fn learn_word(app: &AppHandle, word: String) {
    // Learned: its undo count has done its job.
    app.state::<Arc<undo_counts::UndoCounts>>().forget(&word);
    let state = app.state::<AppState>();
    merge_learned(
        &mut state.pending_learned.lock().unwrap(),
        std::slice::from_ref(&word),
    );
    let mut next = state.settings.lock().unwrap().clone();
    let key = word.to_lowercase();
    if next.personal_words.iter().any(|w| w.to_lowercase() == key) {
        return;
    }
    next.personal_words.push(word);
    if let Err(e) = apply_settings(app, next) {
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
        } => {
            let settings = current_settings(app);
            usage.correction(decide, inject, pill);
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
        } => {
            usage.undo();
            app.state::<Stats>().undo(&original, &corrected);
            if !user_rule {
                app.state::<Arc<undo_counts::UndoCounts>>()
                    .record(&original, &corrected);
            }
        }
        Observed::CorrectorPanicked => usage.crash(),
        Observed::HookReinstalled => usage.hook_reinstall(),
        Observed::InputInterference { active } => set_interference(app, active),
        Observed::EngineCrashed => {
            set_interference(app, false);
            usage.crash();
            // A panic left a marker for the opt-in crash report; other exits leave none.
            crash::take_engine_report(app);
        }
    }
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
    // Take it out first: the engine thread may call apply_settings while it winds down.
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
    let next = {
        let mut settings = state.settings.lock().unwrap();
        settings.start_at_login = actual;
        settings.clone()
    };
    if let Err(e) = state.store.save(&next) {
        eprintln!("start at login: {e}");
    }
    emit_settings(app, &next);
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

fn tooltip(enabled: bool, interference: bool) -> &'static str {
    match (enabled, interference) {
        (true, false) => strings::TOOLTIP,
        (true, true) => strings::TOOLTIP_INTERFERENCE,
        (false, _) => strings::TOOLTIP_PAUSED,
    }
}

fn refresh_tray(app: &AppHandle, enabled: bool) {
    let interference = app.state::<AppState>().interference.lock().unwrap().active;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // Paused by itself looks paused.
        let _ = tray.set_icon(Some(tray_image(enabled && !interference)));
        let _ = tray.set_tooltip(Some(tooltip(enabled, interference)));
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
    stop_engine(app);
    app.exit(0);
}

fn build_tray(app: &AppHandle, enabled: bool) -> tauri::Result<()> {
    let menu = tray_menu(app)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(enabled))
        .tooltip(tooltip(enabled, false))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => {
                let mut next = current_settings(app);
                next.enabled = !next.enabled;
                let _ = apply_settings(app, next);
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
            "crash" => open_settings(app, Some("about")),
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
            app.manage(Updates::new(config_dir.clone(), local_dir));
            app.manage(Remote::new(
                data_dir.clone(),
                info.managed.disable_remote_config,
            ));
            app.manage(net::Rollout::new(config_dir.clone()));
            let store = Store::new(config_dir);
            let first_run = !store.exists();
            let settings = store.load();
            let usage = Usage::new(
                data_dir.clone(),
                info.managed.effective(settings.clone()).share_usage_stats,
            );
            if crashed_last_time {
                usage.crash();
            }
            app.manage(usage);
            let counts = Arc::new(undo_counts::UndoCounts::new(&data_dir));
            app.manage(counts.clone());
            app.manage(Stats::new(data_dir));
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
                    updates::refresh_remote(&remote_app).await;
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
            input_interference
        ])
        .build(tauri::generate_context!())
        .expect("error while building unflub");

    // Closing the settings window keeps unflub running in the tray.
    app.run(|app, event| match event {
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        RunEvent::Exit => stop_engine(app),
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

    #[test]
    fn the_tray_says_when_unflub_paused_itself() {
        assert_eq!(tooltip(true, false), strings::TOOLTIP);
        assert_eq!(tooltip(true, true), strings::TOOLTIP_INTERFERENCE);
        // The user's pause wins.
        assert_eq!(tooltip(false, true), strings::TOOLTIP_PAUSED);
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
