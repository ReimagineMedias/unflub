//! What we know about the focused field: the foreground app's exe name and whether the field
//! is a password box. Refreshed on focus changes and on clicks, Tab and Enter; never per key.
//!
//! The focused window's class decides how we check for a password field:
//! - Edit controls (any class containing "edit": Edit, RichEdit*, WinForms EDIT, Delphi TEdit):
//!   the `ES_PASSWORD` style bit, read on the hook thread. Style and class reads don't send
//!   messages, so they can't block.
//! - Everything else (browsers, Electron, WPF, UWP, Qt, Java, unknown classes): UI Automation's
//!   `IsPassword`, on a worker thread with short UIA timeouts, because UIA calls can block on
//!   hung apps. Any accessibility query makes Chromium build its accessibility tree, which
//!   costs CPU and memory in Chrome and Electron apps, so we only ask when focus may have moved.
//!
//! Each check bumps a generation number; the worker publishes `(generation, answer)` in one
//! atomic. The answer is three-state, and everything except a definite "not a password" counts
//! as secure: no answer yet, a UIA error or timeout, or no UIA at all (fail closed).

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
    /// Edit control: `ES_PASSWORD` decides.
    Win32Edit,
    /// Anything else, including unknown classes: ask UIA, and stay secure until it says no.
    Probe,
}

/// Classify a window class name. Unknown classes are probed (fail closed).
pub fn classify_class(class: &str) -> FieldKind {
    if class.to_ascii_lowercase().contains("edit") {
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
pub fn is_password_edit(class: &str, style: u32) -> bool {
    style & ES_PASSWORD != 0 && classify_class(class) == FieldKind::Win32Edit
}

/// The UIA worker's answer about one focused element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secure {
    NotSecure = 0,
    Secure = 1,
    /// UIA failed or timed out. Treated as secure.
    Unknown = 2,
}

/// Shared slot the UIA worker writes and the hook thread reads: `generation << 2 | answer`.
#[derive(Debug, Default)]
pub struct PasswordSlot(AtomicU64);

impl PasswordSlot {
    pub fn publish(&self, generation: u64, answer: Secure) {
        self.0
            .store(generation << 2 | answer as u64, Ordering::Release);
    }

    /// The answer for `generation`, or `None` while it is pending.
    pub fn answer(&self, generation: u64) -> Option<Secure> {
        let v = self.0.load(Ordering::Acquire);
        (v >> 2 == generation).then_some(match v & 3 {
            0 => Secure::NotSecure,
            1 => Secure::Secure,
            _ => Secure::Unknown,
        })
    }
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
        }
    }
}

impl FocusState {
    pub fn secure(&self) -> bool {
        if self.es_password {
            return true;
        }
        match self.kind {
            FieldKind::Win32Edit => false,
            FieldKind::Probe => {
                !self.uia_available || self.slot.answer(self.generation) != Some(Secure::NotSecure)
            }
        }
    }

    /// Start a new generation; returns it when the worker should be asked about it.
    pub fn next_generation(&mut self) -> Option<u64> {
        self.generation += 1;
        (self.kind == FieldKind::Probe && self.uia_available).then_some(self.generation)
    }
}

#[cfg(windows)]
pub use win::{focus_window, foreground, inspect_window, process_exe, PasswordProbe};

#[cfg(windows)]
mod win {
    use super::{
        classify_class, exe_name_from_path, is_password_edit, FieldKind, PasswordSlot, Secure,
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
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2,
    };
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
            let password = kind == FieldKind::Win32Edit
                && is_password_edit(&class, GetWindowLongW(hwnd, GWL_STYLE) as u32);
            (kind, password)
        }
    }

    struct Request {
        generation: u64,
        due: Instant,
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
        pub fn request(&self, generation: u64, delay: Duration) -> bool {
            let due = Instant::now() + delay;
            self.requests.send(Request { generation, due }).is_ok()
        }
    }

    fn create_uia() -> Result<(IUIAutomation, Vec<String>), String> {
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

    fn serve(uia: &IUIAutomation, rx: &Receiver<Request>, slot: &PasswordSlot) {
        while let Ok(mut req) = rx.recv() {
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
            // SAFETY: valid interface used on the MTA thread that created it.
            let answer = unsafe {
                match uia.GetFocusedElement().and_then(|e| e.CurrentIsPassword()) {
                    Ok(b) if b.as_bool() => Secure::Secure,
                    Ok(_) => Secure::NotSecure,
                    Err(_) => Secure::Unknown,
                }
            };
            slot.publish(req.generation, answer);
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
            "RichEdit20W",
            "RICHEDIT50W",
            "RichEditD2DPT",
            "WindowsForms10.EDIT.app.0.141b42a_r6_ad1",
            "TEdit",
            "TMaskEdit",
        ] {
            assert_eq!(classify_class(c), FieldKind::Win32Edit, "{c}");
        }
        for c in [
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
    fn win32_edits_use_es_password_only() {
        let mut f = FocusState {
            kind: FieldKind::Win32Edit,
            uia_available: true,
            ..FocusState::default()
        };
        assert_eq!(f.next_generation(), None, "no UIA query for edits");
        assert!(!f.secure());
        f.es_password = true;
        assert!(f.secure());
    }
}
