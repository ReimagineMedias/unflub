//! Runs and supervises the keyboard engine, a separate process: `unflub-engine.exe`
//! (`crates/unflub-engine`). That process holds the keyboard hook, the engine, the OS spell
//! checker and the pill, and has no network code at all. This process has the network
//! (updates, remote config, usage stats, crash reports) and never sees a key.
//!
//! [`EngineHost`] owns one `unflub-supervisor` thread:
//!
//! - **Start.** It creates a named pipe for the current user only, spawns the engine beside
//!   `unflub.exe` in a kill-on-close Job Object, accepts only the pid it spawned, checks the
//!   protocol version, and sends the settings ([`engine_settings`]).
//! - **Settings and remote flags** go to the engine as one `Configure` message on every change;
//!   it applies them live (the hook thread picks the new config up at the next key).
//! - **Pause** ends the engine process; resume starts a new one. While paused nothing holds a
//!   keyboard hook or a single typed character, and the engine's memory is returned. Unhooking
//!   inside a live engine would keep ~20 MB and a process that could still see keys for a
//!   resume that takes only ~0.3 s anyway (most of it the dictionary index, which an
//!   in-process resume rebuilt too).
//! - **Crashes** restart it with backoff: 1 s, doubling up to 5 min, back to 1 s once it has run
//!   for a minute. A panic leaves a scrubbed marker that `crash.rs` turns into the usual
//!   opt-in report.
//! - **Quit and updates** send `Shutdown` and wait for the process (killing it after 3 s); the
//!   updater calls this before the installer runs, so `unflub-engine.exe` is never locked.
//!
//! What the engine reports arrives on an `unflub-engine-io` thread as [`Observed`] (local stats,
//! usage counts), learned words, and words-checked counts.
//!
//! On macOS and Linux this is a no-op until their adapters exist.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::remote::RemoteFlags;
use crate::settings::{self, Appearance, Settings, Theme};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use unflub_engine::protocol::{self, EngineSettings, PillSettings};

pub use host::EngineHost;

/// What the engine did that the app may count. Words stay on this machine: they reach only the
/// local Stats page.
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    Corrected {
        original: String,
        corrected: String,
        /// The user's own shortcut, not a typo fix.
        user_rule: bool,
        decide: Duration,
        inject: Duration,
        /// The pill was asked to show it.
        pill: bool,
        /// The local Stats page may keep the words (not in terminals).
        keep_word: bool,
    },
    Undone {
        original: String,
        corrected: String,
        user_rule: bool,
        /// The pair may be kept (not in terminals or unknown apps).
        keep_word: bool,
    },
    CorrectorPanicked,
    /// The engine's watchdog installed the hooks again (Windows dropped them, or the computer
    /// resumed or was unlocked).
    HookReinstalled,
    /// The engine process ended without being asked to. It is restarted with backoff.
    EngineCrashed,
    /// The engine paused itself because its injected input stopped coming back (another
    /// program is interfering with keyboard input), or (`!active`) it works again.
    InputInterference {
        active: bool,
    },
}

pub type Observer = Arc<dyn Fn(Observed) + Send + Sync>;

/// How to start the engine.
#[derive(Debug, Clone)]
pub struct Launch {
    pub exe: PathBuf,
    /// Where a panicking engine leaves its marker (`crash.rs` reads it).
    pub crash_file: PathBuf,
    /// More engine arguments: `--dry-run` for `--perf-idle` runs (the hook only watches), or
    /// `--test-ipc` in tests (no hook at all).
    pub args: Vec<String>,
}

impl Launch {
    /// `unflub-engine.exe` beside this executable.
    pub fn beside_app(crash_file: PathBuf, dry_run: bool) -> Option<Self> {
        let exe = std::env::current_exe()
            .ok()?
            .with_file_name(unflub_engine::ENGINE_EXE);
        let args = if dry_run {
            vec!["--dry-run".to_string()]
        } else {
            Vec::new()
        };
        Some(Self {
            exe,
            crash_file,
            args,
        })
    }
}

