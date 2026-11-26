//! The running engine: hook, engine, spell checker and pill, driven by the app over the pipe.
//!
//! Threads: the main thread reads the pipe; `unflub-engine` owns the stack below (it joins the
//! COM MTA for the spell checker) and writes to the pipe; `unflub-events` forwards hook events
//! to it. Plus the adapter's own threads (`unflub-hook`, `unflub-inject`, `unflub-uia`,
//! `unflub-spell`, `unflub-overlay`, `unflub-caret`).
//!
//! - Engine settings reach the hook thread through `LiveEngine` at the next key. Excluded apps
//!   are in the core `Config`; the hook's own exclude list holds only the app's exe, so keys
//!   typed in unflub's own windows never reach the engine.
//! - A language change restarts the hook with a spell checker for the new language.
//! - The OS look (dark mode, accent) is read again before each pill.
//! - Pausing is the app stopping this process; there is no paused state here.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use unflub_core::{Engine, SkipCounts, Source, SpellBackend};
use unflub_engine::category::category_of;
use unflub_engine::look::OsLook;
use unflub_engine::pill::pill_config;
use unflub_engine::pipe::Pipe;
use unflub_engine::protocol::EngineDiagnostic;
use unflub_engine::protocol::{
    EngineSettings, FixSource, FromEngine, ReinstallReason, ToEngine, Trigger,
};
use unflub_engine::{frame, session};
use unflub_windows::stats::UsageCounters;
use unflub_windows::{
    Correction, Hook, HookEvent, HookOptions, LiveConfig, LiveEngine, Overlay, OverlayHandle,
    PillConfig, PillStyle, WindowsSpellBackend,
};

use crate::Args;

/// How often the word count and the usage counts go to the app (only when they moved).
const WORDS_EVERY: Duration = Duration::from_secs(10);

enum Msg {
    Configure(Box<EngineSettings>),
    Ping(u64),
    Hook(HookEvent),
    /// A word the user taught the engine by undoing its fix.
    Learned(String),
    Shutdown,
}

/// Writes messages to the app. One frame per call, under a lock (which holds whether the
/// session already failed).
#[derive(Clone)]
struct Outbox(Arc<Pipe>, Arc<Mutex<bool>>);

impl Outbox {
    fn send(&self, message: &FromEngine) {
        let mut failed = self.1.lock().unwrap_or_else(|e| e.into_inner());
        if *failed {
            return;
        }
        // Every write has a deadline (`pipe::WRITE_TIMEOUT`): an app that stops reading can't
        // stall this thread, and with it the hook's events and a Shutdown behind them. A failed
        // or timed-out write may have left half a frame, so the session is over: cancelling the
        // pipe's I/O ends the reader, which shuts the engine down (unhooking cleanly).
        if let Err(e) = frame::write(&mut &*self.0, message) {
            if e.kind() != io::ErrorKind::BrokenPipe {
                eprintln!("unflub-engine: can't reach the app ({e}); stopping");
            }
            *failed = true;
            self.0.cancel_io();
        }
    }
}

/// Exit code: 0 after a shutdown or the pipe closing, 3 if the handshake failed.
pub fn run(args: &Args) -> i32 {
    let (pipe, app_exe) = match session::join(&args.pipe, args.app_pid, session::HANDSHAKE_TIMEOUT)
    {
        Ok(joined) => joined,
        Err(e) => {
            eprintln!("unflub-engine: can't join the app ({e})");
            return 3;
        }
    };
    let pipe = Arc::new(pipe);
    let out = Outbox(pipe.clone(), Arc::default());
    let (tx, rx) = mpsc::channel();
    let state = State {
        settings: None,
        app_exe,
        dry_run: args.dry_run,
        no_hook: args.test_ipc,
        test_panic: args.test_panic,
        tx: tx.clone(),
        out,
        running: None,
        pill: None,
        test_engine: None,
        words_checked: Arc::default(),
        usage: Arc::default(),
        skips: Arc::default(),
    };
    let worker = std::thread::Builder::new()
        .name("unflub-engine".into())
        .spawn(move || state.run(rx));
    let worker = match worker {
        Ok(w) => w,
        Err(e) => {
            eprintln!("unflub-engine: can't start the engine thread ({e})");
            return 1;
        }
    };
    read_pipe(&pipe, &tx);
    let _ = tx.send(Msg::Shutdown);
    let _ = worker.join();
    0
}

