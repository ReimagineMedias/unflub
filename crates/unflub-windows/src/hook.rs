//! The hook thread: low-level keyboard and mouse hooks, focus WinEvents, and injection.
//!
//! Everything here runs on one dedicated thread with its own message loop, so the keyboard
//! callback, the mouse callback and the focus callback never run at the same time and share
//! state through a thread-local without locks. The thread joins the COM MTA (it never creates
//! windows or an STA), so a [`WindowsSpellBackend`](crate::spell::WindowsSpellBackend) used by
//! the corrector can be called directly. UI Automation runs on a separate worker thread
//! ([`PasswordProbe`]) because it can block on hung apps.
//!
//! Keys typed while the field is secure (password, or not yet known to be safe), under an IME
//! layout, or in an excluded app never reach the corrector: it gets one `Key::Reset` instead.
//! Letters typed while the field's check is still pending are kept by the hook and handed over
//! only if UIA clears the field before the word ends ([`PendingWord`]).
//!
//! Windows removes a low-level hook whose callback is too slow (`LowLevelHooksTimeout`, at most
//! 1 s on Windows 10+), and every millisecond here is input lag for the whole desktop, so the
//! callback does no I/O: corrections are reported through a channel and printed elsewhere.
//! It doesn't call `SendInput` either: injected events pass through every low-level hook on
//! the system, ours included, so a correction is handed to the `unflub-inject` thread and the
//! user's next keys are held back until it has landed (see [`inject::Pipeline`]).
//!
//! Windows can also drop a low-level hook without telling us (a callback that timed out, or
//! sometimes across sleep and a locked session). A watchdog compares `GetLastInputInfo` with the
//! time of our last callback every few seconds: input the system saw but our hooks didn't means
//! they are gone, so they are installed again (backing off while that keeps happening, until a
//! real callback arrives after the reinstall). They are also installed again once after each
//! resume from sleep and when the session is unlocked (a message-only window receives those
//! notifications). Each time is reported as [`HookEvent::HooksReinstalled`] with its reason, or
//! as [`HookEvent::KeyboardHookLost`] when the keyboard hook couldn't be installed again; then
//! every watchdog tick tries again.
//!
//! Limitation: a non-elevated process doesn't see keys typed into elevated (administrator)
//! windows, and can't inject into them (UIPI). unflub simply does nothing there.

use std::cell::{Cell, RefCell};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use unflub_core::{Action, Adjacency, Context, Key, Replacement, WordEnds};
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, GetLastInputInfo, MapVirtualKeyExW,
    ToUnicodeEx, VkKeyScanExW, HKL, LASTINPUTINFO, MAPVK_VK_TO_VSC, VK_CAPITAL, VK_CONTROL,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowThreadProcessId, IsWindow, KillTimer, PeekMessageW, PostThreadMessageW,
    RegisterClassW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, DEVICE_NOTIFY_WINDOW_HANDLE,
    EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND, HC_ACTION, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, LLKHF_INJECTED, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
    PBT_APMSUSPEND, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_MBUTTONDOWN, WM_POWERBROADCAST, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_TIMER,
    WM_WTSSESSION_CHANGE, WM_XBUTTONDOWN, WNDCLASSW, WTS_SESSION_UNLOCK,
};

use crate::focus::{self, FieldCheck, FocusState, PasswordProbe};
use crate::inject::{self, Finished, Input, Origin, Pipeline, RawEvent, SendOutcome, Tag};
use crate::policy::{
    injection_veto, is_ime_language, veto_modifiers, Gate, GateDecision, PendingWord,
};
use crate::pump::{PumpAck, PumpHealth, PumpProbe};
use crate::stats::{LatencyHistogram, MinuteMarks, UsageCounters};
use crate::translate::{KeyTranslator, Modifiers, RawKey, Typed};
use crate::Corrector;

/// `ToUnicodeEx` flag: don't change keyboard state (Windows 10 1607+). Without it, translating
/// a key in our hook would eat the dead-key state the user's app needs for accents.
const TOUNICODE_NO_STATE_CHANGE: u32 = 0x4;

const HOOK_THREAD_NAME: &str = "unflub-hook";

/// After a click, Tab or Enter, wait this long before asking UIA about the focused field, so
/// the app has handled the input that may have moved focus. The field is secure meanwhile.
const INPUT_RECHECK_DELAY: Duration = Duration::from_millis(50);

/// Posted by the injector thread: results are waiting in the channel.
const WM_APP_SENT: u32 = WM_APP + 1;

/// If an injection or replay hasn't come back after this long, give up on it: replay the held
/// keys once and stop holding or correcting until a probe comes back (see [`inject::Pipeline`]).
const STALE_AFTER: Duration = Duration::from_millis(500);
const STALE_CHECK_MS: u32 = 100;
const FIRST_DEGRADE_BACKOFF: Duration = Duration::from_secs(30);
const MAX_DEGRADE_BACKOFF: Duration = Duration::from_secs(30 * 60);
const DEGRADE_HEALTHY_RESET: Duration = Duration::from_secs(60);

struct DegradeBackoff {
    next: Duration,
    recovered_at: Option<Instant>,
}

impl Default for DegradeBackoff {
    fn default() -> Self {
        Self {
            next: FIRST_DEGRADE_BACKOFF,
            recovered_at: None,
        }
    }
}

impl DegradeBackoff {
    fn stalled(&mut self, now: Instant) -> Duration {
        if self
            .recovered_at
            .take()
            .is_some_and(|at| now.duration_since(at) >= DEGRADE_HEALTHY_RESET)
        {
            self.next = FIRST_DEGRADE_BACKOFF;
        }
        let wait = self.next;
        self.next = (wait * 2).min(MAX_DEGRADE_BACKOFF);
        wait
    }
}

/// On stop, how long to wait for the injector to finish what it was handed before the held-back
/// keys go to the app.
const STOP_DRAIN: Duration = Duration::from_secs(1);

/// The first Backspace held for a double-Backspace undo, also kept here (0: none) so the
/// engine's panic hook can still send it ([`send_held_backspace_now`]). Everything else about it
/// lives in the hook thread's state.
static HELD_FOR_CRASH: AtomicU64 = AtomicU64::new(0);

fn encode_held(ev: RawEvent) -> u64 {
    1 << 63
        | u64::from(ev.vk)
        | u64::from(ev.scan) << 16
        | u64::from(ev.extended) << 32
        | u64::from(ev.foreign) << 33
}

fn decode_held(bits: u64) -> Option<RawEvent> {
    (bits >> 63 == 1).then_some(RawEvent {
        vk: bits as u16,
        scan: (bits >> 16) as u16,
        extended: bits >> 32 & 1 == 1,
        up: false,
        foreign: bits >> 33 & 1 == 1,
    })
}

/// For a panic hook that is about to end the process: send a held first Backspace on (down and
/// up) so the user's keypress isn't lost with the process. Our own hook, if it is still
/// running, lets it through untouched. Returns true if there was one.
///
/// What can't be saved: a process ended from outside (the app killing a stuck engine, or the
/// Job Object when the app itself dies) runs no code, so a Backspace held at that moment is
/// lost. The hold only ever lasts until the user's next key or click.
#[cfg(windows)]
pub fn send_held_backspace_now() -> bool {
    let Some(held) = decode_held(HELD_FOR_CRASH.swap(0, Ordering::AcqRel)) else {
        return false;
    };
    let keys = [held, RawEvent { up: true, ..held }];
    let _ = inject::send(&inject::pass_inputs(&keys), |sent| {
        inject::pass_inputs(&inject::unwind_keys(&keys, sent))
    });
    true
}

/// Posted by the session window: the computer resumed or the session was unlocked (`wParam`:
/// [`REARM_RESUME`], [`REARM_UNLOCK`]), or is about to sleep ([`REARM_SUSPEND`]).
const WM_APP_REARM: u32 = WM_APP + 2;
const REARM_RESUME: usize = 0;
const REARM_UNLOCK: usize = 1;
const REARM_SUSPEND: usize = 2;
/// How often the watchdog compares system input with our callbacks.
const WATCHDOG_MS: u32 = 5_000;

#[derive(Debug, Clone, Default)]
pub struct HookOptions {
    /// Lowercased exe names where keys are not fed to the corrector at all.
    pub excluded_apps: Vec<String>,
    /// Run everything (hooks, focus checks, the corrector) but never inject input or swallow a
    /// key: every action is reported as [`HookEvent::Skipped`] with reason
    /// [`DRY_RUN`](crate::policy::DRY_RUN) and the corrector is reset, as for any vetoed action.
    /// For measuring the hook on a machine someone is typing on.
    pub dry_run: bool,
    /// Where the hook counts typing minutes, words skipped in secure fields and the time of
    /// every decision on a word-ending key, for the app's anonymous usage stats.
    pub usage: Option<Arc<UsageCounters>>,
}