/// What the engine runs with: the user's settings with the remote flags applied, and the undo
/// counts so far. Remote flags only ever switch the pill or its UI Automation lookup off.
pub fn engine_settings(
    s: &Settings,
    flags: &RemoteFlags,
    undo_counts: Vec<(String, String, u32)>,
) -> EngineSettings {
    EngineSettings {
        core: unflub_core::Config {
            undo_counts,
            ..s.to_core()
        },
        pill: PillSettings {
            style: match s.pill_style {
                _ if !flags.pill_enabled => protocol::PillStyle::Off,
                settings::PillStyle::UnderlineCapsule => protocol::PillStyle::UnderlineCapsule,
                settings::PillStyle::Capsule => protocol::PillStyle::Capsule,
                settings::PillStyle::Underline => protocol::PillStyle::Underline,
                settings::PillStyle::Off => protocol::PillStyle::Off,
            },
            position: match s.pill_position {
                settings::PillPosition::Above => protocol::PillPosition::Above,
                settings::PillPosition::Below => protocol::PillPosition::Below,
            },
            opacity: s.pill_opacity,
            duration_ms: s.pill_duration_ms,
            theme: match s.theme {
                Theme::Unflub => protocol::Theme::Unflub,
                Theme::Paper => protocol::Theme::Paper,
                Theme::Ink => protocol::Theme::Ink,
                Theme::System => protocol::Theme::System,
            },
            appearance: match s.appearance {
                Appearance::Auto => protocol::Appearance::Auto,
                Appearance::Light => protocol::Appearance::Light,
                Appearance::Dark => protocol::Appearance::Dark,
            },
            uia_caret: flags.uia_caret,
        },
    }
}

/// Wait before restart number `failures` (1-based): 1 s, 2 s, 4 s ... up to 5 minutes.
pub fn backoff(failures: u32) -> Duration {
    const MAX: Duration = Duration::from_secs(5 * 60);
    let shift = failures.saturating_sub(1).min(16);
    Duration::from_secs(1u64 << shift).min(MAX)
}

/// A run at least this long counts as healthy: the next crash starts the backoff over.
pub const STABLE_RUN: Duration = Duration::from_secs(60);

/// Consecutive failures after a run of `ran` that ended badly, given `failures` before it.
pub fn failures_after(failures: u32, ran: Duration) -> u32 {
    if ran >= STABLE_RUN {
        1
    } else {
        failures.saturating_add(1)
    }
}

#[cfg(windows)]
mod host {
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use unflub_engine::job::KillOnClose;
    use unflub_engine::pipe::{self, Listener, Pipe};
    use unflub_engine::protocol::{FromEngine, ToEngine};
    use unflub_engine::{frame, session};

    use super::{backoff, engine_settings, failures_after, Launch, Observed, Observer};
    use crate::remote::RemoteFlags;
    use crate::settings::Settings;
    use crate::undo_counts::UndoCounts;

    /// No console window for the engine (it inherits our std handles, if any).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// How long a shutdown may take before the engine is killed.
    const STOP_TIMEOUT: Duration = Duration::from_secs(3);
    /// How long the `Shutdown` message may wait for an engine that stopped reading.
    const SHUTDOWN_WRITE: Duration = Duration::from_millis(500);

    type Learned = Arc<Mutex<Box<dyn FnMut(String) + Send>>>;

    enum Cmd {
        Apply(Box<Settings>),
        Flags(RemoteFlags),
        /// The engine of this generation closed its pipe.
        Exited(u64),
        Shutdown,
    }

    /// The supervised engine. Every method returns at once except [`shutdown`](Self::shutdown).
    pub struct EngineHost {
        tx: Sender<Cmd>,
        thread: Option<JoinHandle<()>>,
    }

    impl EngineHost {
        /// Start supervising: the engine starts at once if `settings.enabled`. `on_learned` runs
        /// for each word learned by undo; `observer` for each fix, undo and health event; the
        /// engine's checked-word counts are added to `words_checked`.
        pub fn start(
            launch: Option<Launch>,
            undo_counts: Arc<UndoCounts>,
            settings: Settings,
            flags: RemoteFlags,
            words_checked: Arc<AtomicU64>,
            observer: Observer,
            on_learned: impl FnMut(String) + Send + 'static,
        ) -> Self {
            let (tx, rx) = mpsc::channel();
            let supervisor = Supervisor {
                launch,
                undo_counts,
                settings,
                flags,
                tx: tx.clone(),
                words_checked,
                observer,
                on_learned: Arc::new(Mutex::new(Box::new(on_learned))),
                running: None,
                generation: 0,
                failures: 0,
                next_start: None,
            };
            let thread = std::thread::Builder::new()
                .name("unflub-supervisor".into())
                .spawn(move || supervisor.run(rx))
                .map_err(|e| eprintln!("autocorrect: can't start the supervisor: {e}"))
                .ok();
            Self { tx, thread }
        }

