//! Opt-in crash reports through Sentry.
//!
//! Nothing is sent when unflub crashes. The panic hook writes one scrubbed report to
//! `crash-pending.json` and the process ends (release builds abort on panic). At the next start
//! the settings window asks "Send the crash report?" (Send, Don't send, Always send); with
//! "Always send" (`sendCrashReports`) it goes without asking.
//!
//! A report holds the panic location, the stack (addresses, symbolicated by Sentry from the
//! debug symbols CI uploads), the loaded modules, the OS and Rust versions, and the app version.
//! It never holds typed text: panics on the threads that see keys or words keep only their
//! location, there are no breadcrumbs, `send_default_pii` is off, the machine name is dropped,
//! and the home folder is cut out of every path.
//!
//! Without `SENTRY_DSN` at build time (local and dev builds) there is nowhere to send to: the
//! hook still notes the crash for the usage stats, and nothing is ever asked or sent.
//!
//! The keyboard engine is its own process (`unflub-engine.exe`) with no network code. Its panic
//! hook writes `engine-crash.json`: the panic location, frame addresses, loaded modules and the
//! thread, never the message, with the home folder already cut out. [`take_engine_report`]
//! turns it into a pending report here, under exactly the same rules as the app's own.

use sentry::protocol::{DebugMeta, Event};
use serde_json::Value;
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};

/// Emitted with `true`/`false` when a report starts or stops waiting for an answer.
pub const PENDING_EVENT: &str = "crash-report";

/// Baked in by release CI. `None` turns crash reports off entirely.
const DSN: Option<&str> = option_env!("SENTRY_DSN");

/// Threads whose panic message may be kept. Every other thread can hold typed text or words (the
/// hook, injector, engine, pill, spell checker, UI Automation), so only the panic's location and
/// stack are kept for them.
const MESSAGE_THREADS: &[&str] = &["main", "unflub-update", "unflub-stats"];

const WITHHELD: &str = "panic message withheld: this thread handles typed text";

pub fn enabled() -> bool {
    DSN.is_some_and(|d| !d.trim().is_empty())
}

/// The user's answer to "Send the crash report?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Answer {
    Send,
    DontSend,
    AlwaysSend,
}

pub struct Crashes {
    pending_path: PathBuf,
    pending: Mutex<bool>,
    /// Written by a panicking engine (see the module docs).
    engine_marker: PathBuf,
    release: String,
    /// The `DisableCrashReports` policy.
    disabled: bool,
}

impl Crashes {
    pub fn pending(&self) -> bool {
        *self.pending.lock().unwrap()
    }

    /// Where the engine writes its panic marker.
    pub fn engine_marker(&self) -> PathBuf {
        self.engine_marker.clone()
    }
}

/// The marker's file name, in the same folder as `crash-pending.json`.
pub const ENGINE_MARKER: &str = "engine-crash.json";

/// Install the panic hook and look for a report from the last run. Call once, early in setup.
/// Returns the state to manage and whether the last run crashed. Under the
/// `DisableCrashReports` policy a report is only counted, never kept for sending.
pub fn install(dir: &Path, version: &str, disabled: bool) -> (Crashes, bool) {
    let pending_path = dir.join("crash-pending.json");
    let crashed_last_time = pending_path.exists();
    let release = format!("unflub@{version}");
    let home = home_dir();
    let path = pending_path.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Keep the first report if several threads panic at once.
        if !path.exists() {
            let event = event_from_panic(info, &release, home.as_deref());
            if let Ok(bytes) = serde_json::to_vec(&event) {
                if let Some(dir) = path.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                let _ = fs::write(&path, bytes);
            }
        }
        previous(info);
    }));
    let has_report = crashed_last_time && enabled() && !disabled;
    if crashed_last_time && !has_report {
        // Counted by the caller; nothing to send it to.
        let _ = fs::remove_file(&pending_path);
    }
    (
        Crashes {
            pending_path,
            pending: Mutex::new(has_report),
            engine_marker: dir.join(ENGINE_MARKER),
            release: format!("unflub@{version}"),
            disabled,
        },
        crashed_last_time,
    )
}

