//! What we know about the focused field: the foreground app's exe name and whether the field
//! is a password box. Refreshed on focus changes and on clicks, Tab and Enter; never per key.
//!
//! How we check for a password field:
//! - Any class containing "edit" with the `ES_PASSWORD` style bit: secure at once, read on the
//!   hook thread (style and class reads don't send messages, so they can't block), and
//!   remembered for that window while it exists ([`SeenPasswords`]): "Show password"
//!   checkboxes clear the bit with `EM_SETPASSWORDCHAR(0)`.
//! - Everything else, standard edits included: UI Automation, on a worker thread with short
//!   UIA timeouts because UIA calls can block on hung apps. Secure if `IsPassword` says so, if an
//!   edit box's name, automation id or help text says password or PIN ([`names_a_password`]),
//!   or if the element (by UIA runtime id) was a password field before under the same
//!   foreground window: a revealed password box keeps its runtime id. Any accessibility query
//!   makes Chromium build its accessibility tree, which costs CPU and memory in Chrome and
//!   Electron apps, so we only ask when focus may have moved.
//! - Without UIA, standard edits (Edit, RichEdit20A/W, RICHEDIT50W, RichEditD2D, RichEditD2DPT)
//!   go by `ES_PASSWORD` alone and everything else is secure.
//!
//! Each check bumps a generation number; the worker publishes `(generation, answer)` in one
//! atomic. The answer is three-state, and everything except a definite "not a password" counts
//! as secure: no answer yet, a UIA error or timeout, or no UIA at all (fail closed). The one
//! exception: a standard edit UIA already cleared stays cleared while the answer for the same
//! window is pending (Enter or a click inside it), since its window is that one field.
//!
//! The same query also says whether the caret sits at a word start (empty field, or whitespace
//! before it, read through `TextPattern`), in a second atomic. After a click or focus change the
//! hook passes a "yes" to the engine so the first word typed there may be corrected
//! ([`unflub_core::Engine::caret_at_word_start`]); anything else changes nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lowercased file name from a full image path (`C:\Program Files\X\Code.EXE` -> `code.exe`).
pub fn exe_name_from_path(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

/// How to tell whether a window holds a password field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Standard Windows edit control: `ES_PASSWORD` alone decides.
    Win32Edit,
    /// Anything else, including unknown classes: ask UIA, and stay secure until it says no.
    Probe,
}

/// Classify a window class name. Unknown classes are probed (fail closed).
pub fn classify_class(class: &str) -> FieldKind {
    const STANDARD_EDITS: &[&str] = &[
        "edit",
        "richedit20a",
        "richedit20w",
        "richedit50w",
        "richeditd2d",
        "richeditd2dpt",
    ];
    if STANDARD_EDITS.contains(&class.to_ascii_lowercase().as_str()) {
        FieldKind::Win32Edit
    } else {
        FieldKind::Probe
    }
}

/// Should an `EVENT_OBJECT_FOCUS` with this `idObject` count as a focus change? Everything
/// except objects that are never text fields (Chromium and Firefox don't always use
/// `OBJID_CLIENT`, so we can't require it).
pub fn is_focus_change_object(id_object: i32) -> bool {
    const SYSMENU: i32 = -1;
    const TITLEBAR: i32 = -2;
    const MENU: i32 = -3;
    const SOUND: i32 = -11;
    // VSCROLL -5, HSCROLL -6, SIZEGRIP -7, CARET -8, CURSOR -9, ALERT -10.
    !matches!(id_object, SYSMENU | TITLEBAR | MENU | -10..=-5 | SOUND)
}

const ES_PASSWORD: u32 = 0x20;

/// True when a window of `class` with `style` is a classic password edit control.
/// Any class containing "edit" counts (standard edits and their wrappers such as WinForms and
/// Delphi `TEdit`); on other classes the bit means something else.
pub fn is_password_edit(class: &str, style: u32) -> bool {
    style & ES_PASSWORD != 0 && class.to_ascii_lowercase().contains("edit")
}

/// Does a field's accessible name, automation id or help text say it takes a password, PIN or
/// passphrase? For fields that show what is typed and so don't report `IsPassword`: a password
/// box after "Show password", a Win32 edit whose mask was taken off with
/// `EM_SETPASSWORDCHAR(0)`, a PIN box (#40). A false match only means no corrections there.
pub fn names_a_password(texts: &[&str]) -> bool {
    const ANYWHERE: &[&str] = &[
        "password",
        "passwort",
        "passphrase",
        "passcode",
        "passwd",
        "kennwort",
        "mot de passe",
        "contraseña",
        "contrasena",
        "senha",
        "wachtwoord",
        "lösenord",
        "losenord",
        "adgangskode",
        "salasana",
        "hasło",
        "haslo",
        "heslo",
        "jelszó",
        "parola d'accesso",
        "пароль",
        "パスワード",
        "暗証番号",
        "密码",
        "密碼",
        "비밀번호",
        "암호",
    ];
    // Whole words only: "pin" must not match "spinner" or "pinned".
    const WORDS: &[&str] = &["pin", "pwd", "pass", "otp", "cvv", "cvc"];
    texts.iter().any(|t| {
        let lower = t.to_lowercase();
        ANYWHERE.iter().any(|w| lower.contains(w))
            || t.split(|c: char| !c.is_alphanumeric())
                .flat_map(split_camel)
                .any(|w| WORDS.contains(&w.as_str()))
    })
}

