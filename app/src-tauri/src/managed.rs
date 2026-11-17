//! Machine policy set by an organization (Group Policy, Intune or a .reg file):
//!
//! ```text
//! HKLM\Software\Policies\RMGN\unflub
//!   DisableUpdates       DWORD 1: no update checks, downloads or installs
//!   DisableUsageStats    DWORD 1: no usage stats counted or sent
//!   DisableCrashReports  DWORD 1: no crash reports kept or sent
//!   DisableRemoteConfig  DWORD 1: no remote config fetched; built-in defaults only
//! ```
//!
//! A policy wins over the user's switch, which the settings window shows as "Managed by your
//! organization". Read once at startup.

use serde::Serialize;

pub const KEY: &str = r"Software\Policies\RMGN\unflub";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Managed {
    pub disable_updates: bool,
    pub disable_usage_stats: bool,
    pub disable_crash_reports: bool,
    pub disable_remote_config: bool,
}

impl Managed {
    pub fn read() -> Self {
        Self::from_values(|name| crate::system::policy_dword(KEY, name))
    }

    /// Any non-zero DWORD turns a policy on.
    fn from_values(read: impl Fn(&str) -> Option<u32>) -> Self {
        let on = |name| read(name).is_some_and(|v| v != 0);
        Self {
            disable_updates: on("DisableUpdates"),
            disable_usage_stats: on("DisableUsageStats"),
            disable_crash_reports: on("DisableCrashReports"),
            disable_remote_config: on("DisableRemoteConfig"),
        }
    }

    /// The settings as they take effect: a policy forces its switch off. The stored settings
    /// keep the user's choice for when the policy goes away.
    pub fn effective(&self, mut s: crate::settings::Settings) -> crate::settings::Settings {
        if self.disable_updates {
            s.auto_update = false;
        }
        if self.disable_usage_stats {
            s.share_usage_stats = false;
        }
        if self.disable_crash_reports {
            s.send_crash_reports = false;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn reads_dwords() {
        let m = Managed::from_values(|name| match name {
            "DisableUpdates" => Some(1),
            "DisableUsageStats" => Some(0),
            "DisableRemoteConfig" => Some(7),
            _ => None,
        });
        assert_eq!(
            m,
            Managed {
                disable_updates: true,
                disable_usage_stats: false,
                disable_crash_reports: false,
                disable_remote_config: true,
            }
        );
        assert_eq!(Managed::from_values(|_| None), Managed::default());
    }

    #[test]
    fn policies_only_turn_things_off() {
        let all = Managed {
            disable_updates: true,
            disable_usage_stats: true,
            disable_crash_reports: true,
            disable_remote_config: true,
        };
        let s = all.effective(Settings::default());
        assert!(!s.auto_update && !s.share_usage_stats && !s.send_crash_reports);
        // No policy changes nothing, including a switch the user turned off.
        let off = Settings {
            share_usage_stats: false,
            ..Settings::default()
        };
        assert_eq!(Managed::default().effective(off.clone()), off);
    }
}
