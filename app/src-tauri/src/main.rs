// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    if let Some(code) = unflub_app_lib::dispatch_cli(std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    unflub_app_lib::run()
}
