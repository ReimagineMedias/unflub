//! The engine's panic hook. It writes one scrubbed marker for the app's crash-report flow
//! (`app/src-tauri/src/crash.rs`), which decides, under the user's opt-in, whether it is ever
//! sent. Nothing here can send anything.
//!
//! The marker is the subset of a Sentry event the app needs: the panic's location, unresolved
//! frame addresses and the loaded modules (so Sentry can symbolicate against the debug symbols
//! CI uploads), and the thread name. Never the panic message: every thread in this process
//! handles typed text. The home folder is cut out of every path.
//!
//! Any panic ends the process at once with exit code [`PANIC_EXIT`], in debug builds too, so a
//! half-dead engine never keeps the keyboard hooked; the app restarts it. The hook ends it
//! itself rather than returning to the release build's `abort()`, which would also start
//! Windows Error Reporting (seconds of CPU and a dump with whatever was in memory).

use serde_json::{json, Value};
use std::path::PathBuf;

pub const WITHHELD: &str = "panic message withheld: unflub-engine handles typed text";

/// The engine's exit code after a panic (the same as Rust's for an unwinding panic).
pub const PANIC_EXIT: u32 = 101;

pub fn install(path: Option<PathBuf>) {
    std::panic::set_hook(Box::new(move |info| {
        // Keep the first report if several threads panic at once, or a report the app hasn't
        // picked up yet.
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        // The default hook would print the message, which may hold typed text.
        eprintln!(
            "unflub-engine: {WITHHELD} (at {})",
            location.as_deref().unwrap_or("?")
        );
        if let Some(path) = path.as_ref().filter(|p| !p.exists()) {
            let marker = marker(location.as_deref(), home().as_deref());
            if let Ok(bytes) = serde_json::to_vec(&marker) {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, bytes);
            }
        }
        end_now();
    }));
}

/// End the process now, without unwinding, `atexit` handlers or error reporting.
fn end_now() -> ! {
    #[cfg(windows)]
    // SAFETY: terminating our own process; the pseudo handle needs no closing.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), PANIC_EXIT);
    }
    std::process::abort()
}

fn home() -> Option<String> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .filter(|h| h.len() > 3)
}

/// The marker for a panic at `location`, as Sentry event JSON.
fn marker(location: Option<&str>, home: Option<&str>) -> Value {
    let thread = std::thread::current();
    let value = match location {
        Some(at) => format!("{WITHHELD} (at {})", scrub(at, home)),
        None => WITHHELD.to_string(),
    };
    let (frames, images) = native::stack_and_modules(home);
    json!({
        "platform": "native",
        "level": "fatal",
        "exception": { "values": [{
            "type": "panic",
            "value": value,
            "mechanism": { "type": "panic", "handled": false },
            "stacktrace": { "frames": frames },
        }]},
        "debug_meta": { "images": images },
        "tags": {
            "thread": thread.name().unwrap_or("unnamed"),
            "process": "unflub-engine",
        },
    })
}

/// Replace the home folder (either slash style, any case) with `~`.
fn scrub(s: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return s.to_string();
    };
    let mut out = s.to_string();
    for form in [home.replace('/', "\\"), home.replace('\\', "/")] {
        out = replace_ignore_ascii_case(&out, &form, "~");
    }
    out
}

fn replace_ignore_ascii_case(haystack: &str, needle: &str, with: &str) -> String {
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

#[cfg(windows)]
mod native {
    use findshlibs::{SharedLibrary, SharedLibraryId, TargetSharedLibrary};
    use serde_json::{json, Value};

    /// Frame addresses (oldest first, as Sentry wants them) and the loaded PE modules, as
    /// Sentry's `symbolic` debug images (what `sentry::integrations::debug_images` produces).
    pub fn stack_and_modules(home: Option<&str>) -> (Vec<Value>, Vec<Value>) {
        let bt = backtrace::Backtrace::new_unresolved();
        let mut frames: Vec<Value> = bt
            .frames()
            .iter()
            .map(|f| json!({ "instruction_addr": format!("{:#x}", f.ip() as usize) }))
            .collect();
        frames.reverse();

        let mut images = Vec::new();
        TargetSharedLibrary::each(|lib| {
            let Some(SharedLibraryId::PdbSignature(guid, age)) = lib.debug_id() else {
                return;
            };
            let name = super::scrub(&lib.name().to_string_lossy(), home);
            images.push(json!({
                "type": "symbolic",
                "name": name,
                "code_id": lib.id().map(|id| id.to_string()),
                "id": debug_id(&guid, age),
                "debug_file": lib.debug_name().map(|n| super::scrub(&n.to_string_lossy(), home)),
                // On Windows this is the module base, which Sentry symbolicates against.
                "image_addr": format!("{:#x}", lib.virtual_memory_bias().0),
                "image_size": lib.len(),
            }));
        });
        (frames, images)
    }

    /// A PDB GUID (Windows byte order) and age as a Sentry debug id.
    pub fn debug_id(guid: &[u8; 16], age: u32) -> String {
        let d1 = u32::from_le_bytes([guid[0], guid[1], guid[2], guid[3]]);
        let d2 = u16::from_le_bytes([guid[4], guid[5]]);
        let d3 = u16::from_le_bytes([guid[6], guid[7]]);
        let rest: String = guid[8..].iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "{d1:08x}-{d2:04x}-{d3:04x}-{}-{}-{age:x}",
            &rest[..4],
            &rest[4..]
        )
    }
}

#[cfg(not(windows))]
mod native {
    pub fn stack_and_modules(
        _home: Option<&str>,
    ) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        (Vec::new(), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_has_no_message_and_no_home() {
        let home = r"C:\Users\Someone";
        let m = marker(
            Some(r"C:\Users\Someone\src\hook.rs:10:5"),
            Some(&home.to_lowercase()),
        );
        let text = m.to_string();
        assert!(!text.to_lowercase().contains("someone"), "{text}");
        let value = m["exception"]["values"][0]["value"].as_str().unwrap();
        assert!(value.starts_with(WITHHELD), "{value}");
        assert!(value.contains(r"~\src\hook.rs:10:5"), "{value}");
        assert_eq!(m["tags"]["process"], "unflub-engine");
        assert_eq!(m["level"], "fatal");
    }

    #[test]
    fn scrubs_both_slash_styles() {
        let home = Some(r"C:\Users\A b");
        assert_eq!(
            scrub(r"c:\users\a B\x and C:/Users/A b/y", home),
            r"~\x and ~/y"
        );
        assert_eq!(scrub("nothing", None), "nothing");
    }

    #[cfg(windows)]
    #[test]
    fn debug_ids_match_sentry() {
        // GUID {00112233-4455-6677-8899-AABBCCDDEEFF} as stored in a PDB, age 1.
        let guid = [
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ];
        assert_eq!(
            native::debug_id(&guid, 1),
            "00112233-4455-6677-8899-aabbccddeeff-1"
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_marker_lists_frames_and_this_module() {
        let m = marker(None, None);
        assert!(!m["exception"]["values"][0]["stacktrace"]["frames"]
            .as_array()
            .unwrap()
            .is_empty());
        let images = m["debug_meta"]["images"].as_array().unwrap();
        assert!(images.iter().any(|i| i["name"]
            .as_str()
            .is_some_and(|f| f.to_lowercase().ends_with(".exe"))));
    }
}