/// Reported from the hook thread; read it on another thread (printing is too slow for the hook).
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// A `Replace` landed. `took` is the hook callback for the triggering key.
    Corrected {
        replacement: Replacement,
        app: Option<String>,
        took: Duration,
        timing: InjectTiming,
    },
    /// A first Backspace after `replacement` is held (double-Backspace undo): the pill asks
    /// "Undo?". Followed by `Undone` (second Backspace) or `UndoDeclined`.
    UndoPending { replacement: Replacement },
    /// The held Backspace was sent on (another key, or the undo window ran out) or dropped (a
    /// click or focus change): the pill hides.
    UndoDeclined,
    /// An `Undo` landed.
    Undone {
        replacement: Replacement,
        app: Option<String>,
        took: Duration,
        timing: InjectTiming,
    },
    /// The corrector asked for an action we didn't apply; the key went through untouched and
    /// the corrector was sent `Key::Reset`.
    Skipped {
        replacement: Replacement,
        reason: &'static str,
    },
    /// `SendInput` accepted only `sent` of `total` events (0: blocked, e.g. by UIPI). The
    /// user's key went through and the corrector was reset. `released` says a key left down
    /// by the partial send was released.
    InjectFailed {
        replacement: Replacement,
        sent: usize,
        total: usize,
        released: bool,
    },
    /// The corrector panicked (with this message). It is not called again; keys pass through.
    ///
    /// Only where panics unwind (debug builds and tests). Release builds use `panic = "abort"`,
    /// so a corrector panic ends the process: `unflub-engine` writes its scrubbed crash marker
    /// in its panic hook and the app restarts it.
    CorrectorPanicked(String),
    /// Something degraded but the hook keeps running (e.g. UI Automation unavailable).
    Warning(String),
    /// The keyboard and mouse hooks were installed again: the watchdog saw input they missed
    /// ([`REINSTALL_WATCHDOG`]), or the computer resumed or the session was unlocked
    /// ([`REINSTALL_RESUME`], [`REINSTALL_UNLOCK`]).
    HooksReinstalled { reason: &'static str },
    /// Installing the keyboard hook again failed (`error`): nothing is seen or corrected until
    /// a later attempt works. The hook thread tries again at every watchdog tick and reports
    /// [`HookEvent::HooksReinstalled`] when it does; an app supervising the hook should treat
    /// this like a failed start.
    KeyboardHookLost { error: String },
    /// `active`: our injected input stopped coming back through our hook (another program is
    /// interfering with keyboard input, or Windows dropped the hook mid-correction). Keys now
    /// pass straight through and nothing is corrected. `!active`: a probe came back; back to
    /// normal. See [`inject::Pipeline`].
    InputInterference { active: bool },
    /// Input went to an elevated (administrator) window, which our hooks can't see or fix
    /// (#132). Reported at most once per hook, by the watchdog, which notices the input it
    /// missed.
    ElevatedWindow,
}

/// [`HookEvent::HooksReinstalled`] reasons.
pub const REINSTALL_WATCHDOG: &str = "watchdog";
pub const REINSTALL_RESUME: &str = "resume";
pub const REINSTALL_UNLOCK: &str = "unlock";

/// Where the time of one correction went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InjectTiming {
    /// The corrector call that decided on the fix.
    pub decide: Duration,
    /// From handing the batch to the injector thread until `SendInput` returned, or until the
    /// last injected event came back through our hook when that happened first (it usually
    /// does: `SendInput` runs our hook for each event before it returns).
    pub inject: Duration,
    /// From swallowing the key until the last injected event came back through our hook.
    pub end_to_end: Duration,
}

/// What the hook thread hands back when it stops.
#[derive(Debug, Default)]
pub struct HookReport {
    /// Every keyboard callback for a user key (our injected events excluded).
    pub keyboard: LatencyHistogram,
    /// Corrector calls that led to a fix or undo.
    pub decide: LatencyHistogram,
    /// Hand-off to `SendInput` return, per fix or undo.
    pub inject: LatencyHistogram,
    /// Swallowed key to last injected event seen, per fix or undo.
    pub end_to_end: LatencyHistogram,
}

/// How long the hook thread's message pump may take to answer a liveness probe (#184).
const PUMP_DEADLINE: Duration = Duration::from_secs(5);

/// The running hook thread. Stops (and unhooks) on [`Hook::stop`] or drop.
pub struct Hook {
    thread_id: u32,
    thread: Option<JoinHandle<HookReport>>,
    pump: PumpProbe,
}