        /// Apply new settings (already sanitised). Pausing ends the engine; resuming starts it.
        pub fn apply(&self, settings: &Settings) {
            let _ = self.tx.send(Cmd::Apply(Box::new(settings.clone())));
        }

        /// Apply new remote flags.
        pub fn set_flags(&self, flags: RemoteFlags) {
            let _ = self.tx.send(Cmd::Flags(flags));
        }

        /// Stop the engine process and the supervisor.
        pub fn shutdown(mut self) {
            self.stop();
        }

        fn stop(&mut self) {
            if let Some(thread) = self.thread.take() {
                let _ = self.tx.send(Cmd::Shutdown);
                let _ = thread.join();
            }
        }
    }

    impl Drop for EngineHost {
        fn drop(&mut self) {
            self.stop();
        }
    }

    struct Running {
        child: Child,
        pipe: Arc<Pipe>,
        /// Ends the engine if this process dies first.
        _job: Option<KillOnClose>,
        generation: u64,
        started: Instant,
        reader: Option<JoinHandle<()>>,
    }

    struct Supervisor {
        launch: Option<Launch>,
        undo_counts: Arc<UndoCounts>,
        settings: Settings,
        flags: RemoteFlags,
        tx: Sender<Cmd>,
        words_checked: Arc<AtomicU64>,
        observer: Observer,
        on_learned: Learned,
        running: Option<Running>,
        generation: u64,
        /// Failed starts and crashes in a row.
        failures: u32,
        /// When to try again after a failure.
        next_start: Option<Instant>,
    }

    impl Supervisor {
        fn run(mut self, rx: Receiver<Cmd>) {
            loop {
                self.converge();
                let cmd = match self.next_start {
                    Some(at) => {
                        match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                            Ok(cmd) => cmd,
                            Err(RecvTimeoutError::Timeout) => {
                                self.next_start = None;
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => Cmd::Shutdown,
                        }
                    }
                    None => rx.recv().unwrap_or(Cmd::Shutdown),
                };
                match cmd {
                    Cmd::Apply(next) => {
                        let resumed = !self.settings.enabled && next.enabled;
                        self.settings = *next;
                        if resumed {
                            // The user asked for it: try now, whatever the backoff said.
                            self.failures = 0;
                            self.next_start = None;
                        }
                        self.configure();
                    }
                    Cmd::Flags(flags) => {
                        self.flags = flags;
                        self.configure();
                    }
                    Cmd::Exited(generation) => self.on_exit(generation),
                    Cmd::Shutdown => break,
                }
            }
            self.stop();
        }

        /// Start or stop the engine to match the settings.
        fn converge(&mut self) {
            if !self.settings.enabled {
                // Pause: no process, no hook.
                self.stop();
                self.next_start = None;
                return;
            }
            if self.running.is_some() || self.next_start.is_some() {
                return;
            }
            match self.spawn() {
                Ok(running) => self.running = Some(running),
                Err(e) => {
                    eprintln!("autocorrect: can't start the engine: {e}");
                    self.failures = self.failures.saturating_add(1);
                    self.next_start = Some(Instant::now() + backoff(self.failures));
                }
            }
        }

        fn engine_settings(&self) -> unflub_engine::protocol::EngineSettings {
            let mut s = engine_settings(&self.settings, &self.flags, self.undo_counts.snapshot());
            s.core.undo_counts_epoch = self.undo_counts.epoch();
            s
        }

        fn configure(&mut self) {
            let Some(r) = &self.running else {
                return;
            };
            let settings = self.engine_settings();
            // The write has a deadline (`pipe::WRITE_TIMEOUT`). An engine that stopped reading,
            // or a write cut short, ends this session: kill it and restart with backoff.
            if let Err(e) = frame::write(&mut &*r.pipe, &ToEngine::Configure { settings }) {
                eprintln!("autocorrect: can't reach the engine ({e}); restarting it");
                self.end_session();
            }
        }

