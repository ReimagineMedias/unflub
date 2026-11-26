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
//!   opt-in report. An engine that can't install the keyboard hook (`Ready { hooked: false }`)
//!   or never says it is ready within [`READY_TIMEOUT`] counts as a failed start: it is stopped
//!   and started again with the same backoff, and the tray says so meanwhile
//!   ([`Observed::HookUnavailable`]).
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
use unflub_engine::protocol::{
    self, AppCategory, EngineSettings, FixSource, PillSettings, ReinstallReason, Trigger,
};

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
        /// What ended the word, which part of the engine fixed it and in what kind of app, for
        /// the usage stats.
        trigger: Trigger,
        source: FixSource,
        category: AppCategory,
    },
    /// A first Backspace after a fix is held (the pill asks "Undo?").
    UndoArmed,
    Undone {
        original: String,
        corrected: String,
        user_rule: bool,
        /// The pair may be kept (not in terminals or unknown apps).
        keep_word: bool,
    },
    CorrectorPanicked,
    Diagnostic(protocol::EngineDiagnostic),
    /// The engine installed the hooks again: its watchdog saw them miss input, or the computer
    /// resumed or was unlocked (`None` from an engine that didn't say).
    HookReinstalled {
        reason: Option<ReinstallReason>,
    },
    /// A start couldn't install the keyboard hook or never got ready (each failed start, unlike
    /// [`Observed::HookUnavailable`], which only reports changes).
    HookStartFailed,
    /// Counts and timings from the engine for the usage stats.
    UsageCounts(crate::usage::EngineCounts),
    /// The engine process ended without being asked to. It is restarted with backoff.
    EngineCrashed,
    /// The engine paused itself because its injected input stopped coming back (another
    /// program is interfering with keyboard input), or (`!active`) it works again.
    InputInterference {
        active: bool,
    },
    /// Autocorrect is enabled and the engine has no working session yet.
    /// Cleared after readiness or an intentional pause.
    HookUnavailable {
        active: bool,
    },
}

/// How long a started engine may take to report `Ready` before it counts as a failed start.
pub const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Is `Ready { hooked }` a failed start? Only when the engine was meant to hook the keyboard.
fn hook_start_failed(expects_hook: bool, hooked: bool) -> bool {
    expects_hook && !hooked
}