impl Hook {
    /// Start the hook thread and install the hooks. Returns once they are installed, or the
    /// error if installing failed.
    pub fn start<C: Corrector + Send + 'static>(
        corrector: C,
        options: HookOptions,
    ) -> std::io::Result<(Hook, Receiver<HookEvent>)> {
        let (events_tx, events_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let ack = Arc::new(PumpAck::default());
        let pump_ack = ack.clone();
        let thread = std::thread::Builder::new()
            .name(HOOK_THREAD_NAME.into())
            .spawn(move || run(Box::new(corrector), options, events_tx, ready_tx, pump_ack))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok((
                Hook {
                    thread_id,
                    thread: Some(thread),
                    pump: PumpProbe::new(thread_id, ack, PUMP_DEADLINE),
                },
                events_rx,
            )),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(std::io::Error::other(e))
            }
            Err(_) => {
                let _ = thread.join();
                Err(std::io::Error::other("hook thread exited during startup"))
            }
        }
    }

    /// Is the hook thread's message pump still turning? Never blocks: posts a probe when the
    /// last one was answered, and reports [`PumpHealth::Unresponsive`] once one has gone
    /// unanswered for a few seconds (see [`crate::pump`]). Call it now and then, such as on a
    /// heartbeat.
    pub fn check_pump(&mut self) -> PumpHealth {
        self.pump.check(Instant::now())
    }

    /// Unhook, end the thread and return its timing report.
    pub fn stop(mut self) -> HookReport {
        self.shutdown()
    }

    fn shutdown(&mut self) -> HookReport {
        let Some(thread) = self.thread.take() else {
            return HookReport::default();
        };
        // SAFETY: posting to a thread we own; it has a message queue (created at startup).
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        thread.join().unwrap_or_default()
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// A correction or undo on its way out.
struct Batch {
    batch: u16,
    replacement: Replacement,
    undo: bool,
    app: Option<String>,
    took: Duration,
    swallowed_at: Instant,
    /// When the batch went to the injector thread.
    handed_over: Instant,
    timing: InjectTiming,
    /// The injector's own `SendInput` time has arrived (`timing.inject`).
    inject_reported: bool,
    released: bool,
}

impl Batch {
    /// Fill in the times of a batch that is over at `now`.
    ///
    /// `SendInput` runs every low-level hook on the system, ours included, for each event
    /// before it returns, so the last echo normally finishes the batch while `SendInput` is
    /// still running and the injector's report comes after the batch is gone (#139: every
    /// inject time was 0). The time from the hand-over to that last echo stands in for it then;
    /// `SendInput` returns microseconds later.
    fn finish_timing(&mut self, now: Instant) {
        self.timing.end_to_end = now.saturating_duration_since(self.swallowed_at);
        if !self.inject_reported {
            self.timing.inject = now.saturating_duration_since(self.handed_over);
        }
    }
}

enum JobKind {
    Inject,
    Replay,
    /// One harmless tagged key-up to see whether our injections reach our hook again.
    Probe,
}

/// Work for the injector thread.
struct Job {
    kind: JobKind,
    batch: u16,
    inputs: Vec<windows::Win32::UI::Input::KeyboardAndMouse::INPUT>,
    handed_over: Instant,
    /// How to put keys back if `SendInput` takes only part of `inputs`.
    unwind: Unwind,
}

/// What a job's events were, for putting keys back after a partial send.
enum Unwind {
    /// A correction or undo, sent while the user held these Shift keys.
    Injection {
        events: Vec<inject::KeyEvent>,
        held: inject::ShiftKeys,
    },
    /// The user's own keys, sent again.
    Keys(Vec<RawEvent>),
    /// A probe: one key-up, nothing to put back.
    Nothing,
}

impl Unwind {
    fn inputs(&self, sent: usize) -> Vec<windows::Win32::UI::Input::KeyboardAndMouse::INPUT> {
        match self {
            Unwind::Injection { events, held } => {
                inject::unwind_inputs(&inject::unwind_injection(events, sent, *held))
            }
            Unwind::Keys(events) => inject::pass_inputs(&inject::unwind_keys(events, sent)),
            Unwind::Nothing => Vec::new(),
        }
    }
}

// SAFETY: INPUT is plain data (no pointers); the vector is moved to the injector thread.
unsafe impl Send for Job {}

struct JobDone {
    kind: JobKind,
    batch: u16,
    sent: usize,
    total: usize,
    released: bool,
    took: Duration,
}

/// The `unflub-inject` thread: runs `SendInput` for the hook thread, in hand-over order, and
/// posts `WM_APP_SENT` back. Ends when the hook thread drops its sender.
fn injector(jobs: Receiver<Job>, done: Sender<JobDone>, hook_thread: u32) {
    while let Ok(job) = jobs.recv() {
        let total = job.inputs.len();
        let (sent, released) = match inject::send(&job.inputs, |sent| job.unwind.inputs(sent)) {
            SendOutcome::Complete => (total, false),
            SendOutcome::Partial { sent, released, .. } => (sent, released),
        };
        let result = JobDone {
            kind: job.kind,
            batch: job.batch,
            sent,
            total,
            released,
            took: job.handed_over.elapsed(),
        };
        if done.send(result).is_err() {
            return;
        }
        // SAFETY: posting to the hook thread, which has a message queue.
        let _ = unsafe { PostThreadMessageW(hook_thread, WM_APP_SENT, WPARAM(0), LPARAM(0)) };
    }
}

struct State {
    corrector: Box<dyn Corrector>,
    translator: KeyTranslator,
    rank_layout: Option<HKL>,
    focus: FocusState,
    probe: Option<PasswordProbe>,
    excluded: Vec<String>,
    dry_run: bool,
    events: Sender<HookEvent>,
    report: HookReport,
    gate: Gate,
    pipeline: Pipeline,
    jobs: Sender<Job>,
    done: Receiver<JobDone>,
    batches: Vec<Batch>,
    /// Thread timer id while the pipeline is busy (0 when none).
    timer: usize,
    broken: bool,
    /// A word-start question in flight: `(focus generation, the corrector's reset generation
    /// when it was asked)`. See [`crate::focus::WordStart`].
    word_start: Option<(u64, u64)>,
    /// A first Backspace held for a double-Backspace undo ([`Action::HoldBackspace`]).
    held: Option<RawEvent>,
    /// When the last fix stops being undoable by a double Backspace.
    undo_deadline: Option<Instant>,
    /// While degraded: no probe before this time.
    probe_after: Option<Instant>,
    degrade_backoff: DegradeBackoff,
    next_probe: u16,
    usage: Option<Arc<UsageCounters>>,
    /// Which keys end a word, for counting words skipped in secure fields.
    word_ends: WordEnds,
    /// Letters typed while the field's UIA check is pending (#110).
    pending_word: PendingWord,
    minutes: MinuteMarks,
}

thread_local! {
    /// `GetTickCount` at our last keyboard or mouse callback (or (re)install), for the watchdog.
    static LAST_CALLBACK: Cell<u32> = const { Cell::new(0) };
    /// A real keyboard or mouse callback arrived since the hooks were last (re)installed.
    static CALLBACK_SEEN: Cell<bool> = const { Cell::new(false) };
    /// Input up to this tick happened where our hooks can't see it (see
    /// [`input_hidden_from_hooks`]), so the watchdog doesn't count it as missed.
    static EXCUSED: Cell<Option<u32>> = const { Cell::new(None) };
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Set when an event arrived while the state was busy (re-entrancy); the next event sends
    /// a Reset first so the corrector never acts on a word it only partly saw.
    static MISSED: Cell<bool> = const { Cell::new(false) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    let result = STATE
        .try_with(|cell| {
            let mut guard = cell.try_borrow_mut().ok()?;
            guard.as_mut().map(f)
        })
        .ok()
        .flatten();
    if result.is_none() {
        let _ = MISSED.try_with(|m| m.set(true));
    }
    result
}

fn run(
    corrector: Box<dyn Corrector>,
    options: HookOptions,
    events: Sender<HookEvent>,
    ready: mpsc::SyncSender<Result<u32, String>>,
    pump_ack: Arc<PumpAck>,
) -> HookReport {
    if let Err(e) = crate::spell::ensure_mta() {
        // A WindowsSpellBackend in the corrector still works: `new` put its own thread in the
        // MTA for good, so this thread is in the process's implicit MTA (see spell.rs).
        let _ = events.send(HookEvent::Warning(format!(
            "COM MTA init failed on the hook thread ({e}); relying on the process MTA"
        )));
    }
    let mut msg = MSG::default();
    // SAFETY: creates this thread's message queue so PostThreadMessageW can reach it.
    unsafe {
        let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
    }
    // SAFETY: trivial query.
    let thread_id = unsafe { GetCurrentThreadId() };

    let (jobs, job_rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    if let Err(e) = std::thread::Builder::new()
        .name("unflub-inject".into())
        .spawn(move || injector(job_rx, done_tx, thread_id))
    {
        let _ = ready.send(Err(format!("can't start the injector thread: {e}")));
        return HookReport::default();
    }

    let mut focus = FocusState::default();
    let probe = match PasswordProbe::start(focus.slot.clone()) {
        Ok((p, warnings)) => {
            for w in warnings {
                let _ = events.send(HookEvent::Warning(w));
            }
            focus.uia_available = true;
            Some(p)
        }
        Err(e) => {
            let _ = events.send(HookEvent::Warning(format!(
                "{e}; every field except classic edits counts as secure (no corrections there)"
            )));
            None
        }
    };
    let mut state = State {
        corrector,
        translator: KeyTranslator::new(),
        rank_layout: None,
        focus,
        probe,
        excluded: options
            .excluded_apps
            .iter()
            .map(|e| e.to_lowercase())
            .collect(),
        dry_run: options.dry_run,
        events,
        report: HookReport::default(),
        gate: Gate::default(),
        pipeline: Pipeline::new(),
        jobs,
        done,
        batches: Vec::new(),
        timer: 0,
        broken: false,
        word_start: None,
        held: None,
        undo_deadline: None,
        probe_after: None,
        degrade_backoff: DegradeBackoff::default(),
        next_probe: 0,
        usage: options.usage.clone(),
        word_ends: WordEnds::default(),
        pending_word: PendingWord::default(),
        minutes: MinuteMarks::default(),
    };
    state.refresh_focus(None, Duration::ZERO);
    STATE.with(|s| *s.borrow_mut() = Some(state));

    // SAFETY: hooks are installed and removed on this thread; callbacks are valid for 'static.
    let installed = unsafe { install() };
    let hooks = match installed {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return take_report();
        }
    };
    let _ = ready.send(Ok(thread_id));
    let mut hooks = hooks;
    // SAFETY: a thread timer and a message-only window on this thread, torn down below.
    let (watch_timer, session) = unsafe {
        (
            SetTimer(HWND::default(), 0, WATCHDOG_MS, None),
            SessionWatch::create(),
        )
    };
    let mut watchdog = Watchdog::new(tick_now());
    let mut resumes = ResumeGate::default();
    let mut told_elevated = false;

    // SAFETY: standard message loop; hook callbacks are delivered inside GetMessageW. Thread
    // messages (no window) are ours: injector results, the stale-batch and watchdog timers,
    // and re-arm requests from the session window.
    unsafe {
        while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
            if crate::pump::answer(&msg, &pump_ack) {
                // A liveness probe (#184): reaching it shows the pump turns.
                continue;
            }
            match msg.message {
                WM_APP_SENT if msg.hwnd.is_invalid() => {
                    with_state(State::on_results);
                }
                WM_TIMER if msg.hwnd.is_invalid() && msg.wParam.0 == watch_timer => {
                    with_state(|s| s.maybe_probe(false));
                    let now = tick_now();
                    if hooks.keyboard.is_none() {
                        // A reinstall failed: try again now, without waiting for the backoff.
                        if hooks.reinstall(REINSTALL_WATCHDOG) {
                            watchdog = Watchdog::new(now);
                        }
                    } else {
                        let input = last_input();
                        let mut missed =
                            hooks_missed_input(input, newest(Some(last_callback()), excused()));
                        let hidden = if missed {
                            input_hidden_from_hooks()
                        } else {
                            Hidden::No
                        };
                        if hidden != Hidden::No {
                            // Typed into an elevated window or on the secure desktop: our
                            // hooks were never going to see it (#135).
                            excuse(input);
                            missed = false;
                        }
                        if hidden == Hidden::Elevated && !told_elevated {
                            // Once per hook, so the app can say why nothing is fixed there.
                            told_elevated = true;
                            with_state(|s| {
                                let _ = s.events.send(HookEvent::ElevatedWindow);
                            });
                        }
                        if watchdog.should_reinstall(now, missed, callback_seen()) {
                            hooks.reinstall(REINSTALL_WATCHDOG);
                        }
                    }
                }
                WM_TIMER if msg.hwnd.is_invalid() => {
                    with_state(State::on_timer);
                }
                WM_APP_REARM if msg.hwnd.is_invalid() => {
                    let reason = match msg.wParam.0 {
                        REARM_SUSPEND => {
                            resumes.on_suspend();
                            None
                        }
                        REARM_UNLOCK => Some(REINSTALL_UNLOCK),
                        _ => resumes.on_resume(tick_now()).then_some(REINSTALL_RESUME),
                    };
                    if let Some(reason) = reason {
                        hooks.reinstall(reason);
                        watchdog = Watchdog::new(tick_now());
                    }
                }
                _ => {
                    DispatchMessageW(&msg);
                }
            }
        }
        if watch_timer != 0 {
            let _ = KillTimer(HWND::default(), watch_timer);
        }
        if let Some(session) = session {
            session.destroy();
        }
        hooks.uninstall();
        // Pausing, quitting or restarting the hook: our hooks are gone, so whatever the user
        // typed that we still hold (a held Backspace, keys held back behind an injection, an
        // injection's trigger that never went out) goes straight to the app, in order. Nothing
        // is dropped, a modifier's key-up included.
        if let Some(owed) = with_state(State::drain_for_stop) {
            if !owed.is_empty() {
                let _ = inject::send(&inject::pass_inputs(&owed), |sent| {
                    inject::pass_inputs(&inject::unwind_keys(&owed, sent))
                });
            }
        }
    }
    take_report()
}

fn take_report() -> HookReport {
    let state = STATE.with(|s| s.borrow_mut().take());
    state
        .map(|mut s| {
            s.set_timer(false);
            s.report
        })
        .unwrap_or_default()
}

struct Installed {
    /// `None` after a reinstall failed to install it again.
    keyboard: Option<HHOOK>,
    mouse: Option<HHOOK>,
    win_events: Vec<HWINEVENTHOOK>,
}

fn tick_now() -> u32 {
    // SAFETY: trivial query.
    unsafe { GetTickCount() }
}

fn last_callback() -> u32 {
    LAST_CALLBACK.with(Cell::get)
}

fn callback_seen() -> bool {
    CALLBACK_SEEN.with(Cell::get)
}

/// A real keyboard or mouse callback.
fn mark_callback() {
    let _ = LAST_CALLBACK.try_with(|c| c.set(tick_now()));
    let _ = CALLBACK_SEEN.try_with(|c| c.set(true));
}

fn excused() -> Option<u32> {
    EXCUSED.with(Cell::get)
}

/// Input up to tick `until` doesn't count as missed.
fn excuse(until: u32) {
    let _ = EXCUSED.try_with(|c| c.set(Some(newest(c.get(), Some(until)))));
}

/// The later of two ticks (across the 49.7-day wrap), as the watchdog's baseline.
fn newest(a: Option<u32>, b: Option<u32>) -> u32 {
    match (a, b) {
        (Some(a), Some(b)) => {
            if (b.wrapping_sub(a) as i32) > 0 {
                b
            } else {
                a
            }
        }
        (Some(t), None) | (None, Some(t)) => t,
        (None, None) => 0,
    }
}

/// Is input right now going where a low-level hook of ours can't see it? Windows doesn't call
/// a process's low-level hooks for input to a window of a higher integrity level (an elevated
/// app, UIPI), nor on the secure desktop (UAC prompt, lock screen, Ctrl+Alt+Del), where there
/// is no foreground window for us to read. `GetLastInputInfo` counts that input all the same,
/// which made the watchdog reinstall the hooks over and over (#135).
fn input_hidden_from_hooks() -> Hidden {
    let (hwnd, pid) = focus::foreground();
    if hwnd.is_invalid() || pid == 0 {
        return Hidden::SecureDesktop;
    }
    if hidden_by_integrity(integrity::of_process(pid), integrity::own()) {
        Hidden::Elevated
    } else {
        Hidden::No
    }
}

/// Where input goes that our hooks can't see ([`input_hidden_from_hooks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hidden {
    No,
    /// No foreground window of ours to read: UAC prompt, lock screen, Ctrl+Alt+Del.
    SecureDesktop,
    /// A window of a higher integrity level (an administrator app), or one we can't query.
    Elevated,
}

