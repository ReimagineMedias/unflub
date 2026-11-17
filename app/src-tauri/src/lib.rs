mod engine;
mod settings;
mod system;
mod updates;

use engine::EngineHost;
use settings::{Settings, Store};
use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use updates::{UpdateStatus, Updates};

const SETTINGS_WINDOW: &str = "settings";
const TRAY_ID: &str = "main";
/// Passed by the start-at-login entry so a login launch stays in the tray.
const BACKGROUND_ARG: &str = "--background";

struct AppState {
    store: Store,
    settings: Mutex<Settings>,
    toggle: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// Hook, engine and pill. Taken out on quit.
    engine: Mutex<Option<EngineHost>>,
    /// Words learned by undo since the settings window last saved. The window may still hold
    /// settings from before they were learned, so its next save gets them merged back in.
    pending_learned: Mutex<Vec<String>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemLook {
    accent: Option<String>,
    platform: &'static str,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_settings(app: AppHandle, mut settings: Settings) -> Result<Settings, String> {
    let state = app.state::<AppState>();
    let pending = state.pending_learned.lock().unwrap().clone();
    merge_learned(&mut settings.personal_words, &pending);
    let saved = apply_settings(&app, settings).map_err(|e| e.to_string())?;
    // Saved to disk now; words learned during the save stay pending.
    state
        .pending_learned
        .lock()
        .unwrap()
        .retain(|w| !pending.contains(w));
    Ok(saved)
}

#[tauri::command]
fn reset_settings(app: AppHandle) -> Result<Settings, String> {
    app.state::<AppState>()
        .pending_learned
        .lock()
        .unwrap()
        .clear();
    apply_settings(&app, Settings::default()).map_err(|e| e.to_string())
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
    updates::check(&app).await
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    updates::install(&app).await
}

#[tauri::command]
fn system_look() -> SystemLook {
    SystemLook {
        accent: system::accent_color(),
        platform: std::env::consts::OS,
    }
}

fn current_settings(app: &AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
}

/// Persist, sync side effects (login item, tray), then tell every window.
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
        refresh_tray(app, next.enabled);
    }
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        engine.apply(&next);
    }
    let _ = app.emit("settings-changed", &next);
    Ok(next)
}

/// Undoing a fix taught the engine `word`: keep it in the personal dictionary for next time.
fn learn_word(app: &AppHandle, word: String) {
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
}

fn sync_autostart(app: &AppHandle, on: bool) {
    // A dev build must never register itself to run at login.
    if cfg!(debug_assertions) {
        eprintln!("start at login: skipped in dev builds (would be {on})");
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

fn tooltip(enabled: bool) -> &'static str {
    if enabled {
        "unflub"
    } else {
        "unflub (paused)"
    }
}

fn refresh_tray(app: &AppHandle, enabled: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(tray_image(enabled)));
        let _ = tray.set_tooltip(Some(tooltip(enabled)));
    }
    if let Some(item) = app.state::<AppState>().toggle.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
}

fn open_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let builder =
        WebviewWindowBuilder::new(app, SETTINGS_WINDOW, WebviewUrl::App("index.html".into()))
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
    if let Err(e) = builder.build() {
        eprintln!("settings window: {e}");
    }
}

fn build_tray(app: &AppHandle, enabled: bool) -> tauri::Result<()> {
    let toggle = CheckMenuItemBuilder::with_id("toggle", "Autocorrect")
        .checked(enabled)
        .build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&toggle)
        .separator()
        .text("settings", "Settings…")
        .separator()
        .text("quit", "Quit unflub")
        .build()?;
    *app.state::<AppState>().toggle.lock().unwrap() = Some(toggle);

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(enabled))
        .tooltip(tooltip(enabled))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => {
                let mut next = app.state::<AppState>().settings.lock().unwrap().clone();
                next.enabled = !next.enabled;
                let _ = apply_settings(app, next);
            }
            "settings" => open_settings(app),
            "quit" => {
                stop_engine(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_settings(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_settings(app)
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(Updates::new(config_dir.clone()));
            let store = Store::new(config_dir);
            let first_run = !store.exists();
            let settings = store.load();
            let enabled = settings.enabled;
            let start_at_login = settings.start_at_login;
            app.manage(AppState {
                store,
                settings: Mutex::new(settings.clone()),
                toggle: Mutex::new(None),
                engine: Mutex::new(None),
                pending_learned: Mutex::new(Vec::new()),
            });
            let handle = app.handle();
            let learner = handle.clone();
            let engine =
                EngineHost::start(settings.clone(), move |word| learn_word(&learner, word));
            *app.state::<AppState>().engine.lock().unwrap() = Some(engine);
            build_tray(handle, enabled)?;
            if first_run {
                let _ = app.state::<AppState>().store.save(&settings);
                sync_autostart(handle, start_at_login);
            }
            updates::spawn_daily(handle.clone());
            let background = std::env::args().any(|a| a == BACKGROUND_ARG);
            if first_run || !background {
                open_settings(handle);
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
            install_update
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
    use super::merge_learned;

    #[test]
    fn a_stale_save_keeps_words_learned_since_the_window_loaded() {
        // The window loaded ["rmgn"]; meanwhile undo taught "unflub" and "Teh".
        let learned = vec!["unflub".to_string(), "Teh".to_string()];
        let mut from_window = vec!["rmgn".to_string()];
        merge_learned(&mut from_window, &learned);
        assert_eq!(from_window, ["rmgn", "unflub", "Teh"]);
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
}
