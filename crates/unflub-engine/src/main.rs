//! `unflub-engine.exe`: the part of unflub that sees keys. Started by `unflub.exe`, never by
//! hand:
//!
//! ```text
//! unflub-engine --pipe <name> --app-pid <pid> [--crash-file <path>] [--dry-run] [--test-ipc [--test-panic]]
//! ```
//!
//! It connects to the app's pipe, installs the keyboard hook, runs the engine and the pill, and
//! reports to the app. It has no network code at all (`tests/no_network.rs`). It exits when
//! the app says so, when the pipe closes, or with the app's Job Object.
//!
//! - `--dry-run`: the hook watches but never injects or swallows a key (`policy::DRY_RUN`); the
//!   perf scripts use it. Prints `hooked.` or `hook unavailable` once ready.
//! - `--test-ipc`: handshake and messages only, never a hook or a pill. `--test-panic` then
//!   panics on purpose, to test the crash marker.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod crash;
#[cfg(windows)]
mod runtime;

use std::path::PathBuf;

const USAGE: &str = "usage: unflub-engine --pipe <name> --app-pid <pid> [--crash-file <path>] \
[--dry-run] [--test-ipc [--test-panic]]

unflub-engine is started by unflub.exe; it isn't meant to be run by hand.";

#[derive(Debug, Default, PartialEq)]
struct Args {
    pipe: String,
    app_pid: u32,
    crash_file: Option<PathBuf>,
    dry_run: bool,
    test_ipc: bool,
    test_panic: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut pipe = None;
    let mut app_pid = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pipe" => pipe = Some(args.next().ok_or("--pipe needs a name")?),
            "--app-pid" => {
                let pid = args.next().ok_or("--app-pid needs a pid")?;
                app_pid = Some(pid.parse().map_err(|_| format!("bad pid: {pid}"))?);
            }
            "--crash-file" => {
                out.crash_file = Some(args.next().ok_or("--crash-file needs a path")?.into())
            }
            "--dry-run" => out.dry_run = true,
            "--test-ipc" => out.test_ipc = true,
            "--test-panic" => out.test_panic = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    out.pipe = pipe.ok_or("--pipe is required")?;
    if !out.pipe.starts_with(r"\\.\pipe\") {
        return Err("--pipe must be a local pipe name".into());
    }
    out.app_pid = app_pid.ok_or("--app-pid is required")?;
    if out.test_panic && !out.test_ipc {
        return Err("--test-panic needs --test-ipc".into());
    }
    Ok(out)
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    crash::install(args.crash_file.clone());
    #[cfg(windows)]
    std::process::exit(runtime::run(&args));
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("unflub-engine currently runs on Windows only");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags() {
        let a = parse(&[
            "--pipe",
            r"\\.\pipe\unflub-engine-1-ab",
            "--app-pid",
            "42",
            "--crash-file",
            "x.json",
            "--dry-run",
        ])
        .unwrap();
        assert_eq!(
            a,
            Args {
                pipe: r"\\.\pipe\unflub-engine-1-ab".into(),
                app_pid: 42,
                crash_file: Some("x.json".into()),
                dry_run: true,
                ..Args::default()
            }
        );
        let p = r"\\.\pipe\x";
        assert!(parse(&["--pipe", p, "--app-pid", "1", "--test-ipc", "--test-panic"]).is_ok());
        assert!(parse(&["--pipe", p, "--app-pid", "1", "--test-panic"]).is_err());
        assert!(parse(&["--pipe", p]).is_err(), "app pid required");
        assert!(parse(&["--app-pid", "1"]).is_err(), "pipe required");
        assert!(parse(&["--pipe", r"\\server\pipe\x", "--app-pid", "1"]).is_err());
        assert!(parse(&["--pipe", p, "--app-pid", "x"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
