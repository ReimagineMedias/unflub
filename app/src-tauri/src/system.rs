//! What the OS says: its look (accent colour, dark taskbar), its version, our memory use and
//! machine policy. The pill's OS look is read by the engine process (`unflub_engine::look`).

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

/// Memory only this process holds (Windows "private bytes"), in whole MB. `None` where the OS
/// doesn't say.
pub fn private_memory_mb() -> Option<u64> {
    #[cfg(windows)]
    {
        windows_impl::private_bytes().map(|b| b / (1024 * 1024))
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
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    pub fn read_dword(key: &str, value: &str) -> Option<u32> {
        read_dword_in(HKEY_CURRENT_USER, key, value)
    }

    pub fn read_dword_hklm(key: &str, value: &str) -> Option<u32> {
        read_dword_in(HKEY_LOCAL_MACHINE, key, value)
    }

    pub fn private_bytes() -> Option<u64> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: the pseudo handle needs no closing; counters is a valid local of `size` bytes,
        // and the EX struct starts with the plain one.
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
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