/// Forward the app's messages until it says shutdown or the pipe closes.
fn read_pipe(pipe: &Pipe, tx: &Sender<Msg>) {
    loop {
        let body = match session::read_frame(pipe, unflub_engine::protocol::FRAME_READ_TIMEOUT) {
            Ok(Some(body)) => body,
            Ok(None) => return,
            Err(e) => {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("unflub-engine: pipe: {e}");
                }
                return;
            }
        };
        match frame::decode::<ToEngine>(&body) {
            Ok(ToEngine::Configure { settings }) => {
                let _ = tx.send(Msg::Configure(Box::new(settings)));
            }
            Ok(ToEngine::Ping { nonce }) => {
                let _ = tx.send(Msg::Ping(nonce));
            }
            Ok(ToEngine::Shutdown) => return,
            // Only valid during the handshake.
            Ok(ToEngine::Welcome { .. }) => {}
            Err(_) => eprintln!("unflub-engine: skipped an invalid app message"),
        }
    }
}

/// An installed hook and what talks to it.
struct Running {
    hook: Hook,
    config: LiveConfig,
    /// The language the user asked for (the checker may have fallen back to another).
    language: String,
}

struct State {
    /// `None` until the app's first `Configure`.
    settings: Option<EngineSettings>,
    app_exe: String,
    dry_run: bool,
    /// `--test-ipc`: never install a hook or show a pill.
    no_hook: bool,
    test_panic: bool,
    tx: Sender<Msg>,
    out: Outbox,
    running: Option<Running>,
    pill: Option<(OverlayHandle, PillConfig)>,
    /// `--test-ipc` builds the engine (so the index and settings are exercised) without a hook.
    test_engine: Option<Engine>,
    words_checked: Arc<AtomicU64>,
    /// What the hook and the spell checker count for the usage stats.
    usage: Arc<UsageCounters>,
    /// What the engine counts as skipped words for the usage stats.
    skips: Arc<SkipCounts>,
}

