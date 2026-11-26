//! The real `unflub-engine` binary against an app-side pipe in this test, with `--test-ipc`:
//! handshake, settings, shutdown and the crash marker. It never installs a hook or shows a
//! pill, so it is safe on a machine someone is typing on.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use unflub_engine::pipe::{unique_name, Listener, Pipe};
use unflub_engine::protocol::{EngineSettings, FromEngine, ToEngine};
use unflub_engine::{frame, session};

const T: Duration = Duration::from_secs(20);

fn spawn(extra: &[&str]) -> (Child, Pipe) {
    let name = unique_name();
    let listener = Listener::create(&name).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_unflub-engine"))
        .args([
            "--pipe",
            &name,
            "--app-pid",
            &std::process::id().to_string(),
            "--test-ipc",
        ])
        .args(extra)
        .spawn()
        .unwrap();
    let (pipe, version) = session::serve(listener, child.id(), "unflub.exe", T).unwrap();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    (child, pipe)
}

fn next(pipe: &Pipe) -> FromEngine {
    frame::decode(&frame::read(&mut &*pipe).unwrap().expect("a message")).unwrap()
}

fn wait(child: &mut Child) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(start.elapsed() < T, "engine didn't exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn configure(pipe: &Pipe) {
    frame::write(
        &mut &*pipe,
        &ToEngine::Configure {
            settings: EngineSettings::default(),
        },
    )
    .unwrap();
}

#[test]
fn handshake_configure_and_shutdown() {
    let (mut child, pipe) = spawn(&[]);
    configure(&pipe);
    assert_eq!(next(&pipe), FromEngine::Ready { hooked: false });
    frame::write(&mut &pipe, &ToEngine::Shutdown).unwrap();
    assert!(wait(&mut child).success());
}

#[test]
fn the_engine_worker_answers_a_heartbeat() {
    let (mut child, pipe) = spawn(&[]);
    configure(&pipe);
    assert_eq!(next(&pipe), FromEngine::Ready { hooked: false });
    frame::write(&mut &pipe, &ToEngine::Ping { nonce: 42 }).unwrap();
    let body = session::read_frame(&pipe, Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert_eq!(
        frame::decode::<FromEngine>(&body).unwrap(),
        FromEngine::Pong { nonce: 42 }
    );
    frame::write(&mut &pipe, &ToEngine::Shutdown).unwrap();
    assert!(wait(&mut child).success());
}

#[test]
fn closing_the_pipe_ends_the_engine() {
    let (mut child, pipe) = spawn(&[]);
    configure(&pipe);
    assert_eq!(next(&pipe), FromEngine::Ready { hooked: false });
    drop(pipe);
    assert!(wait(&mut child).success());
}

#[test]
fn a_panic_leaves_a_scrubbed_marker_and_ends_the_process() {
    let marker = std::env::temp_dir().join(format!(
        "unflub-engine-test-crash-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let (mut child, pipe) = spawn(&["--test-panic", "--crash-file", marker.to_str().unwrap()]);
    configure(&pipe);
    assert_eq!(next(&pipe), FromEngine::Ready { hooked: false });
    let status = wait(&mut child);
    assert_eq!(status.code(), Some(101), "{status:?}");
    // The pipe closed with the process.
    assert!(frame::read(&mut &pipe).unwrap().is_none());

    let text = std::fs::read_to_string(&marker).unwrap();
    let _ = std::fs::remove_file(&marker);
    assert!(!text.contains("hunter2"), "{text}");
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    if home.len() > 3 {
        assert!(
            !text.to_lowercase().contains(&home.to_lowercase()),
            "{text}"
        );
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["tags"]["process"], "unflub-engine");
    assert_eq!(v["tags"]["thread"], "unflub-engine");
    assert!(v["exception"]["values"][0]["value"]
        .as_str()
        .unwrap()
        .starts_with("panic message withheld"));
}

#[test]
fn an_engine_not_spawned_by_the_app_gets_nothing() {
    let name = unique_name();
    let listener = Listener::create(&name).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_unflub-engine"))
        .args([
            "--pipe",
            &name,
            "--app-pid",
            &std::process::id().to_string(),
        ])
        .arg("--test-ipc")
        .spawn()
        .unwrap();
    // The app expected some other pid: it drops the engine and waits for one that never comes.
    let err = session::serve(
        listener,
        child.id() + 4,
        "unflub.exe",
        Duration::from_secs(2),
    )
    .err()
    .unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    // No welcome: the engine gives up with the handshake exit code.
    assert_eq!(wait(&mut child).code(), Some(3));
}

/// #123: another process took the pipe before the engine. The app drops it, the engine (which
/// waits for the busy pipe) gets in, and the session works.
#[test]
fn the_engine_gets_in_after_an_impostor_is_dropped() {
    let name = unique_name();
    let listener = Listener::create(&name).unwrap();
    // This test process is the impostor: it isn't the engine the app spawns.
    let impostor = Pipe::connect(&name).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_unflub-engine"))
        .args([
            "--pipe",
            &name,
            "--app-pid",
            &std::process::id().to_string(),
            "--test-ipc",
        ])
        .spawn()
        .unwrap();
    let (pipe, _) = session::serve(listener, child.id(), "unflub.exe", T).unwrap();
    assert_eq!(pipe.client_pid().unwrap(), child.id());
    let mut buf = [0u8; 4];
    assert!(
        !matches!(impostor.read_timeout(&mut buf, Duration::from_millis(200)), Ok(n) if n > 0),
        "the impostor's end is cut off"
    );
    configure(&pipe);
    assert_eq!(next(&pipe), FromEngine::Ready { hooked: false });
    frame::write(&mut &pipe, &ToEngine::Shutdown).unwrap();
    assert!(wait(&mut child).success());
}

#[test]
fn the_engine_exe_carries_the_publisher() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_unflub-engine"));
    let bytes = std::fs::read(exe).unwrap();
    // VERSIONINFO strings are UTF-16LE.
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let has = |s: &str| {
        let needle = utf16(s);
        bytes.windows(needle.len()).any(|w| w == needle)
    };
    assert!(has("Reimagine Media (RMGN)"));
    assert!(has("\u{a9} 2026 Matias Suxo"));
    // The same strings as the app's bundle settings.
    let conf: serde_json::Value =
        serde_json::from_str(include_str!("../../../app/src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(conf["bundle"]["publisher"], "Reimagine Media (RMGN)");
    assert_eq!(conf["bundle"]["copyright"], "\u{a9} 2026 Matias Suxo");
}