        /// The pipe failed: end the engine without asking (asking needs the pipe) and restart
        /// it with backoff, like a crash.
        fn end_session(&mut self) {
            let Some(mut running) = self.running.take() else {
                return;
            };
            running.pipe.cancel_io();
            let _ = running.child.kill();
            let _ = running.child.wait();
            if let Some(reader) = running.reader.take() {
                let _ = reader.join();
            }
            (self.observer)(Observed::EngineCrashed);
            self.failures = failures_after(self.failures, running.started.elapsed());
            self.next_start = Some(Instant::now() + super::backoff(self.failures));
        }

        fn spawn(&mut self) -> std::io::Result<Running> {
            let launch = self
                .launch
                .as_ref()
                .ok_or_else(|| std::io::Error::other("no engine executable"))?;
            let name = pipe::unique_name();
            let listener = Listener::create(&name)?;
            let mut command = Command::new(&launch.exe);
            command
                .arg("--pipe")
                .arg(&name)
                .arg("--app-pid")
                .arg(std::process::id().to_string())
                .arg("--crash-file")
                .arg(&launch.crash_file)
                .stdin(Stdio::null())
                .args(&launch.args)
                .creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(|e| {
                std::io::Error::new(e.kind(), format!("{}: {e}", launch.exe.display()))
            })?;
            let job = KillOnClose::new()
                .and_then(|job| job.assign(&child).map(|()| job))
                .map_err(|e| eprintln!("autocorrect: engine job object: {e}"))
                .ok();
            let pipe = match session::serve(
                listener,
                child.id(),
                &own_exe(),
                session::HANDSHAKE_TIMEOUT,
            ) {
                Ok((pipe, _version)) => Arc::new(pipe),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e);
                }
            };
            self.generation += 1;
            let generation = self.generation;
            let settings = self.engine_settings();
            if let Err(e) = frame::write(&mut &*pipe, &ToEngine::Configure { settings }) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            let reader = {
                let pipe = pipe.clone();
                let tx = self.tx.clone();
                let observer = self.observer.clone();
                let learned = self.on_learned.clone();
                let words = self.words_checked.clone();
                std::thread::Builder::new()
                    .name("unflub-engine-io".into())
                    .spawn(move || {
                        read_engine(&pipe, &observer, &learned, &words);
                        let _ = tx.send(Cmd::Exited(generation));
                    })?
            };
            Ok(Running {
                child,
                pipe,
                _job: job,
                generation,
                started: Instant::now(),
                reader: Some(reader),
            })
        }

        /// The engine's pipe closed without our asking: count it and restart with backoff.
        fn on_exit(&mut self, generation: u64) {
            if self
                .running
                .as_ref()
                .is_none_or(|r| r.generation != generation)
            {
                return;
            }
            let Some(mut running) = self.running.take() else {
                return;
            };
            let status = wait_for(&mut running.child, STOP_TIMEOUT);
            if let Some(reader) = running.reader.take() {
                let _ = reader.join();
            }
            let crashed = status.is_none_or(|s| !s.success());
            eprintln!(
                "autocorrect: the engine stopped ({}); restarting",
                status.map_or("killed".into(), |s| s.to_string())
            );
            if crashed {
                (self.observer)(Observed::EngineCrashed);
            }
            self.failures = failures_after(self.failures, running.started.elapsed());
            self.next_start = Some(Instant::now() + super::backoff(self.failures));
        }

        /// Ask the engine to stop, then make sure it has.
        fn stop(&mut self) {
            let Some(mut running) = self.running.take() else {
                return;
            };
            // Both steps have deadlines, so a stuck engine is killed within about 3.5 s: the
            // write gives up after `SHUTDOWN_WRITE`, and a failed write skips the wait.
            running.pipe.set_write_timeout(Some(SHUTDOWN_WRITE));
            let asked = frame::write(&mut &*running.pipe, &ToEngine::Shutdown).is_ok();
            if !asked || wait_for(&mut running.child, STOP_TIMEOUT).is_none() {
                eprintln!("autocorrect: the engine didn't stop; ending it");
                let _ = running.child.kill();
                let _ = running.child.wait();
            }
            running.pipe.cancel_io();
            if let Some(reader) = running.reader.take() {
                let _ = reader.join();
            }
        }
    }

    /// Wait up to `timeout` for the child. `None` if it is still running.
    fn wait_for(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => return None,
            }
        }
    }

    /// Hand the engine's messages to the app until its pipe closes.
    fn read_engine(pipe: &Pipe, observer: &Observer, learned: &Learned, words: &AtomicU64) {
        loop {
            let body = match frame::read(&mut &*pipe) {
                Ok(Some(body)) => body,
                Ok(None) | Err(_) => return,
            };
            let message = match frame::decode::<FromEngine>(&body) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("autocorrect: skipped an engine message ({e})");
                    continue;
                }
            };
            match message {
                FromEngine::Corrected {
                    original,
                    corrected,
                    user_rule,
                    decide_us,
                    inject_us,
                    pill,
                    keep_word,
                } => observer(Observed::Corrected {
                    keep_word,
                    original,
                    corrected,
                    user_rule,
                    decide: Duration::from_micros(decide_us),
                    inject: Duration::from_micros(inject_us),
                    pill,
                }),
                FromEngine::Undone {
                    original,
                    corrected,
                    user_rule,
                    keep_word,
                } => observer(Observed::Undone {
                    original,
                    corrected,
                    user_rule,
                    keep_word,
                }),
                FromEngine::WordsChecked { count } => {
                    words.fetch_add(count, Ordering::Relaxed);
                }
                FromEngine::Learned { word } => {
                    (learned.lock().unwrap_or_else(|e| e.into_inner()))(word)
                }
                FromEngine::HookReinstalled => observer(Observed::HookReinstalled),
                FromEngine::CorrectorPanicked => observer(Observed::CorrectorPanicked),
                FromEngine::InputInterference { active } => {
                    observer(Observed::InputInterference { active })
                }
                FromEngine::Ready { hooked } => {
                    if !hooked {
                        eprintln!("autocorrect: the engine couldn't install the keyboard hook");
                    }
                }
                FromEngine::Hello { .. } => {}
            }
        }
    }

    /// unflub's own executable name, lowercased, as the hook names the focused app.
    fn own_exe() -> String {
        std::env::current_exe()
            .ok()
            .and_then(|p| Some(p.file_name()?.to_str()?.to_lowercase()))
            .unwrap_or_else(|| "unflub.exe".into())
    }
}