/// Turn a marker left by a panicking engine into a pending report, if crash reports can be
/// sent from this build and policy allows; otherwise just delete it. The first report waiting
/// wins, as with the app's own panics. True if there was a marker (the caller counts a crash).
pub fn take_engine_report(app: &AppHandle) -> bool {
    let crashes = app.state::<Crashes>();
    let Ok(bytes) = fs::read(&crashes.engine_marker) else {
        return false;
    };
    let _ = fs::remove_file(&crashes.engine_marker);
    if !enabled() || crashes.disabled || crashes.pending_path.exists() {
        return true;
    }
    let Some(event) = event_from_engine(&bytes, &crashes.release, home_dir().as_deref()) else {
        return true;
    };
    let written = serde_json::to_vec(&event)
        .map_err(std::io::Error::other)
        .and_then(|json| fs::write(&crashes.pending_path, json));
    if written.is_ok() {
        if crate::effective_settings(app).send_crash_reports {
            send_in_background(app.clone());
        } else {
            set_pending(app, true);
        }
    }
    true
}

/// The engine's marker as a report from this app version, scrubbed again here. The engine
/// already withholds every panic message; any other message is withheld too.
fn event_from_engine(bytes: &[u8], release: &str, home: Option<&str>) -> Option<Event<'static>> {
    let mut event: Event<'static> = serde_json::from_slice(bytes).ok()?;
    for exception in event.exception.values.iter_mut() {
        let withheld = exception
            .value
            .as_deref()
            .is_some_and(|v| v.starts_with("panic message withheld"));
        if !withheld {
            exception.value = Some(WITHHELD.into());
        }
    }
    event.release = Some(Cow::Owned(release.to_string()));
    event.tags.insert("process".into(), "unflub-engine".into());
    Some(scrub(event, home))
}

fn event_from_panic(
    info: &std::panic::PanicHookInfo<'_>,
    release: &str,
    home: Option<&str>,
) -> Event<'static> {
    let mut event =
        sentry::integrations::panic::PanicIntegration::new().event_from_panic_info(info);
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    if !MESSAGE_THREADS.contains(&thread_name) {
        for exception in event.exception.values.iter_mut() {
            exception.value = Some(WITHHELD.into());
        }
    }
    event.tags.insert("thread".into(), thread_name.into());
    event.release = Some(Cow::Owned(release.to_string()));
    // The modules loaded now, so the stack symbolicates against the binary that crashed even if
    // an update replaced it before the report is sent.
    event.debug_meta = Cow::Owned(DebugMeta {
        images: sentry::integrations::debug_images::debug_images(),
        ..Default::default()
    });
    scrub(event, home)
}

