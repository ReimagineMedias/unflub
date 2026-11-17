//! The contract between `unflub.exe` (tray app) and `unflub-engine.exe` (keyboard engine).
//!
//! The engine process sees every key, so it is built without any network code: this crate's
//! dependency tree has no HTTP, TLS, Sentry or Tauri crate and no `Win32_Networking` feature
//! (`tests/no_network.rs` fails otherwise). Everything it says goes to the app over one local
//! named pipe:
//!
//! - [`pipe`]: the app creates `\\.\pipe\unflub-engine-<pid>-<random>` with a DACL for the
//!   current user only and `PIPE_REJECT_REMOTE_CLIENTS`, spawns the engine with that name, and
//!   accepts exactly one client whose process id must be the one it spawned.
//! - [`frame`]: each message is a 4-byte little-endian length and a JSON body.
//! - [`protocol`]: the engine says [`Hello`](protocol::FromEngine::Hello) with its protocol
//!   version; the app answers [`Welcome`](protocol::ToEngine::Welcome) or ends the process.
//!
//! [`pill`] maps the pill settings and the current OS look ([`look`]) to the overlay's colours.
//! It lives here because the engine reads the OS look again before each pill.

pub mod frame;
pub mod look;
pub mod pill;
pub mod protocol;

#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod session;

/// File name of the engine executable, beside `unflub.exe` once installed.
pub const ENGINE_EXE: &str = if cfg!(windows) {
    "unflub-engine.exe"
} else {
    "unflub-engine"
};