/// Input to a process of integrity level `foreground` is invisible to hooks of level `own`. A
/// foreground process we can't even query counts as invisible (elevated or protected).
fn hidden_by_integrity(foreground: Option<u32>, own: Option<u32>) -> bool {
    match (foreground, own) {
        (Some(f), Some(o)) => f > o,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

mod integrity {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Integrity level (the mandatory label's RID: 0x2000 medium, 0x3000 high) of `pid`.
    pub fn of_process(pid: u32) -> Option<u32> {
        // SAFETY: the handle is closed below.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let level = of_handle(process);
            let _ = CloseHandle(process);
            level
        }
    }

    /// This process's integrity level.
    pub fn own() -> Option<u32> {
        // SAFETY: the pseudo handle needs no closing.
        unsafe { of_handle(GetCurrentProcess()) }
    }

    unsafe fn of_handle(process: HANDLE) -> Option<u32> {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        // u64s: TOKEN_MANDATORY_LABEL holds a pointer and must be aligned for it.
        let mut buf = [0u64; 16];
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr().cast()),
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.ok()?;
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, u32::from(count - 1)))
    }
}

/// The hooks were just (re)installed: input before now isn't theirs to have missed, but nothing
/// has shown yet that they work.
fn mark_installed() {
    let _ = LAST_CALLBACK.try_with(|c| c.set(tick_now()));
    let _ = CALLBACK_SEEN.try_with(|c| c.set(false));
}

/// Tick of the last input the system saw in this session, or 0 if unknown.
fn last_input() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: info is a valid, sized LASTINPUTINFO.
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        info.dwTime
    } else {
        0
    }
}

/// What a reinstall reports: the reason when the keyboard hook is back, otherwise that it is
/// lost. A failed reinstall used to report success with the old, already removed hook.
fn reinstall_event(reason: &'static str, keyboard: Result<(), String>) -> HookEvent {
    match keyboard {
        Ok(()) => HookEvent::HooksReinstalled { reason },
        Err(error) => HookEvent::KeyboardHookLost { error },
    }
}

/// Input this long after our last callback means the hooks missed it.
const MISSED_AFTER_MS: u32 = 2_000;

/// Has the system seen input our hooks didn't? Tick counts wrap every 49.7 days, so this
/// compares the signed difference.
fn hooks_missed_input(last_input: u32, last_callback: u32) -> bool {
    last_input != 0 && (last_input.wrapping_sub(last_callback) as i32) > MISSED_AFTER_MS as i32
}

/// Decides when to install the hooks again: at once the first time they miss input, then with
/// a doubling pause while they keep missing it (touch input, or a secure desktop, can look like
/// missed input forever), back to normal only once a real callback has arrived since the last
/// reinstall. A quiet spell right after a reinstall (no input, so nothing missed) proves
/// nothing and keeps the pause.
struct Watchdog {
    not_before: u32,
    backoff_ms: u32,
}

impl Watchdog {
    const FIRST_BACKOFF_MS: u32 = 30_000;
    const MAX_BACKOFF_MS: u32 = 10 * 60_000;

    fn new(now: u32) -> Self {
        Self {
            not_before: now,
            backoff_ms: Self::FIRST_BACKOFF_MS,
        }
    }

    /// `missed`: the system saw input our hooks didn't. `seen`: a real callback arrived since
    /// the hooks were last (re)installed.
    fn should_reinstall(&mut self, now: u32, missed: bool, seen: bool) -> bool {
        if !missed {
            if seen {
                self.backoff_ms = Self::FIRST_BACKOFF_MS;
            }
            return false;
        }
        if (now.wrapping_sub(self.not_before) as i32) < 0 {
            return false;
        }
        self.not_before = now.wrapping_add(self.backoff_ms);
        self.backoff_ms = (self.backoff_ms * 2).min(Self::MAX_BACKOFF_MS);
        true
    }
}

/// One re-arm per resume. Windows sends `PBT_APMRESUMEAUTOMATIC` on every resume and
/// `PBT_APMRESUMESUSPEND` as well when a user is present (sometimes minutes later), so a resume
/// re-arms only if a suspend came before it. A system that never reports suspends falls back to
/// at most one resume re-arm per [`ResumeGate::QUIET_MS`].
#[derive(Debug, Default)]
struct ResumeGate {
    suspended: bool,
    saw_suspend: bool,
    last_rearm: Option<u32>,
}

impl ResumeGate {
    const QUIET_MS: u32 = 60_000;

    fn on_suspend(&mut self) {
        self.suspended = true;
        self.saw_suspend = true;
    }

    /// A resume notification at tick `now`: re-arm?
    fn on_resume(&mut self, now: u32) -> bool {
        let rearm = if self.saw_suspend {
            std::mem::take(&mut self.suspended)
        } else {
            self.last_rearm
                .is_none_or(|t| now.wrapping_sub(t) >= Self::QUIET_MS)
        };
        if rearm {
            self.last_rearm = Some(now);
        }
        rearm
    }
}

/// A message-only window that hears about sleep, resume and session unlock, and tells the hook
/// thread with `WM_APP_REARM`.
struct SessionWatch {
    hwnd: HWND,
    power: Option<HPOWERNOTIFY>,
    wts: bool,
}

impl SessionWatch {
    unsafe fn create() -> Option<Self> {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(session_proc),
            hInstance: module.into(),
            lpszClassName: w!("unflub-hook-session"),
            ..Default::default()
        };
        // Fails harmlessly when a previous hook thread already registered it.
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("unflub-hook-session"),
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            module,
            None,
        )
        .ok()?;
        let power =
            RegisterSuspendResumeNotification(HANDLE(hwnd.0), DEVICE_NOTIFY_WINDOW_HANDLE).ok();
        let wts = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_ok();
        Some(Self { hwnd, power, wts })
    }

    unsafe fn destroy(self) {
        if let Some(power) = self.power {
            let _ = UnregisterSuspendResumeNotification(power);
        }
        if self.wts {
            let _ = WTSUnRegisterSessionNotification(self.hwnd);
        }
        let _ = DestroyWindow(self.hwnd);
    }
}

unsafe extern "system" fn session_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let rearm = match msg {
        WM_POWERBROADCAST
            if matches!(
                wparam.0 as u32,
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND
            ) =>
        {
            Some(REARM_RESUME)
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_APMSUSPEND => Some(REARM_SUSPEND),
        WM_WTSSESSION_CHANGE if wparam.0 as u32 == WTS_SESSION_UNLOCK => Some(REARM_UNLOCK),
        _ => None,
    };
    if let Some(kind) = rearm {
        // The hook thread re-arms once per resume (`ResumeGate`).
        let _ = PostThreadMessageW(GetCurrentThreadId(), WM_APP_REARM, WPARAM(kind), LPARAM(0));
    }
    if msg == WM_POWERBROADCAST {
        return LRESULT(1);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

impl Installed {
    /// Remove and install the keyboard and mouse hooks again. The word in progress is
    /// forgotten: keys may have been missed. Returns whether the keyboard hook is installed;
    /// when it isn't, that is reported instead of a reinstall (#143).
    unsafe fn reinstall(&mut self, reason: &'static str) -> bool {
        if let Some(k) = self.keyboard.take() {
            let _ = UnhookWindowsHookEx(k);
        }
        if let Some(m) = self.mouse.take() {
            let _ = UnhookWindowsHookEx(m);
        }
        let module = GetModuleHandleW(None).unwrap_or_default();
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0)
            .map_err(|e| e.to_string());
        self.keyboard = keyboard.as_ref().ok().copied();
        let installed = self.keyboard.is_some();
        self.mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0).ok();
        mark_installed();
        with_state(|s| {
            s.reset();
            let _ = s.events.send(reinstall_event(reason, keyboard.map(|_| ())));
            // Echoes of a batch in flight may have been lost with the old hook: don't wait for
            // them. A probe right away tells whether our input comes back now.
            if s.pipeline.busy() {
                s.pipeline.abandon();
                s.enter_degraded(Duration::ZERO);
                s.pump();
            }
            s.maybe_probe(true);
        });
        installed
    }

    unsafe fn uninstall(self) {
        if let Some(k) = self.keyboard {
            let _ = UnhookWindowsHookEx(k);
        }
        if let Some(m) = self.mouse {
            let _ = UnhookWindowsHookEx(m);
        }
        for h in self.win_events {
            let _ = UnhookWinEvent(h);
        }
    }
}