/// When the supervisor next has to act on its own: a restart after backoff, or a started
/// engine's readiness deadline, whichever is first.
fn next_wake(
    next_start: Option<std::time::Instant>,
    ready_by: Option<std::time::Instant>,
) -> Option<std::time::Instant> {
    match (next_start, ready_by) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
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
    use unflub_engine::pipe::{self, Cancellation, Listener, Pipe};
    use unflub_engine::protocol::{FromEngine, ToEngine};
    use unflub_engine::{frame, session};

    use super::{
        backoff, engine_settings, failures_after, Launch, Observed, Observer, READY_TIMEOUT,
    };
    use crate::diagnostics::{self, Event};
    use crate::remote::RemoteFlags;
    use crate::settings::Settings;
    use crate::undo_counts::UndoCounts;

    /// No console window for the engine (it inherits our std handles, if any).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// How long a shutdown may take before the engine is killed.
    const STOP_TIMEOUT: Duration = Duration::from_secs(3);
    /// How long the `Shutdown` message may wait for an engine that stopped reading.
    const SHUTDOWN_WRITE: Duration = Duration::from_millis(500);
    const PING_EVERY: Duration = Duration::from_secs(15);
    const MISSED_PINGS: u8 = 2;

    type Learned = Arc<Mutex<Box<dyn FnMut(String) + Send>>>;

    enum Cmd {
        Apply(Box<Settings>, u64),
        Flags(RemoteFlags),
        /// The engine of this generation closed its pipe.
        Exited(u64),
        /// The engine of this generation reported `Ready`.
        Ready {
            generation: u64,
            hooked: bool,
        },
        Pong {
            generation: u64,
            nonce: u64,
        },
        SampleMemory(Sender<Option<u64>>),
        Shutdown,
    }

    /// The supervised engine. Every method returns at once except [`shutdown`](Self::shutdown).
    pub struct EngineHost {
        tx: Sender<Cmd>,
        startup_epoch: Arc<AtomicU64>,
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
            // `--test-ipc` engines never hook, on purpose.
            let expects_hook = launch
                .as_ref()
                .is_some_and(|l| !l.args.iter().any(|a| a == "--test-ipc"));
            let startup_epoch = Arc::new(AtomicU64::new(0));
            let supervisor = Supervisor {
                startup_epoch: startup_epoch.clone(),
                applied_epoch: 0,
                expects_hook,
                hook_unavailable: false,
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
            let startup_observer = supervisor.observer.clone();
            let enabled = supervisor.settings.enabled;
            let thread = std::thread::Builder::new()
                .name("unflub-supervisor".into())
                .spawn(move || supervisor.run(rx))
                .inspect_err(|e| diagnostics::record(Event::SupervisorStartFailed(e.kind())))
                .ok();
            if enabled && thread.is_none() {
                startup_observer(Observed::HookUnavailable { active: true });
            }
            Self {
                tx,
                startup_epoch,
                thread,
            }
        }

        /// Apply new settings (already sanitised). Pausing ends the engine; resuming starts it.
        pub fn apply(&self, settings: &Settings) {
            let epoch = if settings.enabled {
                self.startup_epoch.load(Ordering::Relaxed)
            } else {
                self.startup_epoch
                    .fetch_add(1, Ordering::Relaxed)
                    .wrapping_add(1)
            };
            let _ = self.tx.send(Cmd::Apply(Box::new(settings.clone()), epoch));
        }

        /// Apply new remote flags.
        pub fn set_flags(&self, flags: RemoteFlags) {
            let _ = self.tx.send(Cmd::Flags(flags));
        }

        /// Ask for a sample without holding a lock while the supervisor responds.
        pub fn sample_memory(&self) -> std::sync::mpsc::Receiver<Option<u64>> {
            let (tx, rx) = mpsc::channel();
            let _ = self.tx.send(Cmd::SampleMemory(tx));
            rx
        }

        /// Stop the engine process and the supervisor.
        pub fn shutdown(mut self) {
            self.stop();
        }

        fn stop(&mut self) {
            if let Some(thread) = self.thread.take() {
                self.startup_epoch.fetch_add(1, Ordering::Relaxed);
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

    struct Heartbeat {
        next: Instant,
        nonce: u64,
        waiting: bool,
        missed: u8,
    }

    impl Heartbeat {
        fn new(now: Instant) -> Self {
            Self {
                next: now + PING_EVERY,
                nonce: 0,
                waiting: false,
                missed: 0,
            }
        }

        /// Send the next probe, or end the session after consecutive missed replies.
        fn probe(&mut self, now: Instant) -> Option<u64> {
            self.next = now + PING_EVERY;
            if self.waiting {
                self.missed += 1;
            }
            if self.missed >= MISSED_PINGS {
                return None;
            }
            self.nonce = self.nonce.wrapping_add(1);
            self.waiting = true;
            Some(self.nonce)
        }

        fn acknowledge(&mut self, nonce: u64) {
            if self.waiting && nonce == self.nonce {
                self.waiting = false;
                self.missed = 0;
            }
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
        /// It reported `Ready`.
        ready: bool,
        heartbeat: Heartbeat,
    }

    struct Supervisor {
        /// Pause or Quit invalidates an in-flight startup before its command is dequeued.
        startup_epoch: Arc<AtomicU64>,
        applied_epoch: u64,
        launch: Option<Launch>,
        /// A started engine must hook the keyboard (false for `--test-ipc` test engines).
        expects_hook: bool,
        /// The engine has no working session; told to the observer on every change.
        hook_unavailable: bool,
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
                self.probe_engine();
                // Wake for a pending restart, or for a started engine that hasn't said it's
                // ready.
                let ready_by = self
                    .running
                    .as_ref()
                    .filter(|r| !r.ready)
                    .map(|r| r.started + READY_TIMEOUT);
                let ping_at = self
                    .running
                    .as_ref()
                    .filter(|r| r.ready)
                    .map(|r| r.heartbeat.next);
                let wake = super::next_wake(super::next_wake(self.next_start, ready_by), ping_at);
                let cmd = match wake {
                    Some(at) => {
                        match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                            Ok(cmd) => cmd,
                            Err(RecvTimeoutError::Timeout) => {
                                if self.next_start.is_some_and(|t| t <= Instant::now()) {
                                    self.next_start = None;
                                }
                                if ready_by.is_some_and(|t| t <= Instant::now()) {
                                    diagnostics::record(Event::ReadyTimedOut);
                                    self.failed_start();
                                }
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => Cmd::Shutdown,
                        }
                    }
                    None => rx.recv().unwrap_or(Cmd::Shutdown),
                };
                match cmd {
                    Cmd::Apply(next, epoch) => {
                        self.applied_epoch = epoch;
                        let resumed = !self.settings.enabled && next.enabled;
                        self.settings = *next;
                        if resumed {
                            // The user asked for it: try now, whatever the backoff said.
                            self.failures = 0;
                            self.next_start = None;
                        }
                        if self.settings.enabled {
                            self.configure();
                        }
                    }
                    Cmd::Flags(flags) => {
                        self.flags = flags;
                        self.configure();
                    }
                    Cmd::Exited(generation) => self.on_exit(generation),
                    Cmd::Ready { generation, hooked } => self.on_ready(generation, hooked),
                    Cmd::Pong { generation, nonce } => {
                        if let Some(r) =
                            self.running.as_mut().filter(|r| r.generation == generation)
                        {
                            r.heartbeat.acknowledge(nonce);
                        }
                    }
                    Cmd::SampleMemory(reply) => {
                        let _ = reply.send(self.sample_memory());
                    }
                    Cmd::Shutdown => break,
                }
            }
            self.stop();
        }

        /// Start or stop the engine to match the settings.
        fn converge(&mut self) {
            if !self.settings.enabled {
                // Pause: no process, no hook, and nothing to warn about.
                self.stop();
                self.next_start = None;
                self.set_hook_unavailable(false);
                return;
            }
            if self.running.is_some()
                || self.next_start.is_some()
                || self.startup_cancellation().is_cancelled()
            {
                return;
            }
            self.set_hook_unavailable(true);
            match self.spawn() {
                Ok(running) => self.running = Some(running),
                Err(_) if self.startup_cancellation().is_cancelled() => {}
                Err(e) => {
                    diagnostics::record(Event::EngineStartupFailed(e.kind()));
                    self.failures = self.failures.saturating_add(1);
                    self.schedule_restart();
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
                diagnostics::record(Event::PipeWriteFailed(e.kind()));
                self.end_session();
            }
        }

        /// The pipe failed: end the engine without asking (asking needs the pipe) and restart
        /// it with backoff, like a crash.
        fn end_session(&mut self) {
            let Some(mut running) = self.running.take() else {
                return;
            };
            self.set_hook_unavailable(true);
            running.pipe.cancel_io();
            diagnostics::record(Event::EngineTerminated);
            let _ = running.child.kill();
            let _ = running.child.wait();
            if let Some(reader) = running.reader.take() {
                let _ = reader.join();
            }
            (self.observer)(Observed::EngineCrashed);
            self.failures = failures_after(self.failures, running.started.elapsed());
            self.schedule_restart();
        }

        fn sample_memory(&mut self) -> Option<u64> {
            let running = self.running.as_mut()?;
            if !running.ready
                || running.started.elapsed() < super::STABLE_RUN
                || !crate::system::input_idle()
                || running.child.try_wait().ok()?.is_some()
            {
                return None;
            }
            crate::system::pair_private_mb(&running.child)
        }

        fn probe_engine(&mut self) {
            let now = Instant::now();
            let Some(r) = self
                .running
                .as_mut()
                .filter(|r| r.ready && now >= r.heartbeat.next)
            else {
                return;
            };
            let answered = r.heartbeat.probe(now).is_some_and(|nonce| {
                frame::write(&mut &*r.pipe, &ToEngine::Ping { nonce }).is_ok()
            });
            if !answered {
                diagnostics::record(Event::HeartbeatFailed);
                self.end_session();
            }
        }

        fn schedule_restart(&mut self) {
            let delay = backoff(self.failures);
            self.next_start = Some(Instant::now() + delay);
            diagnostics::record(Event::RestartAfter(delay.as_secs()));
        }

        fn startup_cancellation(&self) -> Cancellation<'_> {
            Cancellation::new(&self.startup_epoch, self.applied_epoch)
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
            let mut child = command.spawn()?;
            diagnostics::record(Event::EngineSpawned);
            let job = KillOnClose::new()
                .and_then(|job| job.assign(&child).map(|()| job))
                .map_err(|e| eprintln!("autocorrect: engine job object: {e}"))
                .ok();
            let pipe = match session::serve_cancellable(
                listener,
                child.id(),
                &own_exe(),
                session::HANDSHAKE_TIMEOUT,
                self.startup_cancellation(),
            ) {
                Ok((pipe, _version)) => Arc::new(pipe),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e);
                }
            };
            if self.startup_cancellation().is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }
            self.generation += 1;
            let generation = self.generation;
            let settings = self.engine_settings();
            if let Err(e) = frame::write(
                &mut pipe.cancellable_writer(self.startup_cancellation()),
                &ToEngine::Configure { settings },
            ) {
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
                        read_engine(&pipe, &observer, &learned, &words, &|control| {
                            let cmd = match control {
                                Control::Ready(hooked) => Cmd::Ready { generation, hooked },
                                Control::Pong(nonce) => Cmd::Pong { generation, nonce },
                            };
                            let _ = tx.send(cmd);
                        });
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
                ready: false,
                heartbeat: Heartbeat::new(Instant::now()),
            })
        }

        /// The engine says it is ready. Without the keyboard hook that is a failed start.
        fn on_ready(&mut self, generation: u64, hooked: bool) {
            let Some(running) = self.running.as_mut().filter(|r| r.generation == generation) else {
                return;
            };
            running.ready = true;
            running.heartbeat = Heartbeat::new(Instant::now());
            if !super::hook_start_failed(self.expects_hook, hooked) {
                self.set_hook_unavailable(false);
                diagnostics::record(Event::EngineReady);
                return;
            }
            diagnostics::record(Event::HookStartFailed);
            self.failed_start();
        }

        /// A start that didn't give us a working hook: stop it, say so, and try again after
        /// the backoff (as for a crash).
        fn failed_start(&mut self) {
            let started = self.running.as_ref().map(|r| r.started);
            self.stop();
            (self.observer)(Observed::HookStartFailed);
            self.set_hook_unavailable(true);
            self.failures = failures_after(
                self.failures,
                started.map_or(Duration::ZERO, |s| s.elapsed()),
            );
            self.schedule_restart();
        }

        fn set_hook_unavailable(&mut self, unavailable: bool) {
            if self.hook_unavailable != unavailable {
                self.hook_unavailable = unavailable;
                (self.observer)(Observed::HookUnavailable {
                    active: unavailable,
                });
            }
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
            self.set_hook_unavailable(true);
            let status = wait_for(&mut running.child, STOP_TIMEOUT);
            if status.is_none() {
                let _ = running.child.kill();
                let _ = running.child.wait();
            }
            if let Some(reader) = running.reader.take() {
                let _ = reader.join();
            }
            let crashed = status.is_none_or(|s| !s.success());
            diagnostics::record(
                status
                    .and_then(|s| s.code())
                    .map_or(Event::EngineTerminated, Event::EngineExit),
            );
            if crashed {
                (self.observer)(Observed::EngineCrashed);
            }
            self.failures = failures_after(self.failures, running.started.elapsed());
            self.schedule_restart();
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
                diagnostics::record(Event::EngineTerminated);
                let _ = running.child.kill();
                let _ = running.child.wait();
            }
            running.pipe.cancel_io();
            if let Some(reader) = running.reader.take() {
                let _ = reader.join();
            }
        }
    }

    #[cfg(test)]
    #[test]
    fn heartbeat_restarts_after_two_misses_and_ignores_stale_pongs() {
        let now = Instant::now();
        let mut heartbeat = Heartbeat::new(now);
        assert_eq!(heartbeat.next, now + PING_EVERY);
        let first = heartbeat.probe(heartbeat.next).unwrap();
        heartbeat.probe(heartbeat.next).unwrap();
        heartbeat.acknowledge(first);
        assert_eq!(heartbeat.probe(heartbeat.next), None);

        let mut heartbeat = Heartbeat::new(now);
        heartbeat.probe(heartbeat.next).unwrap();
        let second = heartbeat.probe(heartbeat.next).unwrap();
        heartbeat.acknowledge(second);
        let third = heartbeat.probe(heartbeat.next).unwrap();
        assert_ne!(second, third);
        assert!(heartbeat.probe(heartbeat.next).is_some());
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

    enum Control {
        Ready(bool),
        Pong(u64),
    }

    /// Hand the engine's messages to the app until its pipe closes.
    fn read_engine(
        pipe: &Pipe,
        observer: &Observer,
        learned: &Learned,
        words: &AtomicU64,
        control: &dyn Fn(Control),
    ) {
        loop {
            let body = match session::read_frame(pipe, unflub_engine::protocol::FRAME_READ_TIMEOUT)
            {
                Ok(Some(body)) => body,
                Ok(None) => return,
                Err(e) => {
                    diagnostics::record(Event::PipeReadFailed(e.kind()));
                    return;
                }
            };
            let message = match frame::decode::<FromEngine>(&body) {
                Ok(m) => m,
                Err(_) => {
                    diagnostics::record(Event::InvalidEngineMessage);
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
                    trigger,
                    source,
                    category,
                } => observer(Observed::Corrected {
                    keep_word,
                    original,
                    corrected,
                    user_rule,
                    decide: Duration::from_micros(decide_us),
                    inject: Duration::from_micros(inject_us),
                    pill,
                    trigger,
                    source,
                    category,
                }),
                FromEngine::UndoArmed => observer(Observed::UndoArmed),
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
                FromEngine::HookReinstalled { reason } => {
                    observer(Observed::HookReinstalled { reason })
                }
                FromEngine::UsageCounts {
                    active_minutes,
                    skipped_secure,
                    skipped_excluded,
                    skipped_code,
                    decide_all_us,
                    spellcheck_us,
                } => observer(Observed::UsageCounts(crate::usage::EngineCounts {
                    active_minutes,
                    skipped_secure,
                    skipped_excluded,
                    skipped_code,
                    decide_all_us,
                    spellcheck_us,
                })),
                FromEngine::Diagnostic { event } => observer(Observed::Diagnostic(event)),
                FromEngine::CorrectorPanicked => observer(Observed::CorrectorPanicked),
                FromEngine::InputInterference { active } => {
                    observer(Observed::InputInterference { active })
                }
                FromEngine::Ready { hooked } => control(Control::Ready(hooked)),
                FromEngine::Pong { nonce } => control(Control::Pong(nonce)),
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

    /// Review finding 7: an engine without its hook is a failed start, and the supervisor
    /// wakes for whichever comes first, a restart or a readiness deadline.
    #[test]
    fn a_hookless_start_fails_and_readiness_has_a_deadline() {
        assert!(hook_start_failed(true, false));
        assert!(!hook_start_failed(true, true));
        assert!(
            !hook_start_failed(false, false),
            "--test-ipc engines never hook"
        );
        let now = std::time::Instant::now();
        let later = now + Duration::from_secs(5);
        assert_eq!(next_wake(None, None), None);
        assert_eq!(next_wake(Some(later), None), Some(later));
        assert_eq!(next_wake(None, Some(now)), Some(now));
        assert_eq!(next_wake(Some(later), Some(now)), Some(now));
        assert!(
            READY_TIMEOUT > Duration::from_secs(5),
            "room for a cold dictionary build"
        );
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
        let expect_availability = |active| {
            assert_eq!(
                rx.recv_timeout(wait),
                Ok(Observed::HookUnavailable { active })
            );
        };
        expect_availability(true); // Startup, before Ready.
        expect_availability(false); // Ready in hook-free test mode.
        expect_availability(true); // Pipe closed. The tray must show the failure.
        assert_eq!(rx.recv_timeout(wait), Ok(Observed::EngineCrashed));
        assert!(marker.exists(), "the panic left its marker");
        std::fs::remove_file(&marker).unwrap();

        let since = Instant::now();
        expect_availability(false); // The restarted engine reported Ready.
        expect_availability(true); // It crashed again.
        assert_eq!(rx.recv_timeout(wait), Ok(Observed::EngineCrashed));
        assert!(
            since.elapsed() >= Duration::from_millis(900),
            "restarted after the 1 s backoff, not at once ({:?})",
            since.elapsed()
        );

        // Pause drops the pending restart and clears the warning.
        host.apply(&Settings {
            enabled: false,
            ..Settings::default()
        });
        expect_availability(false);
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
