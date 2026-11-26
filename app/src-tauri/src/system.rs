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
