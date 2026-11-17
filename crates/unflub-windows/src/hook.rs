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
//!
//! Windows removes a low-level hook whose callback is too slow (`LowLevelHooksTimeout`, at most
//! 1 s on Windows 10+), and every millisecond here is input lag for the whole desktop, so the
//! callback does no I/O: corrections are reported through a channel and printed elsewhere.
//!
//! Limitation: a non-elevated process doesn't see keys typed into elevated (administrator)
//! windows, and can't inject into them (UIPI). unflub simply does nothing there.

use std::cell::{Cell, RefCell};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Once;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use unflub_core::{Action, Context, Key, Replacement};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, ToUnicodeEx, HKL, VK_CAPITAL, VK_CONTROL,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowThreadProcessId, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, EVENT_OBJECT_FOCUS,
    EVENT_SYSTEM_FOREGROUND, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_XBUTTONDOWN,
};

use crate::focus::{self, FocusState, PasswordProbe};
use crate::inject::{self, SendOutcome, INJECT_TAG};
use crate::policy::{injection_veto, is_ime_language, Gate, GateDecision, SwallowTracker};
use crate::stats::LatencyHistogram;
use crate::translate::{KeyTranslator, Modifiers, RawKey, Typed};
use crate::Corrector;

/// `ToUnicodeEx` flag: don't change keyboard state (Windows 10 1607+). Without it, translating
/// a key in our hook would eat the dead-key state the user's app needs for accents.
const TOUNICODE_NO_STATE_CHANGE: u32 = 0x4;

const HOOK_THREAD_NAME: &str = "unflub-hook";

/// After a click, Tab or Enter, wait this long before asking UIA about the focused field, so
/// the app has handled the input that may have moved focus. The field is secure meanwhile.
const INPUT_RECHECK_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Default)]
pub struct HookOptions {
    /// Lowercased exe names where keys are not fed to the corrector at all.
    pub excluded_apps: Vec<String>,
}

/// Reported from the hook thread; read it on another thread (printing is too slow for the hook).
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// A `Replace` was injected. `took` is the whole hook callback, injection included.
    Corrected {
        replacement: Replacement,
        app: Option<String>,
        took: Duration,
    },
    /// An `Undo` was injected.
    Undone {
        replacement: Replacement,
        app: Option<String>,
        took: Duration,
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
    CorrectorPanicked(String),
    /// Something degraded but the hook keeps running (e.g. UI Automation unavailable).
    Warning(String),
}

/// What the hook thread hands back when it stops.
#[derive(Debug, Default)]
pub struct HookReport {
    /// Duration of every keyboard callback that reached our code (our own injections excluded).
    pub keyboard: LatencyHistogram,
}

/// The running hook thread. Stops (and unhooks) on [`Hook::stop`] or drop.
pub struct Hook {
    thread_id: u32,
    thread: Option<JoinHandle<HookReport>>,
}

impl Hook {
    /// Start the hook thread and install the hooks. Returns once they are installed, or the
    /// error if installing failed.
    pub fn start<C: Corrector + Send + 'static>(
        corrector: C,
        options: HookOptions,
    ) -> std::io::Result<(Hook, Receiver<HookEvent>)> {
        install_panic_hook();
        let (events_tx, events_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name(HOOK_THREAD_NAME.into())
            .spawn(move || run(Box::new(corrector), options, events_tx, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok((
                Hook {
                    thread_id,
                    thread: Some(thread),
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

/// The default panic hook writes to stderr and may capture a backtrace: far too slow inside a
/// keyboard hook. On the hook thread we print nothing; the panic is caught around the
/// corrector call and reported as [`HookEvent::CorrectorPanicked`]. Other threads keep the
/// previous hook.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() != Some(HOOK_THREAD_NAME) {
                previous(info);
            }
        }));
    });
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

struct State {
    corrector: Box<dyn Corrector>,
    translator: KeyTranslator,
    focus: FocusState,
    probe: Option<PasswordProbe>,
    excluded: Vec<String>,
    events: Sender<HookEvent>,
    histogram: LatencyHistogram,
    gate: Gate,
    swallow: SwallowTracker,
    broken: bool,
}

thread_local! {
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
        focus,
        probe,
        excluded: options
            .excluded_apps
            .iter()
            .map(|e| e.to_lowercase())
            .collect(),
        events,
        histogram: LatencyHistogram::new(),
        gate: Gate::default(),
        swallow: SwallowTracker::default(),
        broken: false,
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

    // SAFETY: standard message loop; hook callbacks are delivered inside GetMessageW.
    unsafe {
        while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
        hooks.uninstall();
    }
    take_report()
}

fn take_report() -> HookReport {
    let state = STATE.with(|s| s.borrow_mut().take());
    HookReport {
        keyboard: state.map(|s| s.histogram).unwrap_or_default(),
    }
}

struct Installed {
    keyboard: HHOOK,
    mouse: Option<HHOOK>,
    win_events: Vec<HWINEVENTHOOK>,
}

impl Installed {
    unsafe fn uninstall(self) {
        let _ = UnhookWindowsHookEx(self.keyboard);
        if let Some(m) = self.mouse {
            let _ = UnhookWindowsHookEx(m);
        }
        for h in self.win_events {
            let _ = UnhookWinEvent(h);
        }
    }
}

unsafe fn install() -> Result<Installed, String> {
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
        keyboard,
        mouse,
        win_events,
    })
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let start = Instant::now();
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT valid for this call.
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let injected = kb.flags.0 & LLKHF_INJECTED.0 != 0;
        if !(injected && kb.dwExtraInfo == INJECT_TAG) {
            let swallow = with_state(|s| {
                let swallow = s.on_keyboard(wparam.0 as u32, kb, injected, start);
                s.histogram.record(start.elapsed());
                swallow
            });
            if swallow == Some(true) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Called for every mouse move system-wide: bail out before touching any state.
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
    with_state(|s| {
        s.refresh_focus(target, Duration::ZERO);
        s.reset();
    });
}