#[cfg(not(windows))]
mod host {
    use super::{Launch, Observer};
    use crate::remote::RemoteFlags;
    use crate::settings::Settings;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    /// No adapter on this OS yet: settings are stored, nothing is hooked.
    pub struct EngineHost;

    impl EngineHost {
        #[allow(clippy::too_many_arguments)]
        pub fn start(
            _launch: Option<Launch>,
            _undo_counts: Arc<crate::undo_counts::UndoCounts>,
            _settings: Settings,
            _flags: RemoteFlags,
            _words_checked: Arc<AtomicU64>,
            _observer: Observer,
            _on_learned: impl FnMut(String) + Send + 'static,
        ) -> Self {
            Self
        }

        pub fn apply(&self, _settings: &Settings) {}

        pub fn set_flags(&self, _flags: RemoteFlags) {}

        pub fn shutdown(self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_settings_for_the_engine() {
        let s = Settings {
            pill_style: settings::PillStyle::Capsule,
            pill_position: settings::PillPosition::Below,
            pill_opacity: 0.5,
            pill_duration_ms: 2000,
            theme: Theme::Paper,
            appearance: Appearance::Dark,
            ..Settings::default()
        };
        let counts = vec![("teh".to_string(), "the".to_string(), 1)];
        let e = engine_settings(&s, &RemoteFlags::default(), counts.clone());
        assert_eq!(e.core.undo_counts, counts);
        assert_eq!(e.core.undo_window_ms, 2000, "the pill duration");
        assert_eq!(
            e.core,
            unflub_core::Config {
                undo_counts: counts,
                ..s.to_core()
            }
        );
        assert_eq!(
            e.pill,
            PillSettings {
                style: protocol::PillStyle::Capsule,
                position: protocol::PillPosition::Below,
                opacity: 0.5,
                duration_ms: 2000,
                theme: protocol::Theme::Paper,
                appearance: protocol::Appearance::Dark,
                uia_caret: true,
            }
        );
        // The app's defaults are the engine's defaults, but for the app's slightly
        // translucent pill.
        assert_eq!(
            engine_settings(&Settings::default(), &RemoteFlags::default(), Vec::new()).pill,
            PillSettings {
                opacity: Settings::default().pill_opacity,
                ..PillSettings::default()
            }
        );
        for (from, to) in [
            (
                settings::PillStyle::UnderlineCapsule,
                protocol::PillStyle::UnderlineCapsule,
            ),
            (
                settings::PillStyle::Underline,
                protocol::PillStyle::Underline,
            ),
            (settings::PillStyle::Off, protocol::PillStyle::Off),
        ] {
            let s = Settings {
                pill_style: from,
                ..Settings::default()
            };
            assert_eq!(
                engine_settings(&s, &RemoteFlags::default(), Vec::new())
                    .pill
                    .style,
                to
            );
        }
        for (from, to) in [
            (Theme::Unflub, protocol::Theme::Unflub),
            (Theme::Ink, protocol::Theme::Ink),
            (Theme::System, protocol::Theme::System),
        ] {
            let s = Settings {
                theme: from,
                ..Settings::default()
            };
            assert_eq!(
                engine_settings(&s, &RemoteFlags::default(), Vec::new())
                    .pill
                    .theme,
                to
            );
        }
    }

    #[test]
    fn remote_flags_turn_the_pill_and_uia_off() {
        let flags = RemoteFlags {
            pill_enabled: false,
            uia_caret: false,
            ..RemoteFlags::default()
        };
        let p = engine_settings(&Settings::default(), &flags, Vec::new()).pill;
        assert_eq!(p.style, protocol::PillStyle::Off);
        assert!(!p.uia_caret);
    }

    #[test]
    fn restarts_back_off_and_recover() {
        let secs: Vec<u64> = (1..=11).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(u32::MAX), Duration::from_secs(300));
        // Quick crashes add up; a healthy run starts over.
        assert_eq!(failures_after(3, Duration::from_secs(5)), 4);
        assert_eq!(failures_after(9, STABLE_RUN), 1);
    }