unsafe fn install() -> Result<Installed, String> {
    mark_installed();
    let module = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0)
        .map_err(|e| format!("keyboard hook: {e}"))?;
    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0).ok();
    let mut win_events = Vec::new();
    for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS] {
        let h = SetWinEventHook(
            event,
            event,
            HMODULE::default(),
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        if !h.is_invalid() {
            win_events.push(h);
        }
    }
    let missing = [
        mouse.is_none().then_some("mouse hook"),
        (win_events.len() < 2).then_some("focus events"),
    ];
    for m in missing.into_iter().flatten() {
        with_state(|s| {
            let _ = s.events.send(HookEvent::Warning(format!(
                "{m} unavailable; caret moves from it won't reset the word"
            )));
        });
    }
    Ok(Installed {
        keyboard: Some(keyboard),
        mouse,
        win_events,
    })
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    mark_callback();
    if code == HC_ACTION as i32 {
        let start = Instant::now();
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT valid for this call.
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if with_state(|s| s.on_keyboard(wparam.0 as u32, kb, start)) == Some(true) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Called for every mouse move system-wide: note the time for the watchdog (one tick read),
    // then bail out before touching any state.
    mark_callback();
    if code == HC_ACTION as i32
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        with_state(|s| {
            s.reset();
            // The click may have focused another field inside the same window.
            s.refresh_focus(None, INPUT_RECHECK_DELAY);
        });
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Focus moving to a menu, scroll bar, caret etc. isn't a text field changing. Anything
    // else re-checks the field; a burst of events costs one UIA query, since the worker only
    // answers the latest generation.
    if event == EVENT_OBJECT_FOCUS && !focus::is_focus_change_object(id_object) {
        return;
    }
    // A focus event names the focused window; a foreground event names the top-level one, so
    // look up its focused child instead.
    let target = (event == EVENT_OBJECT_FOCUS).then_some(hwnd);
    if event == EVENT_SYSTEM_FOREGROUND {
        // Input before the switch may have gone to a window our hooks can't see (an elevated
        // one that closed or lost the foreground): it doesn't count as missed. Input after the
        // switch still does.
        excuse(tick_now());
    }
    with_state(|s| {
        // Reset first: the word-start question asked by the refresh belongs to this reset.
        s.reset();
        s.refresh_focus(target, Duration::ZERO);
    });
}

/// Keyboard layout of the thread that owns the focused window, which is the one that turns
/// keys into text. It isn't always the foreground window's thread: a child window can belong
/// to another thread (a plug-in, an embedded editor) with its own layout. Falls back to the
/// foreground window when nothing has focus.
fn foreground_layout() -> HKL {
    let (hwnd, _) = focus::foreground();
    let focused = focus::focus_window(hwnd);
    // SAFETY: trivial queries.
    unsafe { GetKeyboardLayout(GetWindowThreadProcessId(focused, None)) }
}

/// Reverse-map printable ASCII through the focused thread's layout without touching the
/// keyboard or dead-key state. Positions use the main typing area's set-1 scan codes.
fn keyboard_adjacency(layout: HKL) -> Adjacency {
    if layout.0.is_null() {
        return Adjacency::default();
    }
    let positions = (b'a'..=b'z')
        .chain(b"1234567890-=[];',./".iter().copied())
        .filter_map(|ch| {
            // SAFETY: mapping queries for the current input locale, no state changes.
            let key = unsafe { VkKeyScanExW(u16::from(ch), layout) };
            if !matches!(key >> 8, 0 | 1) {
                return None; // Unmapped or requires a modifier other than Shift.
            }
            let scan = unsafe { MapVirtualKeyExW(u32::from(key as u8), MAPVK_VK_TO_VSC, layout) };
            let (row, col) = match scan {
                0x02..=0x0d => (0, scan as i8 - 0x02),
                0x10..=0x1b => (1, scan as i8 - 0x10),
                0x1e..=0x29 => (2, scan as i8 - 0x1e),
                0x2c..=0x35 => (3, scan as i8 - 0x2c),
                0x56 => (3, -1), // ISO key between left Shift and the first letter.
                _ => return None,
            };
            Some((ch, row, col))
        })
        .collect::<Vec<_>>();
    if positions
        .iter()
        .take_while(|(ch, _, _)| ch.is_ascii_lowercase())
        .count()
        == 26
    {
        Adjacency::from_key_positions(positions)
    } else {
        Adjacency::default()
    }
}

/// Ctrl, Alt, Shift or Win held: a Backspace with one of these isn't a plain second Backspace.
fn modifier_down() -> bool {
    // SAFETY: trivial queries.
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000 != 0)
}

/// The Shift keys held right now. Keys held back behind an injection were swallowed, so this
/// is also the state the app has seen.
fn held_shift() -> inject::ShiftKeys {
    inject::ShiftKeys {
        left: already_down(VK_LSHIFT.0),
        right: already_down(VK_RSHIFT.0),
    }
}

/// Is `vk` already down? Async state doesn't include the event being hooked yet, so for a
/// key-down this is true exactly when it is an auto-repeat of a press that went through.
fn already_down(vk: u16) -> bool {
    // SAFETY: trivial query.
    unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
}

/// Physical modifier state plus a `ToUnicodeEx` key-state array built from it. Async state
/// reflects every key before the one being hooked, which is exactly what we need.
fn read_keyboard() -> (Modifiers, [u8; 256]) {
    let mut state = [0u8; 256];
    // SAFETY: trivial queries.
    let down = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 };
    for vk in [
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ] {
        if down(vk.0) {
            state[usize::from(vk.0)] = 0x80;
        }
    }
    let either = |a: u16, b: u16| state[usize::from(a)] | state[usize::from(b)] != 0;
    let mods = Modifiers {
        shift: either(VK_LSHIFT.0, VK_RSHIFT.0),
        ctrl: either(VK_LCONTROL.0, VK_RCONTROL.0),
        alt: either(VK_LMENU.0, VK_RMENU.0),
        win: either(VK_LWIN.0, VK_RWIN.0),
        // SAFETY: trivial query. Toggle state (low bit) of Caps Lock.
        caps_lock: unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) } & 1 != 0,
    };
    for (on, vk) in [
        (mods.shift, VK_SHIFT),
        (mods.ctrl, VK_CONTROL),
        (mods.alt, VK_MENU),
    ] {
        if on {
            state[usize::from(vk.0)] = 0x80;
        }
    }
    if mods.caps_lock {
        state[usize::from(VK_CAPITAL.0)] = 0x01;
    }
    (mods, state)
}

/// What `vk` types in `layout`, without touching the dead-key state.
fn to_unicode(vk: u16, scan: u16, key_state: &[u8; 256], layout: HKL) -> Typed {
    let mut buf = [0u16; 8];
    // SAFETY: valid buffers; flag 0x4 keeps the kernel keyboard state unchanged.
    let ret = unsafe {
        ToUnicodeEx(
            u32::from(vk),
            u32::from(scan),
            key_state,
            &mut buf,
            TOUNICODE_NO_STATE_CHANGE,
            layout,
        )
    };
    Typed::from_to_unicode(ret, &buf)
}

impl State {
    /// Every keyboard event. Returns true to swallow it.
    fn on_keyboard(&mut self, msg: u32, kb: &KBDLLHOOKSTRUCT, start: Instant) -> bool {
        let injected = kb.flags.0 & LLKHF_INJECTED.0 != 0;
        let tag = injected.then(|| Tag::decode(kb.dwExtraInfo)).flatten();
        if let Some(Tag::Probe(_)) = tag {
            // Our injections reach our hook: leave degraded. No app ever sees the probe.
            if self.pipeline.degraded() {
                self.pipeline.recover();
                self.probe_after = None;
                self.degrade_backoff.recovered_at = Some(start);
                let _ = self
                    .events
                    .send(HookEvent::InputInterference { active: false });
            }
            return true;
        }
        if let Some(Tag::Inject(batch)) = tag {
            // One of our own correction events coming back: count it and let it through.
            if let Some(f) = self.pipeline.on_injected(batch) {
                self.finished(f);
            }
            self.pump();
            return false;
        }
        let (origin, foreign) = match tag {
            Some(Tag::Replay { batch, foreign }) => (Origin::Replay(batch), foreign),
            _ => (Origin::User, injected),
        };
        let vk = kb.vkCode as u16;
        if matches!(origin, Origin::User) && (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) {
            self.note_typing();
        }
        let ev = RawEvent {
            vk,
            scan: kb.scanCode as u16,
            extended: kb.flags.0 & LLKHF_EXTENDED.0 != 0,
            up: !(msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN),
            foreign,
        };
        let async_down = !ev.up && already_down(vk);
        let replayed = matches!(origin, Origin::Replay(_));
        // A held first Backspace goes out before any other key-down, user or replayed (the
        // pipeline then holds this key behind it), so deleting the space to add a comma keeps
        // working. Only a fresh, plain second Backspace from the user reaches the corrector,
        // which undoes the fix. There is no timeout: the hold ends with the next key, a click,
        // or the hook stopping.
        if !ev.up
            && self.held.is_some()
            && crate::policy::releases_held_backspace(
                vk,
                async_down,
                !replayed && modifier_down(),
                replayed,
            )
        {
            self.release_held();
        }
        let swallow = match self.pipeline.on_input(ev, origin, async_down) {
            Input::Queued | Input::Consumed => true,
            Input::Process { repeat } => !ev.up && self.on_key_down(ev, repeat, replayed, start),
        };
        self.pump();
        self.report.keyboard.record(start.elapsed());
        swallow
    }