/// Keyboard layout of the foreground window's thread.
fn foreground_layout() -> HKL {
    let (hwnd, _) = focus::foreground();
    // SAFETY: trivial queries.
    unsafe { GetKeyboardLayout(GetWindowThreadProcessId(hwnd, None)) }
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
    fn on_keyboard(
        &mut self,
        msg: u32,
        kb: &KBDLLHOOKSTRUCT,
        foreign_injected: bool,
        start: Instant,
    ) -> bool {
        let vk = kb.vkCode as u16;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        if !down {
            // Key-up of a key we swallowed: swallow it too so apps see a matched pair.
            return self.swallow.on_up(vk);
        }
        let repeat = self.swallow.on_down(vk, already_down(vk));
        if MISSED.with(Cell::take) {
            self.reset();
        }
        if self.broken {
            return false;
        }
        let (mods, key_state) = if foreign_injected {
            (Modifiers::default(), [0u8; 256])
        } else {
            read_keyboard()
        };
        let layout = foreground_layout();
        let raw = RawKey {
            vk,
            scan: kb.scanCode as u16,
            down,
            foreign_injected,
        };
        let key = self.translator.translate(raw, mods, || {
            to_unicode(raw.vk, raw.scan, &key_state, layout)
        });
        let swallow = match key {
            Some(key) => {
                let ime = is_ime_language((layout.0 as usize & 0xFFFF) as u16);
                self.handle_key(key, vk, mods, repeat, ime, start)
            }
            None => false,
        };
        // Tab and Enter can move focus or submit a form without any focus WinEvent (Chromium
        // without its accessibility tree): check the field again once the app has reacted.
        if vk == VK_TAB.0 || vk == VK_RETURN.0 {
            self.refresh_focus(None, INPUT_RECHECK_DELAY);
        }
        swallow
    }

    fn handle_key(
        &mut self,
        key: Key,
        vk: u16,
        mods: Modifiers,
        repeat: bool,
        ime: bool,
        start: Instant,
    ) -> bool {
        let blocked = self.focus.secure() || ime || self.excluded();
        match self.gate.decide(key, blocked) {
            GateDecision::Feed => {}
            GateDecision::ResetOnly => {
                self.reset_corrector();
                return false;
            }
            GateDecision::Drop => return false,
        }
        let Some(action) = self.call(key) else {
            return false;
        };
        let (replacement, undo) = match action {
            Action::Pass => return false,
            Action::Replace(r) => (r, false),
            Action::Undo(r) => (r, true),
        };
        if let Some(reason) = injection_veto(key, mods, repeat) {
            return self.skip(replacement, reason);
        }
        let inputs = inject::to_inputs(&inject::plan(replacement.delete, &replacement.insert));
        if let SendOutcome::Partial {
            sent,
            total,
            released,
        } = inject::send(&inputs)
        {
            // Let the user's key through; nothing more we can safely do about partial text.
            self.reset_corrector();
            let _ = self.events.send(HookEvent::InjectFailed {
                replacement,
                sent,
                total,
                released,
            });
            return false;
        }
        self.swallow.swallowed(vk);
        let app = self.focus.app.clone();
        let took = start.elapsed();
        let event = if undo {
            HookEvent::Undone {
                replacement,
                app,
                took,
            }
        } else {
            HookEvent::Corrected {
                replacement,
                app,
                took,
            }
        };
        let _ = self.events.send(event);
        true
    }

    fn excluded(&self) -> bool {
        self.focus
            .app
            .as_ref()
            .is_some_and(|app| self.excluded.iter().any(|e| e == app))
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
        (self.focus.kind, self.focus.es_password) = focus::inspect_window(target);
        let Some(generation) = self.focus.next_generation() else {
            return;
        };
        if let Some(probe) = &self.probe {
            if probe.request(generation, probe_delay) {
                return;
            }
        }
        self.probe = None;
        self.focus.uia_available = false;
        let _ = self.events.send(HookEvent::Warning(
            "UI Automation worker stopped; every field except classic edits now counts as secure"
                .into(),
        ));
    }
}
