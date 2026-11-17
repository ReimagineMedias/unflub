//! What the OS says: its look (accent colour, dark taskbar and apps), its version, our memory
//! use and our login entry.

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

/// True when apps are set to dark mode ("Choose your app mode"), the same switch web views see
/// as `prefers-color-scheme`. Light when the OS doesn't say.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn apps_use_dark() -> bool {
    #[cfg(windows)]
    {
        windows_impl::read_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "AppsUseLightTheme",
        )
        .is_some_and(|v| v == 0)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The OS version as `major.minor.build` (`10.0.26300` on Windows 11), for the `X-OS-Version`
/// header. `unknown` when the OS doesn't say.
pub fn os_version() -> String {
    #[cfg(windows)]
    {
        // GetVersionEx lies to apps without a compatibility manifest; the registry doesn't.
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let major = windows_impl::read_dword_hklm(key, "CurrentMajorVersionNumber");
        let minor = windows_impl::read_dword_hklm(key, "CurrentMinorVersionNumber");
        let build = windows_impl::read_string_hklm(key, "CurrentBuildNumber");
        match (major, minor, build) {
            (Some(major), Some(minor), Some(build)) => format!("{major}.{minor}.{build}"),
            _ => "unknown".into(),
        }
    }
    #[cfg(not(windows))]
    {
        "unknown".into()
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

/// True when the per-user `Run` key has an entry called `name`, enabled or not. Task Manager's
/// "Disable" keeps the entry and marks it off elsewhere; uninstalling removes it.
#[cfg(windows)]
pub fn login_entry_exists(name: &str) -> bool {
    windows_impl::value_exists(r"Software\Microsoft\Windows\CurrentVersion\Run", name)
}

#[cfg(windows)]
mod windows_impl {
    use windows::core::HSTRING;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_ANY, RRF_RT_REG_DWORD,
        RRF_RT_REG_SZ,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    pub fn value_exists(key: &str, value: &str) -> bool {
        // SAFETY: no data buffer is passed, so only the value's presence is read.
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                &HSTRING::from(value),
                RRF_RT_ANY,
                None,
                None,
                None,
            )
        }
        .is_ok()
    }

    pub fn read_dword(key: &str, value: &str) -> Option<u32> {
        read_dword_in(HKEY_CURRENT_USER, key, value)
    }

    pub fn read_dword_hklm(key: &str, value: &str) -> Option<u32> {
        read_dword_in(HKEY_LOCAL_MACHINE, key, value)
    }

    pub fn read_string_hklm(key: &str, value: &str) -> Option<String> {
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: buf and size describe a valid local buffer; RRF_RT_REG_SZ null-terminates.
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
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16(&buf[..len]).ok()
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
