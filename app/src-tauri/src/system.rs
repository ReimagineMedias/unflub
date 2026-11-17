//! What the OS says about its look: accent colour, and whether the taskbar and apps are dark.

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

#[cfg(windows)]
mod windows_impl {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    pub fn read_dword(key: &str, value: &str) -> Option<u32> {
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: data and size point to valid locals sized for a DWORD.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
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
