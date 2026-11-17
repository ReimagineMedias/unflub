//! What we know about the focused field: the foreground app's exe name and whether the field
//! is a password box. Both are refreshed on focus changes, never per key.
//!
//! Password detection has two parts:
//! - `ES_PASSWORD` on a classic Win32 edit control, checked on the hook thread (window style
//!   reads don't send messages, so they can't block on a hung app).
//! - UI Automation's `IsPassword` for everything else (browsers, Electron, WPF, UWP). UIA calls
//!   can block on hung apps, so they run on a dedicated worker thread with short UIA timeouts.
//!   Each focus change bumps a generation number; the worker publishes `(generation, password)`
//!   in one atomic. Until the answer for the current generation arrives, the field counts as
//!   secure (fail closed): the engine skips a few keys at worst, and never corrects a password.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lowercased file name from a full image path (`C:\Program Files\X\Code.EXE` -> `code.exe`).
pub fn exe_name_from_path(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

/// Window classes whose `ES_PASSWORD` style bit means "password edit" (Edit, RichEdit20W,
/// RICHEDIT50W, WindowsForms10.EDIT.app.0...). On other classes bit 0x20 means something else.
pub fn is_edit_class(class: &str) -> bool {
    let lower = class.to_ascii_lowercase();
    lower == "edit" || lower.contains(".edit.") || lower.starts_with("richedit")
}

const ES_PASSWORD: u32 = 0x20;

/// True when a window of `class` with `style` is a classic password edit control.
pub fn is_password_edit(class: &str, style: u32) -> bool {
    style & ES_PASSWORD != 0 && is_edit_class(class)
}

/// Shared slot the UIA worker writes and the hook thread reads: `generation << 1 | password`.
#[derive(Debug, Default)]
pub struct PasswordSlot(AtomicU64);

impl PasswordSlot {
    pub fn publish(&self, generation: u64, password: bool) {
        self.0
            .store(generation << 1 | u64::from(password), Ordering::Release);
    }

    /// Secure while the answer for `generation` hasn't arrived yet, or if it says "password".
    pub fn secure(&self, generation: u64) -> bool {
        let v = self.0.load(Ordering::Acquire);
        v >> 1 != generation || v & 1 == 1
    }
}

/// Hook-thread view of the focused field.
#[derive(Debug)]
pub struct FocusState {
    pub app: Option<String>,
    pub app_pid: u32,
    pub es_password: bool,
    pub generation: u64,
    pub slot: Arc<PasswordSlot>,
    /// False when the UIA worker couldn't start or died; then only `ES_PASSWORD` counts.
    pub uia_available: bool,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            app: None,
            app_pid: 0,
            es_password: false,
            generation: 0,
            slot: Arc::new(PasswordSlot::default()),
            uia_available: false,
        }
    }
}

impl FocusState {
    pub fn secure(&self) -> bool {
        self.es_password || (self.uia_available && self.slot.secure(self.generation))
    }

    /// Start a new focus generation; the caller then asks the worker about it.
    pub fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
}

#[cfg(windows)]
pub use win::{focused_es_password, foreground, process_exe, PasswordProbe};