/// Lowercased parts of a camel-case word: `newPin` -> `new`, `pin`; `userPWD` -> `user`, `pwd`.
fn split_camel(word: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut prev_lower = false;
    for c in word.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(String::new());
        }
        prev_lower = c.is_lowercase();
        out.last_mut()
            .expect("never empty")
            .extend(c.to_lowercase());
    }
    out
}

/// Fields seen as password fields, so they stay secure after their mask comes off (#40): a
/// "Show password" button turns `<input type=password>` into `type=text`, and a Win32 "Show
/// password" checkbox sends `EM_SETPASSWORDCHAR(0)`, which clears `ES_PASSWORD`. Each entry has
/// an owner window and is forgotten once that window is gone; the oldest go first past
/// [`SeenPasswords::CAPACITY`]. A key that turns up again on an unrelated field only makes that
/// field secure.
#[derive(Debug, Clone)]
pub struct SeenPasswords<K> {
    entries: Vec<(isize, K)>,
}

impl<K> Default for SeenPasswords<K> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: PartialEq> SeenPasswords<K> {
    pub const CAPACITY: usize = 64;

    pub fn remember(&mut self, owner: isize, key: K) {
        if self.contains(&key) {
            return;
        }
        if self.entries.len() >= Self::CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push((owner, key));
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.iter().any(|(_, k)| k == key)
    }

    /// Forget fields whose owner window no longer exists.
    pub fn forget_gone(&mut self, exists: impl Fn(isize) -> bool) {
        self.entries.retain(|(owner, _)| exists(*owner));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The UIA worker's answer from what it read about the focused element: a terminal input,
/// UIA's `IsPassword` (`None`: the read failed), a name that says password ([`names_a_password`]),
/// and whether this element was seen as a password field before ([`SeenPasswords`]).
pub fn password_answer(
    terminal: bool,
    is_password: Option<bool>,
    named: bool,
    seen_before: bool,
) -> Secure {
    match is_password {
        _ if terminal || named || seen_before => Secure::Secure,
        Some(true) => Secure::Secure,
        Some(false) => Secure::NotSecure,
        None => Secure::Unknown,
    }
}

/// After a UIA query failed or timed out (`Unknown`) `tries` times already, how long to wait
/// before asking again; `None` when the answer stands.
///
/// The first query on a Chromium page switches on its accessibility tree, which can take longer
/// than the 500 ms UIA transaction timeout. That `Unknown` used to stand for the whole
/// generation, so the field stayed secure until the next click, Tab, Enter or focus event: a
/// whole sentence typed right after focusing a contenteditable went uncorrected (#117).
pub fn retry_after(answer: Secure, tries: usize) -> Option<std::time::Duration> {
    const WAITS_MS: [u64; 2] = [250, 750];
    (answer == Secure::Unknown)
        .then(|| WAITS_MS.get(tries))
        .flatten()
        .map(|ms| std::time::Duration::from_millis(*ms))
}

/// The UIA worker's answer about one focused element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secure {
    NotSecure = 0,
    Secure = 1,
    /// UIA failed or timed out. Treated as secure.
    Unknown = 2,
}

/// Is the caret at a word start? The UIA worker's second answer about the focused element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordStart {
    Yes = 0,
    No = 1,
    /// No text pattern, a selection, a timeout: keep the conservative default.
    Unknown = 2,
}

/// Does a caret after `before` (the character before it, `None` at the start of the field)
/// start a new word? Whitespace and opening brackets or quotes do; a letter, a digit, `.`, `/`
/// or `_` may be part of a longer token (`foo.teh`), so they don't.
pub fn word_start_after(before: Option<char>) -> bool {
    before.is_none_or(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '(' | '[' | '{' | '"' | '\u{201C}' | '\u{2018}' | '\u{00AB}'
            )
    })
}

/// Shared slot the UIA worker writes and the hook thread reads: `generation << 2 | answer`,
/// for the password question and the word-start question.
#[derive(Debug, Default)]
pub struct PasswordSlot {
    secure: AtomicU64,
    word_start: AtomicU64,
}

impl PasswordSlot {
    pub fn publish(&self, generation: u64, answer: Secure) {
        self.secure
            .store(generation << 2 | answer as u64, Ordering::Release);
    }

    pub fn publish_word_start(&self, generation: u64, answer: WordStart) {
        self.word_start
            .store(generation << 2 | answer as u64, Ordering::Release);
    }

    /// The word-start answer for `generation`, or `None` while it is pending.
    pub fn word_start(&self, generation: u64) -> Option<WordStart> {
        let v = self.word_start.load(Ordering::Acquire);
        (v >> 2 == generation).then_some(match v & 3 {
            0 => WordStart::Yes,
            1 => WordStart::No,
            _ => WordStart::Unknown,
        })
    }