    /// The engine Cargo built next to this test (`target/<profile>/`), if any. `cargo test
    /// --workspace` builds it before running any test; `-p unflub-app` alone doesn't.
    #[cfg(windows)]
    fn built_engine() -> Option<PathBuf> {
        let deps = std::env::current_exe().ok()?;
        let exe = deps.parent()?.parent()?.join(unflub_engine::ENGINE_EXE);
        exe.exists().then_some(exe)
    }

    /// The real engine in `--test-ipc --test-panic` mode (never a hook): it panics after each
    /// start, so the supervisor must see the crash, find the marker, restart it after the
    /// backoff, and stop restarting on pause.
    #[cfg(windows)]
    #[test]
    fn supervises_a_real_engine_through_crashes_and_pause() {
        use std::sync::{mpsc, Mutex};
        use std::time::Instant;

        let Some(exe) = built_engine() else {
            eprintln!("skipped: build unflub-engine first (cargo test --workspace does)");
            return;
        };
        let dir = std::env::temp_dir().join(format!("unflub-supervisor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("engine-crash.json");
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let observer: Observer = Arc::new(move |event| {
            let _ = tx.lock().unwrap().send(event);
        });
        let launch = Launch {
            exe,
            crash_file: marker.clone(),
            args: vec!["--test-ipc".into(), "--test-panic".into()],
        };
        let host = EngineHost::start(
            Some(launch),
            Arc::new(crate::undo_counts::UndoCounts::new(&dir, true)),
            Settings::default(),
            RemoteFlags::default(),
            Arc::default(),
            observer,
            |_| {},
        );
        let wait = Duration::from_secs(30);
        assert_eq!(rx.recv_timeout(wait), Ok(Observed::EngineCrashed));
        assert!(marker.exists(), "the panic left its marker");
        std::fs::remove_file(&marker).unwrap();

        let since = Instant::now();
        assert_eq!(rx.recv_timeout(wait), Ok(Observed::EngineCrashed));
        assert!(
            since.elapsed() >= Duration::from_millis(900),
            "restarted after the 1 s backoff, not at once ({:?})",
            since.elapsed()
        );

        // Pause: the pending restart (2 s backoff now) is dropped.
        host.apply(&Settings {
            enabled: false,
            ..Settings::default()
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(4)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        host.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_engine_is_found_beside_the_app() {
        let launch = Launch::beside_app("crash.json".into(), false).unwrap();
        assert_eq!(
            launch.exe.file_name().unwrap(),
            std::ffi::OsStr::new(unflub_engine::ENGINE_EXE)
        );
        assert_eq!(
            launch.exe.parent(),
            std::env::current_exe().unwrap().parent()
        );
    }
}
