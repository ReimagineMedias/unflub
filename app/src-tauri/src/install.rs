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
use std::path::{Path, PathBuf};

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

/// May this copy register itself to start at login? Only an installed copy, from where it was
/// installed: the setup's per-user folder (`%LOCALAPPDATA%\unflub`), the MSI's folder under
/// Program Files, or Scoop's stable `apps\unflub\current` path. Never a portable copy, a dev or
/// `target\release` build, or the Store (which has its own startup task).
pub fn login_allowed(source: InstallSource) -> bool {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return false;
    };
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    login_allowed_in(&dir, source, local.as_deref(), &program_files)
}

/// Windows paths compare without regard to case.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn login_allowed_in(
    dir: &Path,
    source: InstallSource,
    local_appdata: Option<&Path>,
    program_files: &[PathBuf],
) -> bool {
    match source {
        InstallSource::Installer | InstallSource::Winget => {
            local_appdata.is_some_and(|l| same_path(dir, &l.join("unflub")))
                || program_files
                    .iter()
                    .any(|pf| same_path(dir, &pf.join("unflub")))
        }
        InstallSource::Scoop => {
            let parts: Vec<String> = dir
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect();
            parts.ends_with(&["apps".into(), "unflub".into(), "current".into()])
        }
        InstallSource::Store | InstallSource::Portable => false,
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
    fn login_only_from_an_installed_location() {
        let local = Path::new(r"C:\Users\a\AppData\Local");
        let pf = vec![PathBuf::from(r"C:\Program Files")];
        let ok = |dir: &str, source| login_allowed_in(Path::new(dir), source, Some(local), &pf);
        use InstallSource::*;
        // The setup's per-user folder and the MSI's folder, any case or trailing slash.
        assert!(ok(r"C:\Users\a\AppData\Local\unflub", Installer));
        assert!(ok(r"c:\users\A\appdata\local\UNFLUB\", Winget));
        assert!(ok(r"C:\Program Files\unflub", Installer));
        // Dev and release builds in a checkout, even if something calls them "installer".
        assert!(!ok(
            r"D:\Code\unflub-wt\api-updates\target\release",
            Installer
        ));
        assert!(!ok(r"D:\Code\unflub\target\debug", Installer));
        assert!(!ok(r"C:\Users\a\AppData\Local\unflub\sub", Installer));
        assert!(!ok(r"C:\Users\a\AppData\Local\unflub-old", Installer));
        // Scoop's stable path only, never a versioned folder.
        assert!(ok(r"C:\Users\a\scoop\apps\unflub\current", Scoop));
        assert!(!ok(r"C:\Users\a\scoop\apps\unflub\0.1.0", Scoop));
        // Portable and Store copies never write the entry.
        assert!(!ok(r"C:\Users\a\AppData\Local\unflub", Portable));
        assert!(!ok(r"C:\Users\a\AppData\Local\unflub", Store));
        assert!(!login_allowed_in(
            Path::new(r"C:\Users\a\AppData\Local\unflub"),
            Installer,
            None,
            &[]
        ));
    }

    #[test]
    fn json_names() {
        assert_eq!(
            serde_json::to_string(&InstallSource::Winget).unwrap(),
            "\"winget\""
        );
    }
}
