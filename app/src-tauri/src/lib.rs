mod settings;
mod system;

use settings::{Settings, Store};
use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const SETTINGS_WINDOW: &str = "settings";
const TRAY_ID: &str = "main";
/// Passed by the start-at-login entry so a login launch stays in the tray.
const BACKGROUND_ARG: &str = "--background";

struct AppState {
    store: Store,
    settings: Mutex<Settings>,
    toggle: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
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
fn set_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    apply_settings(&app, settings).map_err(|e| e.to_string())
}

#[tauri::command]
fn reset_settings(app: AppHandle) -> Result<Settings, String> {
    apply_settings(&app, Settings::default()).map_err(|e| e.to_string())
}

#[tauri::command]
fn system_look() -> SystemLook {
    SystemLook {
        accent: system::accent_color(),
        platform: std::env::consts::OS,
    }
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
    // The engine picks up `next.to_core()` here once the platform adapter is wired in.
    let _ = app.emit("settings-changed", &next);
    Ok(next)
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
            "quit" => app.exit(0),
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
        .setup(|app| {
            let store = Store::new(app.path().app_config_dir()?);
            let first_run = !store.exists();
            let settings = store.load();
            let enabled = settings.enabled;
            let start_at_login = settings.start_at_login;
            app.manage(AppState {
                store,
                settings: Mutex::new(settings.clone()),
                toggle: Mutex::new(None),
            });
            let handle = app.handle();
            build_tray(handle, enabled)?;
            if first_run {
                let _ = app.state::<AppState>().store.save(&settings);
                sync_autostart(handle, start_at_login);
            }
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
            system_look
        ])
        .build(tauri::generate_context!())
        .expect("error while building unflub");

    // Closing the settings window keeps unflub running in the tray.
    app.run(|_app, event| {
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}