impl State {
    fn run(mut self, rx: Receiver<Msg>) {
        let mut sent_at = Instant::now();
        loop {
            let wait = WORDS_EVERY.saturating_sub(sent_at.elapsed());
            match rx.recv_timeout(wait) {
                Ok(Msg::Configure(next)) => {
                    self.settings = Some(*next);
                    self.sync();
                }
                Ok(Msg::Ping(nonce)) => self.out.send(&FromEngine::Pong { nonce }),
                Ok(Msg::Hook(event)) => self.on_hook_event(event),
                // Only unflub's own settings learn the word; the Windows dictionary, which every
                // app shares, is never written.
                Ok(Msg::Learned(word)) => self.out.send(&FromEngine::Learned { word }),
                Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            // On time even while fixes keep the channel busy.
            if sent_at.elapsed() >= WORDS_EVERY {
                self.send_counts();
                sent_at = Instant::now();
            }
        }
        self.stop();
        self.send_counts();
    }

    /// The word count and the usage counts, when they moved.
    fn send_counts(&self) {
        let count = self.words_checked.swap(0, Ordering::Relaxed);
        if count > 0 {
            self.out.send(&FromEngine::WordsChecked { count });
        }
        let taken = self.usage.take();
        let skipped_excluded = self.skips.excluded_app.swap(0, Ordering::Relaxed);
        let skipped_code = self.skips.code_context.swap(0, Ordering::Relaxed);
        let moved = taken.active_minutes > 0
            || taken.skipped_secure > 0
            || skipped_excluded > 0
            || skipped_code > 0
            || !taken.decide_all_us.is_empty()
            || !taken.spellcheck_us.is_empty();
        if moved {
            self.out.send(&FromEngine::UsageCounts {
                active_minutes: taken.active_minutes,
                skipped_secure: taken.skipped_secure,
                skipped_excluded,
                skipped_code,
                decide_all_us: taken.decide_all_us,
                spellcheck_us: taken.spellcheck_us,
            });
        }
    }

    /// Make the running stack match the settings.
    fn sync(&mut self) {
        let Some(s) = &self.settings else {
            return;
        };
        if self.no_hook {
            let first = self.test_engine.is_none();
            let engine = self
                .test_engine
                .get_or_insert_with(|| Engine::new(s.core.clone(), None));
            engine.set_config(s.core.clone());
            if first {
                self.out.send(&FromEngine::Ready { hooked: false });
                if self.test_panic {
                    panic!("test panic; the user typed: hunter2");
                }
            }
            return;
        }
        let restart = self
            .running
            .as_ref()
            .is_some_and(|r| !s.core.enabled || r.language != s.core.language);
        if restart {
            self.stop();
        }
        let s = self.settings.as_ref().expect("checked above");
        if let Some(r) = &self.running {
            r.config.set(s.core.clone());
        } else if s.core.enabled {
            self.running = start(
                s,
                &self.app_exe,
                self.dry_run,
                &self.tx,
                Counters {
                    words_checked: &self.words_checked,
                    usage: &self.usage,
                    skips: &self.skips,
                },
            );
            let hooked = self.running.is_some();
            if self.dry_run {
                // The perf scripts wait for one of these lines.
                println!(
                    "{}",
                    if hooked {
                        "hooked."
                    } else {
                        "hook unavailable"
                    }
                );
            }
            self.out.send(&FromEngine::Ready { hooked });
        }
        self.sync_pill();
    }

    /// Spawn, update or close the pill to match the settings and the OS look right now.
    fn sync_pill(&mut self) {
        let Some(s) = &self.settings else {
            return;
        };
        let want = pill_config(&s.pill, &OsLook::read());
        if self.running.is_none() || want.style == PillStyle::Off {
            if let Some((overlay, _)) = self.pill.take() {
                overlay.shutdown();
            }
            return;
        }
        match &mut self.pill {
            Some((overlay, current)) => {
                if *current != want {
                    overlay.set_config(want.clone());
                    *current = want;
                }
            }
            None => match Overlay::spawn(want.clone()) {
                Ok(overlay) => self.pill = Some((overlay, want)),
                Err(_) => self.out.send(&FromEngine::Diagnostic {
                    event: EngineDiagnostic::PillUnavailable,
                }),
            },
        }
    }

    fn on_hook_event(&mut self, event: HookEvent) {
        if let Some(correction) = Correction::from_hook_event(&event) {
            // The OS may have switched dark mode or accent since the last pill.
            self.sync_pill();
            let pill = self.pill.is_some();
            if let Some((overlay, _)) = &self.pill {
                overlay.show(correction);
            }
            match event {
                HookEvent::Corrected {
                    replacement,
                    timing,
                    app,
                    ..
                } => self.out.send(&FromEngine::Corrected {
                    keep_word: unflub_windows::policy::keep_words(app.as_deref()),
                    user_rule: replacement.source == Source::UserRule,
                    // The app's name stays here; only its kind goes to the app.
                    category: category_of(app.as_deref()),
                    trigger: Trigger::of(replacement.insert.chars().last().unwrap_or(' ')),
                    source: FixSource::from(replacement.source),
                    original: replacement.original,
                    corrected: replacement.corrected,
                    decide_us: timing.decide.as_micros() as u64,
                    inject_us: timing.inject.as_micros() as u64,
                    pill,
                }),
                HookEvent::UndoPending { .. } => self.out.send(&FromEngine::UndoArmed),
                HookEvent::Undone {
                    replacement, app, ..
                } => self.out.send(&FromEngine::Undone {
                    keep_word: unflub_windows::policy::keep_words(app.as_deref()),
                    user_rule: replacement.source == Source::UserRule,
                    original: replacement.original,
                    corrected: replacement.corrected,
                }),
                _ => {}
            }
            return;
        }
        match event {
            HookEvent::Warning(w) => {
                // Select a fixed category. The warning text never crosses the pipe.
                let event = if w.starts_with("UI Automation")
                    || w.starts_with("COM init for UI Automation")
                    || w.starts_with("can't start the UI Automation thread")
                {
                    EngineDiagnostic::UiaWarning
                } else {
                    EngineDiagnostic::HookWarning
                };
                self.out.send(&FromEngine::Diagnostic { event });
            }
            HookEvent::HooksReinstalled { reason } => {
                eprintln!("unflub-engine: hooks installed again ({reason})");
                self.out.send(&FromEngine::HookReinstalled {
                    reason: ReinstallReason::parse(reason),
                });
            }
            HookEvent::KeyboardHookLost { error } => {
                // The app restarts an engine that reports no hook (a failed start), which
                // installs everything afresh and shows it in the tray meanwhile.
                eprintln!("unflub-engine: the keyboard hook is gone ({error})");
                self.out.send(&FromEngine::Ready { hooked: false });
            }
            HookEvent::CorrectorPanicked(_) => {
                // The message may hold typed text; it stays here.
                eprintln!("unflub-engine: the engine panicked; keys pass through");
                self.out.send(&FromEngine::CorrectorPanicked);
            }
            HookEvent::InjectFailed { sent, total, .. } => self.out.send(&FromEngine::Diagnostic {
                event: EngineDiagnostic::InjectionPartial { sent, total },
            }),
            HookEvent::UndoDeclined => {
                if let Some((overlay, _)) = &self.pill {
                    overlay.hide();
                }
            }
            HookEvent::InputInterference { active } => {
                self.out.send(&FromEngine::InputInterference { active })
            }
            _ => {}
        }
    }

    /// Unhook and close the pill.
    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.hook.stop();
        }
        if let Some((overlay, _)) = self.pill.take() {
            overlay.shutdown();
        }
    }
}