    fn on_key_down(&mut self, ev: RawEvent, repeat: bool, replayed: bool, start: Instant) -> bool {
        if MISSED.with(Cell::take) {
            self.reset();
        }
        if self.broken {
            return false;
        }
        let (mods, key_state) = if ev.foreign {
            (Modifiers::default(), [0u8; 256])
        } else {
            read_keyboard()
        };
        let layout = foreground_layout();
        let raw = RawKey {
            vk: ev.vk,
            scan: ev.scan,
            down: true,
            foreign_injected: ev.foreign,
        };
        let key = self.translator.translate(raw, mods, || {
            to_unicode(raw.vk, raw.scan, &key_state, layout)
        });
        let swallow = match key {
            Some(key) => {
                let ime = is_ime_language((layout.0 as usize & 0xFFFF) as u16);
                if !ime && self.rank_layout != Some(layout) {
                    self.corrector.set_adjacency(keyboard_adjacency(layout));
                    self.rank_layout = Some(layout);
                }
                self.handle_key(key, ev, mods, repeat, replayed, ime, start)
            }
            None => false,
        };
        // Tab and Enter can move focus or submit a form without any focus WinEvent (Chromium
        // without its accessibility tree): check the field again once the app has reacted.
        if ev.vk == VK_TAB.0 || ev.vk == VK_RETURN.0 {
            self.refresh_focus(None, INPUT_RECHECK_DELAY);
        }
        swallow
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_key(
        &mut self,
        key: Key,
        ev: RawEvent,
        mods: Modifiers,
        repeat: bool,
        replayed: bool,
        ime: bool,
        start: Instant,
    ) -> bool {
        if self.pipeline.degraded() {
            // Nothing is injected while our input doesn't come back (the corrector was reset
            // on the way in).
            return false;
        }
        // Read once: the UIA answer may land at any moment.
        let check = self.focus.check();
        let other_block = ime || self.excluded();
        let word_end = self.word_ends.ends_word(key);
        if word_end && check != FieldCheck::Cleared && !other_block {
            if let Some(usage) = &self.usage {
                usage.skipped_secure.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.take_word_start();
        let kept = if other_block {
            self.pending_word.clear();
            Vec::new()
        } else {
            match self.pending_word.on_key(key, check) {
                // Kept until UIA answers; the key goes to the app as usual (#110).
                None => return false,
                Some(letters) => letters,
            }
        };
        self.feed_kept(kept);
        let blocked = other_block || check != FieldCheck::Cleared;
        match self.gate.decide(key, blocked) {
            GateDecision::Feed => {}
            GateDecision::ResetOnly => {
                self.reset_corrector();
                return false;
            }
            GateDecision::Drop => return false,
        }
        let decide_start = Instant::now();
        let Some(action) = self.call(key) else {
            return false;
        };
        let decide = decide_start.elapsed();
        if word_end {
            if let Some(usage) = &self.usage {
                usage.record_decide(decide);
            }
        }
        if action == Action::Pass {
            return false;
        }
        // A key another program injected carries no modifier state of its own, but Ctrl, Alt
        // or Win held on the keyboard still turn our Backspaces into shortcuts.
        let mods = if ev.foreign {
            veto_modifiers(mods, read_keyboard().0)
        } else {
            mods
        };
        let (replacement, undo) = match action {
            Action::Pass => return false,
            Action::Replace(r) => (r, false),
            Action::Undo(r) => (r, true),
            Action::HoldBackspace(r) => {
                return self.hold_backspace(r, key, ev, mods, repeat, replayed)
            }
        };
        if let Some(reason) = injection_veto(key, mods, repeat, self.dry_run) {
            return self.skip(replacement, reason);
        }
        let strokes = if undo {
            inject::plan(replacement.delete, &replacement.insert)
        } else {
            inject::plan_correction(
                replacement.delete,
                &replacement.insert,
                replacement.original.chars().last(),
            )
        };
        // Real keys of the focused app's layout where it has them (#33).
        // Caps Lock as Windows has it: `mods` is empty for keys another program injected.
        // SAFETY: trivial query. Toggle state (low bit) of Caps Lock.
        let caps_lock = unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) } & 1 != 0;
        let keys = inject::TargetLayout::new(foreground_layout(), caps_lock);
        let shift = held_shift();
        let strokes = inject::with_keys(strokes, &keys, shift);
        if let Some(reason) = inject::packet_veto(self.focus.app.as_deref(), &strokes, &keys) {
            return self.skip(replacement, reason);
        }
        let events = inject::key_events(&strokes, shift);
        // The second Backspace: the held one goes with the undo, and back to the app with it if
        // SendInput takes none of the undo (see `Pipeline::begin_undo`).
        let held = if undo {
            self.undo_deadline = None;
            self.set_held(None)
        } else {
            self.undo_deadline = self.corrector.undo_window().map(|w| Instant::now() + w);
            None
        };
        // Swallow the key and hand the batch over; SendInput runs on the injector thread.
        let batch = self.pipeline.begin_undo(ev, held, events.len());
        let handed_over = Instant::now();
        let job = Job {
            kind: JobKind::Inject,
            batch,
            inputs: inject::to_inputs(&events, batch),
            handed_over,
            unwind: Unwind::Injection {
                events,
                held: shift,
            },
        };
        self.batches.push(Batch {
            batch,
            replacement,
            undo,
            app: self.focus.app.clone(),
            took: start.elapsed(),
            swallowed_at: handed_over,
            handed_over,
            timing: InjectTiming {
                decide,
                ..InjectTiming::default()
            },
            inject_reported: false,
            released: false,
        });
        if self.jobs.send(job).is_err() {
            // Injector gone: nothing was sent, so the pipeline gives the key back.
            if let Some(f) = self.pipeline.on_sent(batch, 0) {
                self.finished(f);
            }
        }
        true
    }

    /// Injector results arrived (`WM_APP_SENT`).
    fn on_results(&mut self) {
        while let Ok(r) = self.done.try_recv() {
            match r.kind {
                JobKind::Inject => {
                    if let Some(b) = self.batches.iter_mut().find(|b| b.batch == r.batch) {
                        b.timing.inject = r.took;
                        b.inject_reported = true;
                        b.released = r.released;
                    }
                    self.report.inject.record(r.took);
                    if let Some(f) = self.pipeline.on_sent(r.batch, r.sent) {
                        self.finished(f);
                    }
                }
                JobKind::Probe => {}
                JobKind::Replay => {
                    let lost = self.pipeline.on_replay_sent(r.batch, r.sent, r.total);
                    if lost > 0 {
                        self.reset();
                        let _ = self.events.send(HookEvent::Warning(format!(
                            "{lost} held-back key events could not be replayed (SendInput refused them)"
                        )));
                    }
                }
            }
        }
        self.pump();
    }

    /// Letters typed while the field was pending, now cleared: the corrector gets them in order,
    /// as it would have (they are already in the app). They can't lead to an action; if one
    /// does anyway, the rest of the word is dropped as for a field that turned secure.
    fn feed_kept(&mut self, letters: Vec<char>) {
        for c in letters {
            let key = Key::Char(c);
            match self.gate.decide(key, false) {
                GateDecision::Feed => {}
                GateDecision::ResetOnly => {
                    self.reset_corrector();
                    continue;
                }
                GateDecision::Drop => continue,
            }
            match self.call(key) {
                Some(Action::Pass) => {}
                Some(_) => {
                    if self.gate.decide(key, true) == GateDecision::ResetOnly {
                        self.reset_corrector();
                    }
                    return;
                }
                None => return,
            }
        }
    }

    /// If the UIA worker has answered the word-start question, tell the corrector (only a "yes"
    /// matters). An atomic read; never waits.
    fn take_word_start(&mut self) {
        let Some((focus_generation, reset_generation)) = self.word_start else {
            return;
        };
        match self.focus.slot.word_start(focus_generation) {
            None => {}
            Some(answer) => {
                self.word_start = None;
                if answer == focus::WordStart::Yes {
                    self.corrector.caret_at_word_start(reset_generation);
                }
            }
        }
    }

    /// A first Backspace after a fix: swallow it and hold it (the pill asks "Undo?"), unless
    /// it mustn't be swallowed (dry run, modifier held, auto-repeat) or it is a replay (keys
    /// typed after it may already be on their way to the app).
    fn hold_backspace(
        &mut self,
        replacement: Replacement,
        key: Key,
        ev: RawEvent,
        mods: Modifiers,
        repeat: bool,
        replayed: bool,
    ) -> bool {
        if !crate::policy::may_hold_backspace(replayed) {
            // A plain Backspace after all: close the window and let the engine count it as one.
            if self.corrector.end_undo_window() {
                let _ = self.call(Key::Backspace);
            }
            return false;
        }
        if let Some(reason) = injection_veto(key, mods, repeat, self.dry_run) {
            self.corrector.end_undo_window();
            return self.skip(replacement, reason);
        }
        self.pipeline.hold(ev);
        self.set_held(Some(ev));
        // Held until answered: no timer may end it (see `check_undo_deadline`).
        self.undo_deadline = None;
        let _ = self.events.send(HookEvent::UndoPending { replacement });
        true
    }

    /// Set or clear the held Backspace (kept in [`HELD_FOR_CRASH`] too). Returns the old one.
    fn set_held(&mut self, held: Option<RawEvent>) -> Option<RawEvent> {
        HELD_FOR_CRASH.store(held.map_or(0, encode_held), Ordering::Release);
        std::mem::replace(&mut self.held, held)
    }

    /// Send a held Backspace on, ahead of anything typed after it, and close the undo window.
    fn release_held(&mut self) {
        if let Some(held) = self.set_held(None) {
            self.corrector.end_undo_window();
            self.pipeline.release_held(held);
            let _ = self.events.send(HookEvent::UndoDeclined);
        }
        self.undo_deadline = None;
    }

    /// A click or focus change: a held Backspace was meant for the old caret, so it is dropped
    /// (sending it now would delete a character wherever the click put the caret).
    fn drop_held(&mut self) {
        if self.set_held(None).is_some() {
            self.corrector.end_undo_window();
            let _ = self.events.send(HookEvent::UndoDeclined);
        }
        self.undo_deadline = None;
    }

    /// The time to start a double-Backspace undo ran out (the fix's pill is gone): the next
    /// Backspace is a plain one. A Backspace already held is never released by time; it waits
    /// for the next key, which releases it (so it never holds other input back).
    fn check_undo_deadline(&mut self) {
        if self.held.is_none() && self.undo_deadline.is_some_and(|d| Instant::now() >= d) {
            self.corrector.end_undo_window();
            self.undo_deadline = None;
        }
    }

    /// The hook is stopping (pause, quit, engine stop, language change) and its hooks are gone:
    /// let the injector finish what it was handed (up to [`STOP_DRAIN`]), then return every
    /// user key still owed to the app, in order: a held Backspace first (anything typed after
    /// it would have released it), then what the pipeline holds ([`Pipeline::drain`]).
    fn drain_for_stop(&mut self) -> Vec<RawEvent> {
        // Dropping our sender ends the injector once its queue is empty.
        self.jobs = mpsc::channel().0;
        let deadline = Instant::now() + STOP_DRAIN;
        while let Ok(r) = self
            .done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if let JobKind::Inject = r.kind {
                // No echo can come back now: only the count matters.
                let _ = self.pipeline.on_sent(r.batch, r.sent);
            }
        }
        let mut owed = Vec::new();
        if let Some(held) = self.set_held(None) {
            owed.extend([held, RawEvent { up: true, ..held }]);
        }
        owed.extend(self.pipeline.drain());
        owed
    }

