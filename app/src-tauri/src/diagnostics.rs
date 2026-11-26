//! Local operational diagnostics. Callers can only supply fixed event types and numbers.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use unflub_engine::protocol::{EngineDiagnostic, ReinstallReason};

const LIMIT: u64 = 256 * 1024;
static DIRECTORY: OnceLock<Mutex<PathBuf>> = OnceLock::new();

pub enum Event {
    AppStarted,
    SettingsSaveFailed(io::ErrorKind),
    UsageSaveFailed(io::ErrorKind),
    #[cfg(windows)]
    SupervisorStartFailed(io::ErrorKind),
    #[cfg(windows)]
    EngineSpawned,
    #[cfg(windows)]
    EngineStartupFailed(io::ErrorKind),
    #[cfg(windows)]
    EngineReady,
    #[cfg(windows)]
    HookStartFailed,
    #[cfg(windows)]
    ReadyTimedOut,
    #[cfg(windows)]
    EngineExit(i32),
    #[cfg(windows)]
    EngineTerminated,
    #[cfg(windows)]
    RestartAfter(u64),
    #[cfg(windows)]
    HeartbeatFailed,
    #[cfg(windows)]
    PipeReadFailed(io::ErrorKind),
    #[cfg(windows)]
    PipeWriteFailed(io::ErrorKind),
    #[cfg(windows)]
    InvalidEngineMessage,
    HookReinstalled(Option<ReinstallReason>),
    CorrectorPanicked,
    InputInterference(bool),
    Engine(EngineDiagnostic),
}

impl Event {
    fn line(self) -> String {
        match self {
            Self::AppStarted => "app_started".into(),
            Self::SettingsSaveFailed(kind) => format!("settings_save_failed kind={kind:?}"),
            Self::UsageSaveFailed(kind) => format!("usage_save_failed kind={kind:?}"),
            #[cfg(windows)]
            Self::SupervisorStartFailed(kind) => format!("supervisor_start_failed kind={kind:?}"),
            #[cfg(windows)]
            Self::EngineSpawned => "engine_spawned".into(),
            #[cfg(windows)]
            Self::EngineStartupFailed(kind) => format!("engine_startup_failed kind={kind:?}"),
            #[cfg(windows)]
            Self::EngineReady => "engine_ready".into(),
            #[cfg(windows)]
            Self::HookStartFailed => "hook_start_failed".into(),
            #[cfg(windows)]
            Self::ReadyTimedOut => "ready_timed_out".into(),
            #[cfg(windows)]
            Self::EngineExit(code) => format!("engine_exit code={code}"),
            #[cfg(windows)]
            Self::EngineTerminated => "engine_terminated".into(),
            #[cfg(windows)]
            Self::RestartAfter(seconds) => format!("engine_retry seconds={seconds}"),
            #[cfg(windows)]
            Self::HeartbeatFailed => "heartbeat_failed".into(),
            #[cfg(windows)]
            Self::PipeReadFailed(kind) => format!("pipe_read_failed kind={kind:?}"),
            #[cfg(windows)]
            Self::PipeWriteFailed(kind) => format!("pipe_write_failed kind={kind:?}"),
            #[cfg(windows)]
            Self::InvalidEngineMessage => "invalid_engine_message".into(),
            Self::HookReinstalled(reason) => format!("hook_reinstalled reason={reason:?}"),
            Self::CorrectorPanicked => "corrector_panicked".into(),
            Self::InputInterference(active) => format!("input_interference active={active}"),
            Self::Engine(EngineDiagnostic::HookWarning) => "hook_warning".into(),
            Self::Engine(EngineDiagnostic::UiaWarning) => "uia_warning".into(),
            Self::Engine(EngineDiagnostic::PillUnavailable) => "pill_unavailable".into(),
            Self::Engine(EngineDiagnostic::InjectionPartial { sent, total }) => {
                format!("injection_partial sent={sent} total={total}")
            }
        }
    }
}

pub fn init(dir: &Path) {
    let _ = DIRECTORY.set(Mutex::new(dir.to_owned()));
    record(Event::AppStarted);
}

pub fn record(event: Event) {
    let Some(dir) = DIRECTORY.get().and_then(|dir| dir.lock().ok()) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = append(&dir, &format!("{now} {}\n", event.line()));
}

fn append(dir: &Path, line: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let current = dir.join("diagnostics.log");
    match fs::metadata(&current) {
        Ok(meta) if meta.len().saturating_add(line.len() as u64) > LIMIT => {
            let previous = dir.join("diagnostics.1.log");
            match fs::rename(previous, dir.join("diagnostics.2.log")) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            }
            fs::rename(&current, dir.join("diagnostics.1.log"))?;
        }
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(current)?
        .write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_limit_and_keeps_two_backups() {
        let dir = std::env::temp_dir().join(format!("unflub-diagnostics-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("diagnostics.log");
        for byte in *b"abc" {
            fs::write(&current, vec![byte; LIMIT as usize]).unwrap();
            append(&dir, "app_started\n").unwrap();
        }
        assert_eq!(fs::read(&current).unwrap(), b"app_started\n");
        assert_eq!(
            fs::read(dir.join("diagnostics.1.log")).unwrap(),
            vec![b'c'; LIMIT as usize]
        );
        assert_eq!(
            fs::read(dir.join("diagnostics.2.log")).unwrap(),
            vec![b'b'; LIMIT as usize]
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