    /// The answer for `generation`, or `None` while it is pending.
    pub fn answer(&self, generation: u64) -> Option<Secure> {
        let v = self.secure.load(Ordering::Acquire);
        (v >> 2 == generation).then_some(match v & 3 {
            0 => Secure::NotSecure,
            1 => Secure::Secure,
            _ => Secure::Unknown,
        })
    }
}

/// Where the focused field stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldCheck {
    /// Not a password field: corrections allowed.
    Cleared,
    /// UIA hasn't answered yet: secure until it does.
    Pending,
    /// A password field, or one UIA couldn't clear: secure.
    Secure,
}

/// Hook-thread view of the focused field.
#[derive(Debug)]
pub struct FocusState {
    pub app: Option<String>,
    pub app_pid: u32,
    pub kind: FieldKind,
    pub es_password: bool,
    pub generation: u64,
    pub slot: Arc<PasswordSlot>,
    /// False when the UIA worker couldn't start or died. Non-edit fields then count as secure.
    pub uia_available: bool,
    /// The focused window (`HWND` as an integer) this is about.
    pub target: isize,
    /// `target` had `ES_PASSWORD` at some point ([`FocusState::seen`]).
    pub was_password: bool,
    /// Windows seen with `ES_PASSWORD`, owned by themselves (forgotten once destroyed).
    pub seen: SeenPasswords<isize>,
    /// The same standard edit as before, which UIA had already cleared: it stays cleared while
    /// the new answer is pending (an `HWND` of a standard edit is one field).
    carried_safe: bool,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            app: None,
            app_pid: 0,
            kind: FieldKind::Probe,
            es_password: false,
            generation: 0,
            slot: Arc::new(PasswordSlot::default()),
            uia_available: false,
            target: 0,
            was_password: false,
            seen: SeenPasswords::default(),
            carried_safe: false,
        }
    }
}

impl FocusState {
    /// Secure (no corrections, and the corrector sees nothing): a password field now or before,
    /// or anything UIA hasn't cleared. Fails closed.
    ///
    /// Standard edits used to count as cleared on `ES_PASSWORD` alone, which a "Show password"
    /// checkbox removes (#40); they now need UIA's answer too, which also checks the field's
    /// name, unless the same edit was cleared just before. Only without UIA does
    /// `ES_PASSWORD` alone decide for them.
    pub fn secure(&self) -> bool {
        self.check() != FieldCheck::Cleared
    }

    /// [`FocusState::secure`] in detail: secure for good (for this generation), or only until
    /// the UIA answer arrives.
    pub fn check(&self) -> FieldCheck {
        if self.es_password || self.was_password {
            return FieldCheck::Secure;
        }
        if !self.uia_available {
            return match self.kind {
                FieldKind::Win32Edit => FieldCheck::Cleared,
                FieldKind::Probe => FieldCheck::Secure,
            };
        }
        match self.slot.answer(self.generation) {
            Some(Secure::NotSecure) => FieldCheck::Cleared,
            Some(_) => FieldCheck::Secure,
            None if self.kind == FieldKind::Win32Edit && self.carried_safe => FieldCheck::Cleared,
            None => FieldCheck::Pending,
        }
    }

    /// Focus may have moved to `target` (an `HWND` as an integer) of `kind`, with `ES_PASSWORD`
    /// as read now. Remembers password edits, and starts a new generation: returns it when the
    /// UIA worker should be asked ([`FocusState::next_generation`]).
    pub fn refresh(&mut self, target: isize, kind: FieldKind, es_password: bool) -> Option<u64> {
        let same_cleared_edit = kind == FieldKind::Win32Edit
            && self.kind == FieldKind::Win32Edit
            && target == self.target
            && !self.secure();
        if es_password {
            self.seen.remember(target, target);
        }
        self.kind = kind;
        self.es_password = es_password;
        self.target = target;
        self.was_password = self.seen.contains(&target);
        self.carried_safe = same_cleared_edit;
        self.next_generation()
    }

    /// Start a new generation; returns it when the worker should be asked about it: always
    /// when UIA works (probed fields for the password answer, every field for the word-start
    /// answer).
    pub fn next_generation(&mut self) -> Option<u64> {
        self.generation += 1;
        self.uia_available.then_some(self.generation)
    }
}

#[cfg(windows)]
pub(crate) use win::create_uia;
#[cfg(windows)]
pub use win::{focus_window, foreground, inspect_window, process_exe, PasswordProbe};

#[cfg(windows)]
mod win {
    use super::{
        classify_class, exe_name_from_path, is_password_edit, names_a_password, FieldKind,
        PasswordSlot, Secure, SeenPasswords, WordStart,
    };
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::core::{Interface, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
        SafeArrayUnaccessData,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::UIA_EditControlTypeId;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement,
        IUIAutomationTextPattern, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
        TextUnit_Character, UIA_TextPatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongW,
        GetWindowThreadProcessId, GUITHREADINFO, GWL_STYLE,
    };

    /// UIA connection and transaction timeouts. Short: a stuck app should not keep the field
    /// "pending" for long. Defaults are 2 s and 20 s.
    const UIA_CONNECTION_TIMEOUT_MS: u32 = 250;
    const UIA_TRANSACTION_TIMEOUT_MS: u32 = 500;