    /// A user key-down: count the minute it falls in, once.
    fn note_typing(&mut self) {
        if let Some(usage) = &self.usage {
            if self.minutes.new_minute(tick_now()) {
                usage.active_minutes.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// The stale-batch timer fired.
    fn on_timer(&mut self) {
        self.check_undo_deadline();
        let now = Instant::now();
        if self.pipeline.tick(now, STALE_AFTER) {
            let wait = self.degrade_backoff.stalled(now);
            self.enter_degraded(wait);
        }
        self.pump();
    }

    /// The pipeline gave up waiting for our own events: stop correcting, tell the app, and
    /// probe again after `wait`.
    fn enter_degraded(&mut self, wait: Duration) {
        self.degrade_backoff.recovered_at = None;
        self.reset();
        // Abandoned batches will never be reported.
        self.batches.clear();
        self.probe_after = Some(Instant::now() + wait);
        let _ = self
            .events
            .send(HookEvent::InputInterference { active: true });
    }

    /// While degraded, send one probe when it is due (`now`: don't wait).
    fn maybe_probe(&mut self, now: bool) {
        if !self.pipeline.degraded() {
            return;
        }
        if !now && self.probe_after.is_some_and(|t| Instant::now() < t) {
            return;
        }
        self.next_probe = self.next_probe.wrapping_add(1);
        let job = Job {
            kind: JobKind::Probe,
            batch: self.next_probe,
            inputs: inject::probe_inputs(self.next_probe),
            handed_over: Instant::now(),
            unwind: Unwind::Nothing,
        };
        let _ = self.jobs.send(job);
    }

    /// An injection batch is over: report it.
    fn finished(&mut self, f: Finished) {
        let Some(i) = self.batches.iter().position(|b| b.batch == f.batch) else {
            return;
        };
        let mut b = self.batches.swap_remove(i);
        if f.sent < f.total {
            // Let the corrector forget the word; the pipeline already gave the key back if
            // nothing was typed.
            self.reset();
            let _ = self.events.send(HookEvent::InjectFailed {
                replacement: b.replacement,
                sent: f.sent,
                total: f.total,
                released: b.released,
            });
            return;
        }
        b.finish_timing(Instant::now());
        self.report.decide.record(b.timing.decide);
        self.report.end_to_end.record(b.timing.end_to_end);
        let event = if b.undo {
            HookEvent::Undone {
                replacement: b.replacement,
                app: b.app,
                took: b.took,
                timing: b.timing,
            }
        } else {
            HookEvent::Corrected {
                replacement: b.replacement,
                app: b.app,
                took: b.took,
                timing: b.timing,
            }
        };
        let _ = self.events.send(event);
    }

    /// Hand due replays to the injector and keep the stale-batch timer in step.
    fn pump(&mut self) {
        for r in self.pipeline.pump(Instant::now()) {
            let total = r.events.len();
            let job = Job {
                kind: JobKind::Replay,
                batch: r.batch,
                inputs: inject::replay_inputs(&r.events, r.batch),
                handed_over: Instant::now(),
                unwind: Unwind::Keys(r.events),
            };
            if self.jobs.send(job).is_err() {
                self.pipeline.on_replay_sent(r.batch, 0, total);
            }
        }
        // The same timer also closes the undo window (within one tick of its end).
        self.set_timer(self.pipeline.busy() || self.undo_deadline.is_some());
    }

    fn set_timer(&mut self, on: bool) {
        // SAFETY: thread timers on this thread; WM_TIMER arrives in our message loop.
        unsafe {
            if on && self.timer == 0 {
                self.timer = SetTimer(HWND::default(), 0, STALE_CHECK_MS, None);
            } else if !on && self.timer != 0 {
                let _ = KillTimer(HWND::default(), self.timer);
                self.timer = 0;
            }
        }
    }

    /// The user excluded the focused app, or unflub never corrects there (VM consoles,
    /// terminals: [`crate::policy::never_correct`]).
    fn excluded(&self) -> bool {
        self.focus.app.as_ref().is_some_and(|app| {
            self.excluded.iter().any(|e| e == app) || crate::policy::never_correct(app).is_some()
        })
    }

    /// Feed one key to the corrector, catching panics so a bug can't take the keyboard down.
    fn call(&mut self, key: Key) -> Option<Action> {
        if self.broken {
            return None;
        }
        let ctx = Context {
            app: self.focus.app.as_deref(),
            secure: self.focus.secure(),
        };
        let corrector = &mut self.corrector;
        match catch_unwind(AssertUnwindSafe(|| corrector.on_key(key, &ctx))) {
            Ok(action) => Some(action),
            Err(payload) => {
                self.broken = true;
                let _ = self
                    .events
                    .send(HookEvent::CorrectorPanicked(panic_message(&*payload)));
                None
            }
        }
    }

    fn skip(&mut self, replacement: Replacement, reason: &'static str) -> bool {
        self.reset_corrector();
        let _ = self.events.send(HookEvent::Skipped {
            replacement,
            reason,
        });
        false
    }

    /// Tell the corrector to forget the word. Its answer is ignored.
    fn reset_corrector(&mut self) {
        let _ = self.call(Key::Reset);
    }

    /// Caret moved, focus changed, or we lost track: forget the word and start fresh.
    fn reset(&mut self) {
        self.translator.forget_selection();
        self.drop_held();
        self.pending_word.clear();
        self.gate.on_reset();
        self.reset_corrector();
    }

    /// Re-read the foreground app and the focused field, and ask UIA about it if its class
    /// needs that. `target` is the focused window when the caller knows it.
    fn refresh_focus(&mut self, target: Option<HWND>, probe_delay: Duration) {
        let (foreground, pid) = focus::foreground();
        if pid != self.focus.app_pid {
            self.focus.app_pid = pid;
            self.focus.app = focus::process_exe(pid);
        }
        let target = target
            .filter(|h| !h.is_invalid())
            .unwrap_or_else(|| focus::focus_window(foreground));
        let (kind, es_password) = focus::inspect_window(target);
        // SAFETY: a plain query on window handles.
        self.focus
            .seen
            .forget_gone(|hwnd| unsafe { IsWindow(HWND(hwnd as _)) }.as_bool());
        let Some(generation) = self.focus.refresh(target.0 as isize, kind, es_password) else {
            self.word_start = None;
            return;
        };
        // Asked after the reset that goes with this refresh, so the answer is about the caret
        // the first word will be typed at.
        self.word_start = self
            .corrector
            .reset_generation()
            .map(|reset| (generation, reset));
        if let Some(probe) = &self.probe {
            if probe.request(generation, probe_delay, foreground) {
                return;
            }
        }
        self.word_start = None;
        self.probe = None;
        self.focus.uia_available = false;
        let _ = self.events.send(HookEvent::Warning(
            "UI Automation worker stopped; every field except classic edits now counts as secure"
                .into(),
        ));
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::*;

    #[test]
    fn interference_backoff_resets_after_a_healthy_minute() {
        let mut backoff = DegradeBackoff::default();
        let start = Instant::now();
        assert_eq!(backoff.stalled(start), FIRST_DEGRADE_BACKOFF);
        let mut recovered = start + FIRST_DEGRADE_BACKOFF;
        for seconds in [60, 120, 240, 480, 960, 1800, 1800] {
            let stall = recovered + DEGRADE_HEALTHY_RESET - Duration::from_millis(1);
            backoff.recovered_at = Some(recovered);
            let wait = backoff.stalled(stall);
            assert_eq!(wait, Duration::from_secs(seconds));
            recovered = stall + wait;
        }
        backoff.recovered_at = Some(recovered);
        assert_eq!(
            backoff.stalled(recovered + DEGRADE_HEALTHY_RESET),
            FIRST_DEGRADE_BACKOFF
        );
    }

    #[test]
    fn a_held_backspace_survives_the_round_trip_for_the_panic_hook() {
        for ev in [
            RawEvent {
                vk: 0x08,
                scan: 0x0E,
                extended: false,
                up: false,
                foreign: false,
            },
            RawEvent {
                vk: 0x08,
                scan: 0xFFFF,
                extended: true,
                up: false,
                foreign: true,
            },
        ] {
            assert_eq!(decode_held(encode_held(ev)), Some(ev));
        }
        assert_eq!(decode_held(0), None);
    }

    fn batch(at: Instant) -> Batch {
        Batch {
            batch: 1,
            replacement: Replacement {
                delete: 3,
                insert: "the ".into(),
                original: "teh".into(),
                corrected: "the".into(),
                source: unflub_core::Source::Bundled,
            },
            undo: false,
            app: None,
            took: Duration::ZERO,
            swallowed_at: at,
            handed_over: at,
            timing: InjectTiming::default(),
            inject_reported: false,
            released: false,
        }
    }

    /// #139: the echoes finish the batch before the injector's report arrives, so the report
    /// can't be what fills in the inject time.
    #[test]
    fn inject_time_is_set_when_the_echoes_finish_first() {
        let at = Instant::now();
        let mut b = batch(at);
        b.finish_timing(at + Duration::from_micros(900));
        assert_eq!(b.timing.inject, Duration::from_micros(900));
        assert_eq!(b.timing.end_to_end, Duration::from_micros(900));

        // The report came first (a partial send, or a slow echo): it is kept.
        let mut b = batch(at);
        b.timing.inject = Duration::from_micros(400);
        b.inject_reported = true;
        b.finish_timing(at + Duration::from_micros(900));
        assert_eq!(b.timing.inject, Duration::from_micros(400));
        assert_eq!(b.timing.end_to_end, Duration::from_micros(900));
    }

    /// #143: a reinstall whose keyboard hook failed is not a reinstall.
    #[test]
    fn a_failed_keyboard_reinstall_is_reported_as_lost() {
        assert!(matches!(
            reinstall_event(REINSTALL_RESUME, Ok(())),
            HookEvent::HooksReinstalled { reason: "resume" }
        ));
        match reinstall_event(REINSTALL_WATCHDOG, Err("access denied".into())) {
            HookEvent::KeyboardHookLost { error } => assert_eq!(error, "access denied"),
            other => panic!("{other:?}"),
        }
    }

    /// #135: input our hooks can't see doesn't count as missed.
    #[test]
    fn input_to_higher_integrity_windows_is_excused() {
        const LOW: u32 = 0x1000;
        const MEDIUM: u32 = 0x2000;
        const HIGH: u32 = 0x3000;
        const SYSTEM: u32 = 0x4000;
        assert!(
            hidden_by_integrity(Some(HIGH), Some(MEDIUM)),
            "elevated app"
        );
        assert!(hidden_by_integrity(Some(SYSTEM), Some(HIGH)));
        assert!(hidden_by_integrity(None, Some(MEDIUM)), "can't query it");
        assert!(!hidden_by_integrity(Some(MEDIUM), Some(MEDIUM)));
        assert!(
            !hidden_by_integrity(Some(LOW), Some(MEDIUM)),
            "sandboxed app"
        );
        assert!(
            !hidden_by_integrity(Some(HIGH), Some(HIGH)),
            "we run elevated too"
        );
        assert!(
            !hidden_by_integrity(Some(HIGH), None),
            "own level unknown: keep checking"
        );
        let own = integrity::own();
        assert!(own.is_some_and(|l| l >= LOW), "{own:?}");

        // An excuse moves the baseline forward, never back, across the tick wrap too.
        assert_eq!(newest(Some(10_000), None), 10_000);
        assert_eq!(newest(Some(10_000), Some(30_000)), 30_000);
        assert_eq!(newest(Some(30_000), Some(10_000)), 30_000);
        assert_eq!(newest(Some(u32::MAX - 5), Some(3)), 3);
        assert_eq!(newest(None, None), 0);
        // Typed into an elevated window from 10 s to 20 s, excused at the tick: not missed.
        assert!(!hooks_missed_input(
            20_000,
            newest(Some(10_000), Some(20_000))
        ));
        // Input our hooks missed after the excuse still counts.
        assert!(hooks_missed_input(
            25_000,
            newest(Some(10_000), Some(20_000))
        ));
    }

    #[test]
    fn missed_input_needs_a_gap() {
        assert!(!hooks_missed_input(10_000, 10_000));
        assert!(
            !hooks_missed_input(11_000, 10_000),
            "under the grace period"
        );
        assert!(hooks_missed_input(12_500, 10_000));
        assert!(
            !hooks_missed_input(9_000, 10_000),
            "input before our last callback"
        );
        assert!(!hooks_missed_input(0, 10_000), "unknown input time");
        // Across the 49.7-day tick wrap.
        assert!(hooks_missed_input(1_000, u32::MAX - 5_000));
        assert!(!hooks_missed_input(1_000, u32::MAX - 500));
    }

    #[test]
    fn watchdog_backs_off_while_input_keeps_being_missed() {
        let mut w = Watchdog::new(0);
        assert!(!w.should_reinstall(5_000, false, true));
        assert!(
            w.should_reinstall(10_000, true, true),
            "first miss: at once"
        );
        assert!(!w.should_reinstall(15_000, true, false));
        assert!(w.should_reinstall(40_000, true, false), "after 30 s");
        assert!(!w.should_reinstall(80_000, true, false), "then 60 s");
        assert!(w.should_reinstall(100_000, true, false));
        // A real callback after the reinstall, and no missed input: back to normal.
        assert!(!w.should_reinstall(105_000, false, true));
        assert!(w.should_reinstall(230_000, true, false));
        assert!(!w.should_reinstall(240_000, true, false));
        assert!(w.should_reinstall(260_000, true, false));
        // The pause never exceeds ten minutes.
        let mut w = Watchdog::new(0);
        let mut now = 0u32;
        for _ in 0..20 {
            while !w.should_reinstall(now, true, false) {
                now += 1_000;
            }
        }
        assert_eq!(w.backoff_ms, Watchdog::MAX_BACKOFF_MS);
    }

    /// The bug this guards: every reinstall marked a callback, so the next quiet tick (no input
    /// at all, so nothing missed) reset the pause, and a hook Windows keeps dropping was
    /// reinstalled every 30 s forever.
    #[test]
    fn a_quiet_tick_after_a_reinstall_keeps_the_backoff() {
        let mut w = Watchdog::new(0);
        assert!(w.should_reinstall(10_000, true, true));
        // Quiet: no input since the reinstall, so nothing was missed, but no callback either.
        assert!(!w.should_reinstall(15_000, false, false));
        assert!(!w.should_reinstall(20_000, false, false));
        assert!(w.should_reinstall(40_000, true, false), "30 s pause");
        assert!(!w.should_reinstall(45_000, false, false));
        assert!(!w.should_reinstall(80_000, true, false), "then 60 s");
        assert!(w.should_reinstall(100_000, true, false));
        // Reset by the quiet ticks, this would be back to 60 s.
        assert_eq!(w.backoff_ms, 8 * Watchdog::FIRST_BACKOFF_MS);
    }

    #[test]
    fn one_rearm_per_resume() {
        let mut g = ResumeGate::default();
        // No suspend reported yet: the first resume re-arms, its twin doesn't.
        assert!(g.on_resume(1_000));
        assert!(!g.on_resume(2_000), "PBT_APMRESUMESUSPEND after AUTOMATIC");
        // With suspends reported, each resume after one re-arms once, however late the twin.
        g.on_suspend();
        assert!(g.on_resume(10_000_000));
        assert!(!g.on_resume(10_300_000), "user arrived five minutes later");
        g.on_suspend();
        assert!(g.on_resume(20_000_000));
        // Without suspend notifications: at most one a minute.
        let mut g = ResumeGate::default();
        assert!(g.on_resume(u32::MAX - 10_000));
        assert!(!g.on_resume(u32::MAX));
        assert!(g.on_resume(60_000), "across the tick wrap");
    }
}
