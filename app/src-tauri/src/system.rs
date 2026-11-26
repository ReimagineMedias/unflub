//! What the OS says: its look (accent colour, dark taskbar), its version, our memory use,
//! machine policy, and the coarse facts the usage stats bucket (build, keyboard language,
//! installed memory). The pill's OS look is read by the engine process (`unflub_engine::look`).

/// The OS accent colour as `#RRGGBB`, if the platform exposes one.
pub fn accent_color() -> Option<String> {
    #[cfg(windows)]
    {
        // DWM stores the accent as 0xAABBGGRR.
        let abgr = windows_impl::read_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")?;
        let (r, g, b) = (abgr & 0xff, (abgr >> 8) & 0xff, (abgr >> 16) & 0xff);
        Some(format!("#{r:02X}{g:02X}{b:02X}"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// True when the taskbar and tray use a dark background, so the tray icon should be light.
pub fn taskbar_is_dark() -> bool {
    #[cfg(windows)]
    {
        windows_impl::read_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "SystemUsesLightTheme",
        )
        .map(|v| v == 0)
        .unwrap_or(true)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// The active app before opening the native tray menu.
pub fn foreground_exe() -> Option<String> {
    #[cfg(windows)]
    return foreground::exe();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
pub use foreground::start as track_foreground;

#[cfg(windows)]
mod foreground {
    use std::cell::Cell;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
        EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
    };

    // Native foreground events run on the main thread, including while the tray popup is open.
    // Keep the previous app while the Windows taskbar or its overflow puts itself in front.
    thread_local! {
        static LAST_APP: Cell<Option<(HWND, u32)>> = const { Cell::new(None) };
    }

    fn is_tray(window: HWND) -> bool {
        let mut class = [0u16; 64];
        // SAFETY: GetClassNameW bounds its write by the length of this buffer.
        let len = unsafe { GetClassNameW(window, &mut class) };
        matches!(
            String::from_utf16_lossy(&class[..len.max(0) as usize]).as_str(),
            "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "NotifyIconOverflowWindow"
                | "TopLevelWindowForOverflowXamlIsland"
        )
    }

    fn app_pid(window: HWND) -> Option<u32> {
        if window == unsafe { windows::Win32::UI::WindowsAndMessaging::GetShellWindow() } {
            return None;
        }
        let mut pid = 0;
        // SAFETY: pid is a valid output buffer, and these calls accept an invalid window.
        unsafe {
            if !IsWindowVisible(window).as_bool() {
                return None;
            }
            GetWindowThreadProcessId(window, Some(&mut pid));
        }
        (pid != 0 && pid != std::process::id()).then_some(pid)
    }

    fn remember(window: HWND) {
        if !is_tray(window) {
            LAST_APP.set(app_pid(window).map(|pid| (window, pid)));
        }
    }

    pub fn start() {
        // SAFETY: the callback is a static function, with no user pointer. Windows removes
        // the event subscription when the process exits. This observes focus, not input.
        unsafe {
            remember(GetForegroundWindow());
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
        }
    }

    unsafe extern "system" fn on_foreground(
        _: HWINEVENTHOOK,
        _: u32,
        window: HWND,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        remember(window);
    }

    pub fn exe() -> Option<String> {
        // SAFETY: the foreground window is borrowed and may stop being valid at any time.
        let window = unsafe { GetForegroundWindow() };
        let pid = if is_tray(window) {
            let (last, pid) = LAST_APP.get()?;
            (app_pid(last) == Some(pid)).then_some(pid)?
        } else {
            app_pid(window)?
        };
        // SAFETY: the path query uses a valid output buffer. Close the handle even on error.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            result.ok()?;
            let path = String::from_utf16(&path[..len as usize]).ok()?;
            let exe = std::path::Path::new(&path)
                .file_name()?
                .to_str()?
                .to_lowercase();
            (exe.ends_with(".exe")
                && exe.chars().count() <= crate::settings::MAX_APP_CHARS
                && !exe.chars().any(char::is_control))
            .then_some(exe)
        }
    }
}

/// The tray and its engine's private bytes in whole MiB, using the owned child handle.
#[cfg(windows)]
pub fn pair_private_mb(child: &std::process::Child) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: this pseudo handle needs no closing.
    let app = unsafe { GetCurrentProcess() };
    let engine = HANDLE(child.as_raw_handle());
    let bytes =
        windows_impl::private_bytes(app)?.checked_add(windows_impl::private_bytes(engine)?)?;
    Some(bytes / (1024 * 1024))
}

/// True after 60 seconds with no mouse or keyboard input in the current Windows session.
pub fn input_idle() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

        let mut last = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: last is a valid, sized buffer. Wrapping subtraction handles the tick rollover.
        unsafe {
            GetLastInputInfo(&mut last).as_bool()
                && GetTickCount().wrapping_sub(last.dwTime) >= 60_000
        }
    }
    #[cfg(not(windows))]
    false
}

/// The Windows build number (`CurrentBuildNumber`, e.g. 26100). `None` elsewhere.
pub fn windows_build() -> Option<u32> {
    #[cfg(windows)]
    {
        windows_impl::read_string_hklm(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuildNumber",
        )?
        .trim()
        .parse()
        .ok()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The `LANGID` of the keyboard layout active in the foreground window. `None` elsewhere.
pub fn keyboard_langid() -> Option<u16> {
    #[cfg(windows)]
    {
        windows_impl::foreground_langid()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Installed memory in GB, rounded to the nearest whole GB. `None` where the OS doesn't say.
pub fn installed_ram_gb() -> Option<u64> {
    #[cfg(windows)]
    {
        const GB_IN_KB: u64 = 1024 * 1024;
        windows_impl::installed_memory_kb().map(|kb| (kb + GB_IN_KB / 2) / GB_IN_KB)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// A machine policy DWORD under `HKLM\<key>` (see `managed.rs`). `None` when unset or not on
/// Windows.
pub fn policy_dword(key: &str, name: &str) -> Option<u32> {
    #[cfg(windows)]
    {
        windows_impl::read_dword_hklm(key, name)
    }
    #[cfg(not(windows))]
    {
        let _ = (key, name);
        None
    }
}

#[cfg(windows)]
mod windows_impl {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    use windows::Win32::System::SystemInformation::GetPhysicallyInstalledSystemMemory;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    pub fn read_string_hklm(key: &str, value: &str) -> Option<String> {
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: buf and size describe a valid local buffer; the value is NUL-terminated.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(key),
                &HSTRING::from(value),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        let units = (size as usize / 2).min(buf.len());
        let end = buf[..units].iter().position(|&c| c == 0).unwrap_or(units);
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    pub fn foreground_langid() -> Option<u16> {
        // SAFETY: trivial queries; a null window gives thread 0, whose layout is this thread's.
        let hkl = unsafe {
            let thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
            GetKeyboardLayout(thread)
        };
        let langid = (hkl.0 as usize & 0xFFFF) as u16;
        (langid != 0).then_some(langid)
    }

    pub fn installed_memory_kb() -> Option<u64> {
        let mut kb = 0u64;
        // SAFETY: kb is a valid local.
        unsafe { GetPhysicallyInstalledSystemMemory(&mut kb) }.ok()?;
        (kb > 0).then_some(kb)
    }

    pub fn read_dword(key: &str, value: &str) -> Option<u32> {
        read_dword_in(HKEY_CURRENT_USER, key, value)
    }

    pub fn read_dword_hklm(key: &str, value: &str) -> Option<u32> {
        read_dword_in(HKEY_LOCAL_MACHINE, key, value)
    }

    pub fn private_bytes(process: HANDLE) -> Option<u64> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: the caller owns a valid process handle; counters is a valid local of `size` bytes,
        // and the EX struct starts with the plain one.
        unsafe {
            GetProcessMemoryInfo(
                process,
                &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                size,
            )
        }
        .ok()?;
        Some(counters.PrivateUsage as u64)
    }

    fn read_dword_in(root: HKEY, key: &str, value: &str) -> Option<u32> {
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: data and size point to valid locals sized for a DWORD.
        let status = unsafe {
            RegGetValueW(
                root,
                &HSTRING::from(key),
                &HSTRING::from(value),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut data as *mut u32 as *mut _),
                Some(&mut size),
            )
        };
        status.is_ok().then_some(data)
    }
}