/// Remove anything that could identify the user: machine name, user, breadcrumbs, request
/// data, extra context, and the home folder from every string.
fn scrub(mut event: Event<'static>, home: Option<&str>) -> Event<'static> {
    event.server_name = None;
    event.user = None;
    event.request = None;
    event.breadcrumbs = Default::default();
    event.extra.clear();
    event.contexts.retain(|k, _| k == "os" || k == "rust");
    let Some(home) = home.filter(|h| h.len() > 3) else {
        return event;
    };
    let Ok(mut value) = serde_json::to_value(&event) else {
        return event;
    };
    strip_home(&mut value, home);
    serde_json::from_value(value).unwrap_or(event)
}

/// Replace the home folder (any slash style, any case) with `~` in every string.
fn strip_home(value: &mut Value, home: &str) {
    match value {
        Value::String(s) => {
            let forms = [
                home.to_string(),
                home.replace('\\', "/"),
                home.replace('/', "\\"),
            ];
            for form in forms {
                *s = replace_ignore_case(s, &form, "~");
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| strip_home(v, home)),
        Value::Object(map) => map.values_mut().for_each(|v| strip_home(v, home)),
        _ => {}
    }
}

fn replace_ignore_case(haystack: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() || !haystack.is_ascii() || !needle.is_ascii() {
        return haystack.replace(needle, with);
    }
    let lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for (i, _) in lower.match_indices(&needle_lower) {
        out.push_str(&haystack[last..i]);
        out.push_str(with);
        last = i + needle.len();
    }
    out.push_str(&haystack[last..]);
    out
}

fn home_dir() -> Option<String> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .filter(|h| !h.is_empty())
}

fn set_pending(app: &AppHandle, pending: bool) {
    *app.state::<Crashes>().pending.lock().unwrap() = pending;
    let _ = app.emit(PENDING_EVENT, pending);
    crate::refresh_tray_menu(app);
}

/// After setup: send a waiting report if the user said "Always send". Otherwise it waits for
/// [`answer`].
pub fn after_start(app: &AppHandle) {
    if app.state::<Crashes>().pending() && crate::effective_settings(app).send_crash_reports {
        send_in_background(app.clone());
    }
}

pub fn answer(app: &AppHandle, answer: Answer) {
    match answer {
        Answer::DontSend => {
            let _ = fs::remove_file(&app.state::<Crashes>().pending_path);
            set_pending(app, false);
        }
        Answer::Send => send_in_background(app.clone()),
        Answer::AlwaysSend => {
            let mut next = crate::current_settings(app);
            next.send_crash_reports = true;
            if let Err(e) = crate::apply_settings(app, next) {
                eprintln!("crash reports: {e}");
            }
            send_in_background(app.clone());
        }
    }
}

fn send_in_background(app: AppHandle) {
    set_pending(&app, false);
    let spawned = std::thread::Builder::new()
        .name("unflub-crash".into())
        .spawn(move || {
            let path = app.state::<Crashes>().pending_path.clone();
            let event = fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Event<'static>>(&b).ok());
            if let Some(event) = event {
                send(event);
            }
            let _ = fs::remove_file(&path);
        });
    if let Err(e) = spawned {
        eprintln!("crash reports: {e}");
    }
}

fn send(event: Event<'static>) {
    let Some(dsn) = DSN.and_then(|d| d.parse().ok()) else {
        return;
    };
    let home = home_dir();
    // The struct is non-exhaustive, so set fields on a default one.
    let mut options = sentry::ClientOptions::new();
    options.dsn = Some(dsn);
    options.release = event.release.clone();
    options.send_default_pii = false;
    options.max_breadcrumbs = 0;
    options.auto_session_tracking = false;
    // Only the OS and Rust versions; the stack and modules came from the crashed process.
    options.default_integrations = false;
    options.integrations = vec![Arc::new(
        sentry::integrations::contexts::ContextIntegration::new().add_device(false),
    )];
    options.before_send = Some(Arc::new(move |event| Some(scrub(event, home.as_deref()))));
    let guard = sentry::init(options);
    sentry::capture_event(event);
    guard.flush(Some(Duration::from_secs(10)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Breadcrumb, Exception, Frame, Stacktrace, User};

    fn event_with_paths(home: &str) -> Event<'static> {
        let mut event = Event {
            server_name: Some("MY-LAPTOP".into()),
            user: Some(User {
                username: Some("alex".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        event.breadcrumbs.values.push(Breadcrumb {
            message: Some("typed: hello".into()),
            ..Default::default()
        });
        event.exception.values.push(Exception {
            ty: "panic".into(),
            value: Some(format!(
                "can't read {home}\\AppData\\Roaming\\ca.rmgn.unflub"
            )),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    abs_path: Some(format!(
                        "{}/AppData/Local/unflub/src/x.rs",
                        home.replace('\\', "/")
                    )),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });
        event
    }

    #[test]
    fn scrub_removes_identity_and_home() {
        let home = r"C:\Users\Alex";
        let event = scrub(event_with_paths(home), Some(home));
        assert_eq!(event.server_name, None);
        assert_eq!(event.user, None);
        assert!(event.breadcrumbs.values.is_empty());
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.to_lowercase().contains("alex"), "{text}");
        assert!(text.contains(r"~\\AppData\\Roaming"), "{text}");
        assert!(text.contains("~/AppData/Local"), "{text}");
    }

    #[test]
    fn replaces_ignoring_case() {
        assert_eq!(
            replace_ignore_case(
                r"c:\users\alex\x and C:\Users\Alex\y",
                r"C:\Users\Alex",
                "~"
            ),
            r"~\x and ~\y"
        );
        assert_eq!(
            replace_ignore_case("nothing here", "C:\\Users\\a", "~"),
            "nothing here"
        );
    }

    /// A panic on a thread that sees keys keeps its stack but not its message.
    #[test]
    fn hook_thread_panics_lose_their_message() {
        let captured: Arc<Mutex<Option<Event<'static>>>> = Arc::default();
        let sink = captured.clone();
        let thread = std::thread::Builder::new()
            .name("unflub-hook".into())
            .spawn(move || {
                let previous = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    if std::thread::current().name() == Some("unflub-hook") {
                        *sink.lock().unwrap() = Some(event_from_panic(info, "unflub@test", None));
                    } else {
                        previous(info);
                    }
                }));
                panic!("the user typed: hunter2");
            })
            .unwrap();
        assert!(thread.join().is_err());
        let _ = std::panic::take_hook();
        let event = captured.lock().unwrap().take().expect("hook ran");
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains(WITHHELD));
        assert_eq!(event.tags["thread"], "unflub-hook");
        assert_eq!(event.release.as_deref(), Some("unflub@test"));
    }

    /// What `crates/unflub-engine/src/crash.rs` writes, cut down.
    const ENGINE_MARKER_JSON: &str = r#"{
        "platform": "native", "level": "fatal",
        "exception": { "values": [{ "type": "panic",
            "value": "panic message withheld: unflub-engine handles typed text (at src/runtime.rs:1:1)",
            "mechanism": { "type": "panic", "handled": false },
            "stacktrace": { "frames": [{ "instruction_addr": "0x7ff6a0001234" }] } }] },
        "debug_meta": { "images": [{ "type": "symbolic",
            "name": "C:\\Users\\Alex\\AppData\\Local\\unflub\\unflub-engine.exe",
            "code_id": "6738A5B21F000", "id": "00112233-4455-6677-8899-aabbccddeeff-1",
            "debug_file": "unflub_engine.pdb", "image_addr": "0x7ff6a0000000",
            "image_size": 2031616 }] },
        "tags": { "thread": "unflub-hook", "process": "unflub-engine" }
    }"#;

    #[test]
    fn engine_markers_become_scrubbed_reports() {
        let event = event_from_engine(
            ENGINE_MARKER_JSON.as_bytes(),
            "unflub@9.9.9",
            Some(r"C:\Users\Alex"),
        )
        .expect("parses as a Sentry event");
        assert_eq!(event.release.as_deref(), Some("unflub@9.9.9"));
        assert_eq!(event.tags["thread"], "unflub-hook");
        assert_eq!(event.tags["process"], "unflub-engine");
        assert_eq!(event.debug_meta.images.len(), 1);
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.to_lowercase().contains("alex"), "{text}");
        assert!(text.contains("0x7ff6a0001234"), "{text}");
        assert!(text.contains("panic message withheld"), "{text}");

        // A message that isn't the engine's withheld text is withheld here.
        let leaky = ENGINE_MARKER_JSON.replace(
            "panic message withheld: unflub-engine handles typed text (at src/runtime.rs:1:1)",
            "the user typed hunter2",
        );
        let event = event_from_engine(leaky.as_bytes(), "unflub@1", None).unwrap();
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(event_from_engine(b"not json", "unflub@1", None).is_none());
    }

    #[test]
    fn local_builds_have_no_dsn() {
        // CI sets SENTRY_DSN only for release builds; `cargo test` must never send anything.
        if std::env::var_os("SENTRY_DSN").is_none() {
            assert!(!enabled());
        }
    }
}