#[cfg(windows)]
mod win {
    use super::{exe_name_from_path, is_password_edit, PasswordSlot};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::sync::Arc;
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
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomation2};
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

    /// Is the keyboard-focus window of `foreground`'s thread a classic password edit?
    pub fn focused_es_password(foreground: HWND) -> bool {
        // SAFETY: plain Win32 queries with valid out-pointers; none of them send messages.
        unsafe {
            let tid = GetWindowThreadProcessId(foreground, None);
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if tid == 0 || GetGUIThreadInfo(tid, &mut info).is_err() || info.hwndFocus.is_invalid()
            {
                return false;
            }
            let mut class = [0u16; 128];
            let n = GetClassNameW(info.hwndFocus, &mut class);
            if n <= 0 {
                return false;
            }
            let style = GetWindowLongW(info.hwndFocus, GWL_STYLE) as u32;
            is_password_edit(&String::from_utf16_lossy(&class[..n as usize]), style)
        }
    }

    /// Handle to the UIA worker thread. Dropping it stops the worker after its current query.
    pub struct PasswordProbe {
        requests: Sender<u64>,
    }

    impl PasswordProbe {
        /// Start the worker. It initialises COM as MTA on its own thread (the UIA client
        /// threading model Microsoft recommends) and owns the `IUIAutomation` object.
        pub fn start(slot: Arc<PasswordSlot>) -> std::io::Result<Self> {
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("unflub-uia".into())
                .spawn(move || worker(rx, slot))?;
            Ok(Self { requests: tx })
        }

        /// Ask about the field focused now. False if the worker is gone.
        pub fn request(&self, generation: u64) -> bool {
            self.requests.send(generation).is_ok()
        }
    }

    fn worker(rx: Receiver<u64>, slot: Arc<PasswordSlot>) {
        // SAFETY: balanced by CoUninitialize below; this thread never enters an STA.
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // SAFETY: COM is initialised on this thread.
        let uia: Option<IUIAutomation> =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() };
        if let Some(uia2) = uia.as_ref().and_then(|u| u.cast::<IUIAutomation2>().ok()) {
            // SAFETY: valid interface; failures just leave the defaults.
            unsafe {
                let _ = uia2.SetConnectionTimeout(UIA_CONNECTION_TIMEOUT_MS);
                let _ = uia2.SetTransactionTimeout(UIA_TRANSACTION_TIMEOUT_MS);
            }
        }
        while let Ok(mut generation) = rx.recv() {
            // Coalesce: only the latest focus matters.
            loop {
                match rx.try_recv() {
                    Ok(g) => generation = g,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return finish(init.is_ok()),
                }
            }
            let password = uia.as_ref().is_some_and(|u| {
                // SAFETY: valid interface used on the MTA thread that created it.
                unsafe {
                    u.GetFocusedElement()
                        .and_then(|e| e.CurrentIsPassword())
                        .is_ok_and(|b| b.as_bool())
                }
            });
            slot.publish(generation, password);
        }
        drop(uia);
        finish(init.is_ok());
    }

    fn finish(initialised: bool) {
        if initialised {
            // SAFETY: matches the successful CoInitializeEx on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn es_password_only_counts_on_edit_classes() {
        assert!(is_password_edit("Edit", 0x50010020));
        assert!(is_password_edit(
            "WindowsForms10.EDIT.app.0.141b42a_r6_ad1",
            0x20
        ));
        assert!(is_password_edit("RICHEDIT50W", 0x20));
        assert!(!is_password_edit("Edit", 0x50010000));
        // 0x20 on a button is BS_... something else entirely.
        assert!(!is_password_edit("Button", 0x20));
        assert!(!is_password_edit("Chrome_RenderWidgetHostHWND", 0x20));
    }

    #[test]
    fn pending_generation_is_secure_until_answered() {
        let mut f = FocusState {
            uia_available: true,
            ..FocusState::default()
        };
        let g1 = f.next_generation();
        assert!(f.secure(), "no answer yet: fail closed");
        f.slot.publish(g1, false);
        assert!(!f.secure());
        let g2 = f.next_generation();
        assert!(f.secure(), "stale answer for g1 doesn't count for g2");
        f.slot.publish(g2, true);
        assert!(f.secure(), "password field");
        f.next_generation();
        f.slot.publish(g2, false);
        assert!(f.secure(), "late answer for an old generation is ignored");
    }

    #[test]
    fn es_password_wins_and_no_uia_means_es_password_only() {
        let mut f = FocusState::default();
        f.next_generation();
        assert!(!f.secure(), "without UIA, pending doesn't block");
        f.es_password = true;
        assert!(f.secure());
    }
}
