//! Tauri's build step, plus one rule for the engine sidecar.
//!
//! `bundle > externalBin` in tauri.conf.json puts `unflub-engine.exe` in the installers. Tauri's
//! build step also copies that file next to the app in `target/<profile>/`, which is exactly
//! where Cargo writes the engine it builds from source. So the sidecar applies only to release
//! builds run by the Tauri CLI (`npm run tauri build`, whose beforeBuildCommand builds the
//! engine into `binaries/` first). Every other build (`cargo build`, `clippy`, `test`,
//! `tauri dev`) drops it: nothing is required in `binaries/`, and a stale copy there never
//! replaces the engine Cargo just built.

fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_ENV_PLATFORM");
    let tauri_cli = std::env::var_os("TAURI_ENV_PLATFORM").is_some();
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if !(tauri_cli && release) {
        without_sidecar();
    }
    tauri_build::build()
}

/// Remove `bundle.externalBin` through `TAURI_CONFIG`, the merge patch tauri-build applies on top
/// of tauri.conf.json (a JSON null deletes the key), keeping any patch the CLI already set.
fn without_sidecar() {
    let mut patch: serde_json::Value = std::env::var("TAURI_CONFIG")
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    if !patch["bundle"].is_object() {
        patch["bundle"] = serde_json::json!({});
    }
    patch["bundle"]["externalBin"] = serde_json::Value::Null;
    // Edition 2021: set_var is safe; the build script is single-threaded here.
    std::env::set_var("TAURI_CONFIG", patch.to_string());
}
