//! What the OS look contributes to the pill: app dark mode and the accent colour. Read again
//! before each pill, so "match system" follows the OS without a message from the app.

/// The OS look right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OsLook {
    /// Apps are in dark mode ("Choose your app mode").
    pub apps_dark: bool,
    /// Accent colour as `#RRGGBB`.
    pub accent: Option<String>,
}

impl OsLook {
    pub fn read() -> Self {
        Self {
            apps_dark: apps_use_dark(),
            accent: accent_color(),
        }
    }
}

/// The OS accent colour as `#RRGGBB`, if the platform exposes one.
pub fn accent_color() -> Option<String> {
    #[cfg(windows)]
    {
        // DWM stores the accent as 0xAABBGGRR.
        let abgr = read_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")?;
        let (r, g, b) = (abgr & 0xff, (abgr >> 8) & 0xff, (abgr >> 16) & 0xff);
        Some(format!("#{r:02X}{g:02X}{b:02X}"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// True when apps are in dark mode, the switch web views see as `prefers-color-scheme`. Light
/// when the OS doesn't say.
pub fn apps_use_dark() -> bool {
    #[cfg(windows)]
    {
        read_dword(
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
fn read_dword(key: &str, value: &str) -> Option<u32> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

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