/// Where a started stack counts.
struct Counters<'a> {
    words_checked: &'a Arc<AtomicU64>,
    usage: &'a Arc<UsageCounters>,
    skips: &'a Arc<SkipCounts>,
}

/// Build the spell checker and engine, and install the hook. `None` if the hook can't be
/// installed.
fn start(
    s: &EngineSettings,
    app_exe: &str,
    dry_run: bool,
    tx: &Sender<Msg>,
    counters: Counters<'_>,
) -> Option<Running> {
    let backend = spell_backend(&s.core.language).map(|mut b| {
        b.time_checks_into(counters.usage.clone());
        b
    });
    let mut engine = Engine::new(
        s.core.clone(),
        backend.map(|b| Box::new(b) as Box<dyn SpellBackend>),
    );
    engine.count_words_into(counters.words_checked.clone());
    engine.count_skips_into(counters.skips.clone());
    let learned = tx.clone();
    let (corrector, config) = LiveEngine::new(engine, move |word| {
        let _ = learned.send(Msg::Learned(word));
    });
    // Excluded apps are in the engine's Config, which changes live. The hook's own list holds
    // only unflub itself: words typed in Settings (your dictionary, app names) are never
    // "fixed", and the tour's practice box has its own engine in the app.
    let options = HookOptions {
        excluded_apps: vec![app_exe.to_lowercase()],
        dry_run,
        usage: Some(counters.usage.clone()),
    };
    let (hook, events) = match Hook::start(corrector, options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("unflub-engine: can't install the keyboard hook: {e}");
            return None;
        }
    };
    // The hook's channel ends when the hook stops, which ends this thread.
    let forward = tx.clone();
    let forwarder = std::thread::Builder::new()
        .name("unflub-events".into())
        .spawn(move || {
            for event in events {
                if forward.send(Msg::Hook(event)).is_err() {
                    break;
                }
            }
        });
    if let Err(e) = forwarder {
        // Nobody would read the hook's events; don't run without them.
        eprintln!("unflub-engine: can't start the event thread: {e}");
        hook.stop();
        return None;
    }
    Some(Running {
        hook,
        config,
        language: s.core.language.clone(),
    })
}

/// The Windows spell checker for `language`, or for the user's UI language if Windows has no
/// dictionary for it.
fn spell_backend(language: &str) -> Option<WindowsSpellBackend> {
    WindowsSpellBackend::new(Some(language))
        .or_else(|e| {
            eprintln!("unflub-engine: no {language} spell checker ({e}); using the default");
            WindowsSpellBackend::new(None)
        })
        .map_err(|e| eprintln!("unflub-engine: Windows spell checker unavailable ({e})"))
        .ok()
}
