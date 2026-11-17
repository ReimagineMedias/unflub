//! Where this copy of unflub came from, which decides who updates it.
//!
//! The setup and the package managers leave a one-line `install-source` file beside
//! `unflub.exe`: the NSIS setup writes `installer`, or what `/SOURCE=` says (the winget
//! manifest passes `/SOURCE=winget`); the Scoop manifest writes `scoop`. Without the file:
//! an MSIX package is `store`, an MSI or NSIS install is `installer` (its uninstaller is
//! beside it, or it lives under Program Files), and anything else is `portable`.
//!
//! Only `installer` and `winget` copies update themselves. The others show "Update with …"
//! and leave it to their tool: Scoop and the Store replace files they own, and a portable copy
//! has no installer to run.

use serde::Serialize;
use std::path::Path;

pub const MARKER: &str = "install-source";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallSource {
    Installer,
    Winget,
    Scoop,
    Store,
    Portable,
}

impl InstallSource {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "installer" | "setup" | "msi" | "chocolatey" => Some(Self::Installer),
            "winget" => Some(Self::Winget),
            "scoop" => Some(Self::Scoop),
            "store" => Some(Self::Store),
            "portable" => Some(Self::Portable),
            _ => None,
        }
    }

    /// unflub may download and run its own installer.
    pub fn self_updates(self) -> bool {
        matches!(self, Self::Installer | Self::Winget)
    }

    /// Where this copy of unflub came from.
    pub fn detect() -> Self {
        let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        else {
            return Self::Portable;
        };
        detect_in(&dir, packaged())
    }
}

fn detect_in(dir: &Path, packaged: bool) -> InstallSource {
    if let Some(source) = std::fs::read_to_string(dir.join(MARKER))
        .ok()
        .and_then(|s| InstallSource::parse(&s))
    {
        return source;
    }
    if packaged {
        return InstallSource::Store;
    }
    let program_files = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|pf| dir.starts_with(pf));
    if dir.join("uninstall.exe").exists() || program_files {
        InstallSource::Installer
    } else {
        InstallSource::Portable
    }
}

/// Running from an MSIX package (the Microsoft Store).
fn packaged() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
        let mut len = 0u32;
        // SAFETY: a size query with no buffer. APPMODEL_ERROR_NO_PACKAGE (15700) means unpackaged;
        // ERROR_INSUFFICIENT_BUFFER (122) means there is a package name to read.
        let status = unsafe { GetCurrentPackageFullName(&mut len, windows::core::PWSTR::null()) };
        status.0 == 122
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("unflub-src-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_marker_wins() {
        for (text, want) in [
            ("scoop\n", InstallSource::Scoop),
            ("winget", InstallSource::Winget),
            (" Installer ", InstallSource::Installer),
            ("chocolatey", InstallSource::Installer),
            ("portable", InstallSource::Portable),
        ] {
            let dir = temp("marker");
            std::fs::write(dir.join(MARKER), text).unwrap();
            std::fs::write(dir.join("uninstall.exe"), "").unwrap();
            assert_eq!(detect_in(&dir, true), want, "{text:?}");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn without_a_marker() {
        let dir = temp("plain");
        assert_eq!(detect_in(&dir, false), InstallSource::Portable);
        assert_eq!(detect_in(&dir, true), InstallSource::Store);
        std::fs::write(dir.join("uninstall.exe"), "").unwrap();
        assert_eq!(detect_in(&dir, false), InstallSource::Installer);
        std::fs::write(dir.join(MARKER), "something new").unwrap();
        assert_eq!(
            detect_in(&dir, false),
            InstallSource::Installer,
            "unknown text is ignored"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn who_updates() {
        assert!(InstallSource::Installer.self_updates());
        assert!(InstallSource::Winget.self_updates());
        for s in [
            InstallSource::Scoop,
            InstallSource::Store,
            InstallSource::Portable,
        ] {
            assert!(!s.self_updates(), "{s:?}");
        }
    }

    #[test]
    fn json_names() {
        assert_eq!(
            serde_json::to_string(&InstallSource::Winget).unwrap(),
            "\"winget\""
        );
    }
}