    /// Foreground window and its process id.
    pub fn foreground() -> (HWND, u32) {
        // SAFETY: plain Win32 queries with a valid out-pointer.
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            (hwnd, pid)
        }
    }

    /// Lowercased exe name of `pid`, or `None` if the process can't be opened (protected or
    /// elevated processes).
    pub fn process_exe(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        // SAFETY: handle is closed below; buffer and size describe the same allocation.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(handle);
            ok.ok()?;
            Some(exe_name_from_path(&String::from_utf16_lossy(
                &buf[..len as usize],
            )))
        }
    }

    /// The window with keyboard focus in `foreground`'s thread, or `foreground` itself.
    pub fn focus_window(foreground: HWND) -> HWND {
        // SAFETY: plain Win32 queries with valid out-pointers; none of them send messages.
        unsafe {
            let tid = GetWindowThreadProcessId(foreground, None);
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if tid != 0 && GetGUIThreadInfo(tid, &mut info).is_ok() && !info.hwndFocus.is_invalid()
            {
                info.hwndFocus
            } else {
                foreground
            }
        }
    }

    /// Classify `hwnd` by class name, and read `ES_PASSWORD` for classic edits.
    pub fn inspect_window(hwnd: HWND) -> (FieldKind, bool) {
        // SAFETY: class and style reads; neither sends messages to the window.
        unsafe {
            let mut class = [0u16; 128];
            let n = GetClassNameW(hwnd, &mut class);
            if n <= 0 {
                return (FieldKind::Probe, false);
            }
            let class = String::from_utf16_lossy(&class[..n as usize]);
            let kind = classify_class(&class);
            let password = is_password_edit(&class, GetWindowLongW(hwnd, GWL_STYLE) as u32);
            (kind, password)
        }
    }

    struct Request {
        generation: u64,
        due: Instant,
        /// The foreground window: password fields seen under it are remembered until it is
        /// gone.
        owner: isize,
    }

    /// Handle to the UIA worker thread. Dropping it stops the worker after its current query.
    pub struct PasswordProbe {
        requests: Sender<Request>,
    }

    impl PasswordProbe {
        /// Start the worker and wait until UIA is ready. It initialises COM as MTA on its own
        /// thread (the UIA client threading model Microsoft recommends) and owns the
        /// `IUIAutomation` object. Returns the probe plus any warnings, or why UIA is unusable.
        pub fn start(slot: Arc<PasswordSlot>) -> Result<(Self, Vec<String>), String> {
            let (tx, rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("unflub-uia".into())
                .spawn(move || worker(rx, slot, ready_tx))
                .map_err(|e| format!("can't start the UI Automation thread: {e}"))?;
            match ready_rx.recv() {
                Ok(Ok(warnings)) => Ok((Self { requests: tx }, warnings)),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("UI Automation thread exited during startup".into()),
            }
        }

        /// Ask about the element focused `delay` from now (after the app has handled the click
        /// or key that may move focus). False if the worker is gone.
        pub fn request(&self, generation: u64, delay: Duration, owner: HWND) -> bool {
            let due = Instant::now() + delay;
            let owner = owner.0 as isize;
            self.requests
                .send(Request {
                    generation,
                    due,
                    owner,
                })
                .is_ok()
        }
    }

    /// A UIA client with short timeouts, for an MTA thread that owns it.
    pub(crate) fn create_uia() -> Result<(IUIAutomation, Vec<String>), String> {
        // SAFETY: COM is initialised on this thread. CUIAutomation8 implements IUIAutomation2
        // (timeouts); plain CUIAutomation doesn't, so it is only the fallback.
        let uia: IUIAutomation = unsafe {
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
        }
        .map_err(|e| format!("UI Automation unavailable ({e})"))?;
        let timeouts = uia.cast::<IUIAutomation2>().and_then(|uia2| {
            // SAFETY: valid interface on its own MTA thread.
            unsafe {
                uia2.SetConnectionTimeout(UIA_CONNECTION_TIMEOUT_MS)?;
                uia2.SetTransactionTimeout(UIA_TRANSACTION_TIMEOUT_MS)
            }
        });
        let warnings = match timeouts {
            Ok(()) => Vec::new(),
            Err(e) => vec![format!(
                "UI Automation timeouts not set ({e}); a hung app can keep its fields \
                 treated as secure for up to 20 s"
            )],
        };
        Ok((uia, warnings))
    }

    type Ready = mpsc::SyncSender<Result<Vec<String>, String>>;

    /// Is the caret at a word start in `element`? Through `TextPattern`: the selection must be
    /// one empty range (a caret), and the character before it whitespace or nothing.
    fn caret_word_start(element: &IUIAutomationElement) -> WordStart {
        // SAFETY: valid interfaces on the worker's MTA thread, under the UIA timeouts.
        let found = unsafe {
            (|| -> windows::core::Result<Option<bool>> {
                let pattern: IUIAutomationTextPattern =
                    element.GetCurrentPatternAs(UIA_TextPatternId)?;
                let selection = pattern.GetSelection()?;
                if selection.Length()? != 1 {
                    return Ok(None);
                }
                let caret = selection.GetElement(0)?;
                let empty = caret.CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &caret,
                    TextPatternRangeEndpoint_End,
                )? == 0;
                if !empty {
                    return Ok(None);
                }
                let before = caret.Clone()?;
                let moved = before.MoveEndpointByUnit(
                    TextPatternRangeEndpoint_Start,
                    TextUnit_Character,
                    -1,
                )?;
                if moved == 0 {
                    return Ok(Some(true));
                }
                let text = before.GetText(4)?.to_string();
                Ok(Some(crate::focus::word_start_after(text.chars().next())))
            })()
        };
        match found {
            Ok(Some(true)) => WordStart::Yes,
            Ok(Some(false)) => WordStart::No,
            _ => WordStart::Unknown,
        }
    }

    fn worker(rx: Receiver<Request>, slot: Arc<PasswordSlot>, ready: Ready) {
        // SAFETY: balanced by CoUninitialize below; this thread never enters an STA.
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if let Err(e) = init.ok() {
            let _ = ready.send(Err(format!("COM init for UI Automation failed ({e})")));
            return;
        }
        match create_uia() {
            Ok((uia, warnings)) => {
                let _ = ready.send(Ok(warnings));
                serve(&uia, &rx, &slot);
            }
            Err(e) => {
                let _ = ready.send(Err(e));
            }
        }
        // SAFETY: matches the successful CoInitializeEx on this thread; `uia` is dropped.
        unsafe { CoUninitialize() };
    }

    /// The element's UIA runtime id: stable for one field while it exists, also across a
    /// "Show password" toggle (Chromium keeps the node, a Win32 edit keeps its `HWND`).
    unsafe fn runtime_id(element: &IUIAutomationElement) -> Option<Vec<i32>> {
        let array = element.GetRuntimeId().ok()?;
        if array.is_null() {
            return None;
        }
        let id = (|| {
            let low = SafeArrayGetLBound(array, 1).ok()?;
            let high = SafeArrayGetUBound(array, 1).ok()?;
            let len = usize::try_from(high - low + 1).ok().filter(|n| *n > 0)?;
            let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).ok()?;
            let id = std::slice::from_raw_parts(data as *const i32, len).to_vec();
            let _ = SafeArrayUnaccessData(array);
            Some(id)
        })();
        let _ = SafeArrayDestroy(array);
        id
    }

    /// Is the focused `element` a password field, now or before (`seen`, filled here)?
    unsafe fn password_field(
        element: &IUIAutomationElement,
        owner: isize,
        seen: &mut SeenPasswords<Vec<i32>>,
    ) -> Secure {
        // A terminal inside an ordinary app (VS Code's xterm.js) may be at a password prompt
        // we can't see: secure too.
        let terminal = element
            .CurrentClassName()
            .is_ok_and(|c| crate::policy::is_terminal_input_class(&c.to_string()));
        let is_password = element.CurrentIsPassword().ok().map(|b| b.as_bool());
        // Names are only read on edit boxes: a document's name is its title.
        let named = element.CurrentControlType().ok() == Some(UIA_EditControlTypeId) && {
            let texts: Vec<String> = [
                element.CurrentName(),
                element.CurrentAutomationId(),
                element.CurrentHelpText(),
            ]
            .into_iter()
            .filter_map(|t| t.ok().map(|t| t.to_string()))
            .collect();
            names_a_password(&texts.iter().map(String::as_str).collect::<Vec<_>>())
        };
        let id = runtime_id(element);
        let seen_before = id.as_ref().is_some_and(|id| seen.contains(id));
        if is_password == Some(true) || named {
            if let Some(id) = id {
                seen.remember(owner, id);
            }
        }
        super::password_answer(terminal, is_password, named, seen_before)
    }

    fn serve(uia: &IUIAutomation, rx: &Receiver<Request>, slot: &PasswordSlot) {
        let mut seen = SeenPasswords::<Vec<i32>>::default();
        // A newer request that arrived while an answer was being retried.
        let mut next: Option<Request> = None;
        loop {
            let mut req = match next.take() {
                Some(r) => r,
                None => match rx.recv() {
                    Ok(r) => r,
                    Err(_) => return,
                },
            };
            // Coalesce to the latest request and wait until it is due.
            loop {
                match rx.try_recv() {
                    Ok(r) => {
                        req = r;
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => return,
                    Err(TryRecvError::Empty) => {}
                }
                let now = Instant::now();
                if req.due <= now {
                    break;
                }
                match rx.recv_timeout(req.due - now) {
                    Ok(r) => req = r,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            // SAFETY: a plain query on window handles.
            seen.forget_gone(|owner| unsafe { IsWindow(HWND(owner as _)) }.as_bool());
            let mut ask = || {
                // SAFETY: valid interface used on the MTA thread that created it.
                let focused = unsafe { uia.GetFocusedElement() };
                // SAFETY: as above, for the element it returned.
                let answer = match &focused {
                    Ok(e) => unsafe { password_field(e, req.owner, &mut seen) },
                    Err(_) => Secure::Unknown,
                };
                (answer, focused)
            };
            let (mut answer, mut focused) = ask();
            // A failed or timed-out query is asked again before it counts (the field stays
            // pending, so secure, meanwhile), unless focus moves on first.
            let mut tries = 0;
            while let Some(wait) = super::retry_after(answer, tries) {
                match rx.recv_timeout(wait) {
                    Ok(r) => {
                        next = Some(r);
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        tries += 1;
                        (answer, focused) = ask();
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            if next.is_some() {
                // Superseded: the hook has moved on to a newer generation.
                continue;
            }
            slot.publish(req.generation, answer);
            // Only worth asking where the first word may be corrected at all.
            let word_start = match (answer, focused) {
                (Secure::NotSecure, Ok(e)) => caret_word_start(&e),
                _ => WordStart::Unknown,
            };
            slot.publish_word_start(req.generation, word_start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates the UIA client and sets its timeouts, without querying any app (a query would
    /// touch whatever the user has focused).
    #[cfg(windows)]
    #[test]
    fn uia_worker_starts_with_timeouts() {
        let slot = Arc::new(PasswordSlot::default());
        let (probe, warnings) = PasswordProbe::start(slot).expect("UI Automation available");
        assert!(warnings.is_empty(), "{warnings:?}");
        drop(probe);
    }

    #[test]
    fn word_starts() {
        for c in [
            None,
            Some(' '),
            Some('\n'),
            Some('\t'),
            Some('('),
            Some('"'),
        ] {
            assert!(word_start_after(c), "{c:?}");
        }
        for c in ['a', 'Z', '7', '.', '/', '_', '-', '\''] {
            assert!(!word_start_after(Some(c)), "{c:?}");
        }
    }

    #[test]
    fn word_start_answers_are_per_generation() {
        let slot = PasswordSlot::default();
        assert_eq!(slot.word_start(1), None);
        slot.publish_word_start(1, WordStart::Yes);
        assert_eq!(slot.word_start(1), Some(WordStart::Yes));
        assert_eq!(slot.word_start(2), None, "pending for a newer generation");
        slot.publish_word_start(2, WordStart::Unknown);
        assert_eq!(slot.word_start(1), None, "stale");
        assert_eq!(slot.word_start(2), Some(WordStart::Unknown));
        // The two answers don't share bits.
        slot.publish(2, Secure::NotSecure);
        assert_eq!(slot.answer(2), Some(Secure::NotSecure));
        assert_eq!(slot.word_start(2), Some(WordStart::Unknown));
    }

    #[test]
    fn exe_names_are_lowercased_file_names() {
        assert_eq!(
            exe_name_from_path(r"C:\Program Files\Microsoft VS Code\Code.EXE"),
            "code.exe"
        );
        assert_eq!(exe_name_from_path("notepad.exe"), "notepad.exe");
        assert_eq!(exe_name_from_path("/usr/bin/Foo"), "foo");
    }

    #[test]
    fn class_names_choose_how_to_check() {
        for c in [
            "Edit",
            "EDIT",
            "RichEdit20A",
            "RichEdit20W",
            "RICHEDIT50W",
            "RichEditD2D",
            "RichEditD2DPT",
        ] {
            assert_eq!(classify_class(c), FieldKind::Win32Edit, "{c}");
        }
        for c in [
            // Edit wrappers and custom edits: ES_PASSWORD can make them secure, but its
            // absence proves nothing, so UIA is asked too.
            "WindowsForms10.EDIT.app.0.141b42a_r6_ad1",
            "TEdit",
            "TMaskEdit",
            "MyCustomEditor",
            "Chrome_RenderWidgetHostHWND",
            "MozillaWindowClass",
            "HwndWrapper[DefaultDomain;;b5f6]",
            "Windows.UI.Core.CoreWindow",
            "DesktopWindowXamlSource",
            // Unknown or custom-drawn: fail closed and ask UIA.
            "Qt5152QWindowIcon",
            "SunAwtFrame",
            "ConsoleWindowClass",
            "Notepad",
            "",
        ] {
            assert_eq!(classify_class(c), FieldKind::Probe, "{c}");
        }
    }

    #[test]
    fn focus_events_count_unless_the_object_is_never_text() {
        // OBJID_WINDOW, OBJID_CLIENT, and Chromium/Firefox child ids.
        for id in [0, -4, 1, 42, -12, i32::MIN] {
            assert!(is_focus_change_object(id), "{id}");
        }
        // SYSMENU, TITLEBAR, MENU, VSCROLL, HSCROLL, SIZEGRIP, CARET, CURSOR, ALERT, SOUND.
        for id in [-1, -2, -3, -5, -6, -7, -8, -9, -10, -11] {
            assert!(!is_focus_change_object(id), "{id}");
        }
    }

    #[test]
    fn es_password_only_counts_on_edit_classes() {
        assert!(is_password_edit("Edit", 0x50010020));
        assert!(is_password_edit(
            "WindowsForms10.EDIT.app.0.141b42a_r6_ad1",
            0x20
        ));
        assert!(is_password_edit("RICHEDIT50W", 0x20));
        assert!(is_password_edit("TEdit", 0x20));
        assert!(is_password_edit("MyCustomEditor", 0x20));
        assert!(!is_password_edit("Edit", 0x50010000));
        // 0x20 on a button is BS_... something else entirely.
        assert!(!is_password_edit("Button", 0x20));
        assert!(!is_password_edit("Chrome_RenderWidgetHostHWND", 0x20));
    }

    #[test]
    fn slot_is_three_state_and_per_generation() {
        let slot = PasswordSlot::default();
        assert_eq!(slot.answer(1), None);
        for a in [Secure::NotSecure, Secure::Secure, Secure::Unknown] {
            slot.publish(7, a);
            assert_eq!(slot.answer(7), Some(a));
            assert_eq!(slot.answer(8), None);
        }
    }

    fn probe_field() -> FocusState {
        FocusState {
            kind: FieldKind::Probe,
            uia_available: true,
            ..FocusState::default()
        }
    }

    #[test]
    fn probe_fields_are_secure_unless_definitely_not() {
        let mut f = probe_field();
        let g1 = f.next_generation().expect("probe requested");
        assert!(f.secure(), "pending: fail closed");
        f.slot.publish(g1, Secure::Unknown);
        assert!(f.secure(), "UIA error or timeout: fail closed");
        f.slot.publish(g1, Secure::Secure);
        assert!(f.secure(), "password field");
        f.slot.publish(g1, Secure::NotSecure);
        assert!(!f.secure());
        let g2 = f.next_generation().unwrap();
        assert!(f.secure(), "stale answer for g1 doesn't count for g2");
        f.next_generation();
        f.slot.publish(g2, Secure::NotSecure);
        assert!(f.secure(), "late answer for an old generation is ignored");
    }

    #[test]
    fn without_uia_probe_fields_are_secure() {
        let mut f = FocusState {
            uia_available: false,
            ..probe_field()
        };
        assert_eq!(f.next_generation(), None, "nothing to ask");
        assert!(f.secure());
    }

    #[test]
    fn unknown_classes_fail_closed() {
        let mut f = FocusState {
            kind: classify_class("Qt5152QWindowIcon"),
            uia_available: true,
            ..FocusState::default()
        };
        let g = f.next_generation().expect("unknown classes are probed");
        assert!(f.secure(), "secure until UIA answers");
        f.slot.publish(g, Secure::NotSecure);
        assert!(!f.secure());
        f.uia_available = false;
        assert!(f.secure(), "no UIA: secure");
        assert!(
            FocusState::default().secure(),
            "before the first refresh: secure"
        );
    }

    #[test]
    fn non_standard_edits_are_secure_on_es_password_and_probed_otherwise() {
        let mut f = FocusState {
            kind: classify_class("TEdit"),
            es_password: is_password_edit("TEdit", 0x20),
            uia_available: true,
            ..FocusState::default()
        };
        let g = f.next_generation().expect("probed");
        f.slot.publish(g, Secure::NotSecure);
        assert!(f.secure(), "ES_PASSWORD wins over UIA");
        f.es_password = is_password_edit("TEdit", 0);
        assert!(!f.secure(), "no ES_PASSWORD and UIA says not secure");
        f.next_generation();
        assert!(f.secure(), "no ES_PASSWORD: secure until UIA answers");
    }

    #[test]
    fn win32_edits_use_es_password_and_uia() {
        let mut f = FocusState {
            uia_available: true,
            ..FocusState::default()
        };
        let g = f.refresh(100, FieldKind::Win32Edit, false).expect("asked");
        assert!(f.secure(), "a new edit: secure until UIA answers (#40)");
        f.slot.publish(g, Secure::Secure);
        assert!(f.secure(), "UIA's answer counts: its name says password");
        f.slot.publish(g, Secure::NotSecure);
        assert!(!f.secure());
        // Enter or a click in the same edit: still cleared while the new answer is pending...
        let g = f.refresh(100, FieldKind::Win32Edit, false).unwrap();
        assert!(!f.secure(), "same edit, cleared before");
        // ...and a late "secure" still counts.
        f.slot.publish(g, Secure::Unknown);
        assert!(f.secure());
        // Another edit starts over.
        let g = f.refresh(100, FieldKind::Win32Edit, false).unwrap();
        assert!(f.secure(), "the last answer for this edit wasn't a clear");
        f.slot.publish(g, Secure::NotSecure);
        f.refresh(200, FieldKind::Win32Edit, false);
        assert!(f.secure(), "another edit: pending");
        // ES_PASSWORD decides at once.
        f.refresh(300, FieldKind::Win32Edit, true);
        assert!(f.secure());
        // Without UIA, ES_PASSWORD alone decides for standard edits.
        let mut f = FocusState::default();
        assert_eq!(f.refresh(100, FieldKind::Win32Edit, false), None);
        assert!(!f.secure());
        f.refresh(100, FieldKind::Win32Edit, true);
        assert!(f.secure());
    }

    /// #40: "Show password" takes the mask off (EM_SETPASSWORDCHAR(0) clears ES_PASSWORD);
    /// the edit stays secure while it exists.
    #[test]
    fn an_edit_that_was_a_password_box_stays_secure() {
        let mut f = FocusState {
            uia_available: true,
            ..FocusState::default()
        };
        f.refresh(100, FieldKind::Win32Edit, true);
        assert!(f.secure());
        let g = f.refresh(100, FieldKind::Win32Edit, false).unwrap();
        f.slot.publish(g, Secure::NotSecure);
        assert!(f.secure(), "unmasked, but it was a password box");
        assert!(f.was_password);
        // Another edit isn't affected.
        let g = f.refresh(101, FieldKind::Win32Edit, false).unwrap();
        f.slot.publish(g, Secure::NotSecure);
        assert!(!f.secure());
        // Once the window is gone it is forgotten (its handle may be reused).
        f.seen.forget_gone(|hwnd| hwnd != 100);
        let g = f.refresh(100, FieldKind::Win32Edit, false).unwrap();
        f.slot.publish(g, Secure::NotSecure);
        assert!(!f.secure());
    }

    /// #110: only a missing answer is pending; anything that isn't a clear is secure for good.
    #[test]
    fn pending_is_only_a_missing_answer() {
        let mut f = FocusState {
            uia_available: true,
            ..FocusState::default()
        };
        let g = f.refresh(7, FieldKind::Probe, false).unwrap();
        assert_eq!(f.check(), FieldCheck::Pending);
        f.slot.publish(g, Secure::NotSecure);
        assert_eq!(f.check(), FieldCheck::Cleared);
        let g = f.refresh(7, FieldKind::Probe, false).unwrap();
        assert_eq!(
            f.check(),
            FieldCheck::Pending,
            "one HWND, many fields: asked again"
        );
        f.slot.publish(g, Secure::Unknown);
        assert_eq!(f.check(), FieldCheck::Secure);
        f.refresh(8, FieldKind::Win32Edit, true);
        assert_eq!(f.check(), FieldCheck::Secure, "ES_PASSWORD never waits");
        f.uia_available = false;
        f.refresh(9, FieldKind::Probe, false);
        assert_eq!(f.check(), FieldCheck::Secure, "no UIA: nothing to wait for");
    }

    /// #117: a timed-out query is asked twice more before the field stays secure.
    #[test]
    fn unknown_answers_are_retried_twice() {
        use std::time::Duration;
        assert_eq!(
            retry_after(Secure::Unknown, 0),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            retry_after(Secure::Unknown, 1),
            Some(Duration::from_millis(750))
        );
        assert_eq!(retry_after(Secure::Unknown, 2), None);
        assert_eq!(retry_after(Secure::NotSecure, 0), None);
        assert_eq!(retry_after(Secure::Secure, 0), None);
    }

    #[test]
    fn seen_passwords_are_bounded_and_per_owner() {
        let mut seen = SeenPasswords::<Vec<i32>>::default();
        assert!(seen.is_empty());
        seen.remember(1, vec![42, 7]);
        seen.remember(1, vec![42, 7]);
        assert_eq!(seen.len(), 1, "once");
        assert!(seen.contains(&vec![42, 7]));
        assert!(!seen.contains(&vec![42, 8]));
        for i in 0..SeenPasswords::<Vec<i32>>::CAPACITY as i32 {
            seen.remember(2, vec![i]);
        }
        assert_eq!(seen.len(), SeenPasswords::<Vec<i32>>::CAPACITY);
        assert!(!seen.contains(&vec![42, 7]), "the oldest went first");
        seen.forget_gone(|owner| owner != 2);
        assert!(seen.is_empty());
    }

    /// #40: the worker's answer. Anything that says password wins; only a clean read clears.
    #[test]
    fn password_answers() {
        assert_eq!(
            password_answer(false, Some(false), false, false),
            Secure::NotSecure
        );
        assert_eq!(
            password_answer(false, Some(true), false, false),
            Secure::Secure
        );
        assert_eq!(
            password_answer(false, Some(false), true, false),
            Secure::Secure,
            "revealed, but named Password"
        );
        assert_eq!(
            password_answer(false, Some(false), false, true),
            Secure::Secure,
            "revealed, but it was masked before"
        );
        assert_eq!(
            password_answer(true, Some(false), false, false),
            Secure::Secure,
            "terminal"
        );
        assert_eq!(password_answer(false, None, false, false), Secure::Unknown);
    }

    #[test]
    fn names_that_say_password() {
        for names in [
            &["Password"][..],
            &["Enter password:"],
            &["", "loginPassword"],
            &["Confirm new passphrase"],
            &["", "", "Your PIN"],
            &["", "newPin"],
            &["", "user_pwd"],
            &["", "pass"],
            &["Mot de passe"],
            &["Kennwort"],
            &["Contraseña"],
            &["パスワード"],
            &["Card CVV"],
            &["Enter the OTP"],
        ] {
            assert!(names_a_password(names), "{names:?}");
        }
        for names in [
            &["Message"][..],
            &["Search", "searchbox", "Type to search"],
            &["Spinner value"],
            &["Pinned notes"],
            &["Compass heading"],
            &["Passenger name"],
            &["Shopping list"],
            &[],
        ] {
            assert!(!names_a_password(names), "{names:?}");
        }
    }
}
