//! Messages between the app and the engine. JSON, one per [`frame`](crate::frame).
//!
//! Why JSON and not bincode: messages are rare (a settings change, a fix, a word count every
//! few seconds), so encoding speed doesn't matter, while a self-describing format lets either
//! side skip a message it doesn't know and keeps `#[serde(default)]` fields compatible across
//! versions. `serde_json` is already in both processes. bincode's format isn't
//! self-describing (a field added on one side mis-decodes on the other without an error), and
//! the project stopped maintenance in 2025.
//!
//! A session:
//!
//! 1. The engine connects and sends [`FromEngine::Hello`].
//! 2. The app checks the pipe client's process id and [`PROTOCOL`], then sends
//!    [`ToEngine::Welcome`] and [`ToEngine::Configure`]. A mismatch ends the engine.
//! 3. The engine installs the hook and reports [`FromEngine::Ready`]. From then on it sends
//!    fixes, undos, learned words, word counts and health events, and the app sends new
//!    settings. [`ToEngine::Shutdown`], or the pipe closing, unhooks and exits.

use serde::{Deserialize, Serialize};
use unflub_core::Config;

pub use crate::category::AppCategory;

/// Bump on any change a peer of the previous version would misread.
pub const PROTOCOL: u32 = 2;

/// The app probes every 15 seconds. Leave room for two missed probes and a cold start.
pub const FRAME_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// App to engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToEngine {
    /// The answer to [`FromEngine::Hello`]. `app_exe` is the app's executable name, lowercased:
    /// the hook never feeds keys typed into unflub's own windows to the engine.
    Welcome { protocol: u32, app_exe: String },
    /// Everything the engine runs with. Sent after `Welcome` and on every change.
    Configure { settings: EngineSettings },
    /// Confirm the engine worker is processing messages, and the hook thread's message pump
    /// too (it must have answered the probe sent with the previous ping).
    Ping { nonce: u64 },
    /// Unhook, close the pill and exit.
    Shutdown,
}

/// Engine to app. The words in `Corrected`, `Undone` and `Learned` go only to the app's local
/// Stats page and personal dictionary. The rest (trigger, source, app category, counts and
/// timings) feeds the anonymous usage stats; the focused app's name never crosses the pipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FromEngine {
    Hello {
        protocol: u32,
        version: String,
    },
    /// The hook is installed (`hooked`), or couldn't be. The engine keeps running and tries
    /// again on the next settings change. Sent again with `hooked: false` if the keyboard hook
    /// is lost later (a reinstall after resume, unlock or the watchdog failed): the app treats
    /// it as a failed start and restarts the engine.
    Ready {
        hooked: bool,
    },
    /// Sent by the engine worker after it reaches the matching ping in its queue, unless the
    /// hook thread's message pump has stopped answering (then the app restarts the engine).
    Pong {
        nonce: u64,
    },
    Corrected {
        original: String,
        corrected: String,
        /// The user's own shortcut, not a typo fix.
        user_rule: bool,
        decide_us: u64,
        inject_us: u64,
        /// The pill was asked to show it.
        pill: bool,
        /// The local Stats page may keep this typo and fix. False in terminals, where the word
        /// may be a password typed at a prompt (`unflub_windows::policy::keep_words`), and
        /// when missing.
        #[serde(default)]
        keep_word: bool,
        /// The key that ended the word.
        #[serde(default)]
        trigger: Trigger,
        /// Which part of the engine made the fix.
        #[serde(default)]
        source: FixSource,
        /// The kind of app it happened in.
        #[serde(default)]
        category: AppCategory,
    },
    /// A first Backspace after a fix is held: the pill asks "Undo?".
    UndoArmed,
    Undone {
        original: String,
        corrected: String,
        user_rule: bool,
        /// The app may keep this pair (for learning by undo). False in terminals and unknown
        /// apps, as for `Corrected`, and when missing.
        #[serde(default)]
        keep_word: bool,
    },
    /// Words the engine checked since the last report.
    WordsChecked {
        count: u64,
    },
    /// The user taught the engine a word by undoing its fix.
    Learned {
        word: String,
    },
    /// The hooks were installed again, and why (missing from an older engine).
    HookReinstalled {
        #[serde(default)]
        reason: Option<ReinstallReason>,
    },
    /// Counts and timings since the last of these (sent with the word count, every few
    /// seconds while there is something to send). Times in microseconds.
    UsageCounts {
        /// Minutes with at least one key typed.
        active_minutes: u64,
        /// Words that ended in a password field (or one not yet known to be safe).
        skipped_secure: u64,
        /// Words typed in an app on the user's exclusion list.
        skipped_excluded: u64,
        /// Words the code-skip rules left alone.
        skipped_code: u64,
        /// Every decision on a word-ending key, fixed or not.
        decide_all_us: Vec<u32>,
        /// Each Windows spell checker `Check` call.
        spellcheck_us: Vec<u32>,
    },
    /// A local diagnostic. The wire representation contains no user-derived strings.
    Diagnostic {
        event: EngineDiagnostic,
    },
    /// The corrector panicked; keys pass through until the engine restarts.
    CorrectorPanicked,
    /// `active`: our injected input stopped coming back (another program is interfering with
    /// keyboard input), so the engine paused itself: keys pass through, nothing is corrected.
    /// `!active`: it works again.
    InputInterference {
        active: bool,
    },
}

/// Fixed warning types the app may keep in its local log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EngineDiagnostic {
    HookWarning,
    UiaWarning,
    PillUnavailable,
    InjectionPartial { sent: usize, total: usize },
}

/// What ended a fixed word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    #[default]
    Space,
    Punctuation,
    EnterTab,
}

impl Trigger {
    /// The trigger for the boundary character a fix ended with.
    pub fn of(boundary: char) -> Self {
        match boundary {
            '\n' | '\r' | '\t' => Self::EnterTab,
            c if c.is_whitespace() => Self::Space,
            _ => Self::Punctuation,
        }
    }
}

/// Which part of the engine made a fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FixSource {
    /// unflub's own dictionary and ranking.
    #[default]
    Core,
    /// The Windows spell checker's own replacement.
    Os,
    /// One of the user's shortcuts.
    UserRule,
}

impl From<unflub_core::Source> for FixSource {
    fn from(s: unflub_core::Source) -> Self {
        match s {
            unflub_core::Source::Bundled => Self::Core,
            unflub_core::Source::System => Self::Os,
            unflub_core::Source::UserRule => Self::UserRule,
        }
    }
}

/// Why the hooks were installed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReinstallReason {
    /// The watchdog saw input the hooks missed.
    Watchdog,
    /// The computer resumed from sleep.
    Resume,
    /// The session was unlocked.
    Unlock,
}

impl ReinstallReason {
    /// From the adapter's `HookEvent::HooksReinstalled` reason.
    pub fn parse(reason: &str) -> Option<Self> {
        match reason {
            "watchdog" => Some(Self::Watchdog),
            "resume" => Some(Self::Resume),
            "unlock" => Some(Self::Unlock),
            _ => None,
        }
    }
}

/// What the engine runs with: the core config plus the pill. The app has already applied the
/// remote flags (they can only switch things off).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EngineSettings {
    pub core: Config,
    pub pill: PillSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PillSettings {
    pub style: PillStyle,
    pub position: PillPosition,
    pub opacity: f32,
    pub duration_ms: u32,
    pub theme: Theme,
    pub appearance: Appearance,
    /// Ask UI Automation for the caret in browser and XAML fields.
    pub uia_caret: bool,
}

impl Default for PillSettings {
    fn default() -> Self {
        let o = unflub_windows::PillConfig::default();
        Self {
            style: PillStyle::default(),
            position: PillPosition::default(),
            opacity: o.opacity,
            duration_ms: o.duration_ms,
            theme: Theme::default(),
            appearance: Appearance::default(),
            uia_caret: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PillStyle {
    #[default]
    UnderlineCapsule,
    Capsule,
    Underline,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PillPosition {
    #[default]
    Above,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    Unflub,
    Paper,
    Ink,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    #[default]
    Auto,
    Light,
    Dark,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(v: T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), v, "{json}");
    }

    #[test]
    fn every_message_round_trips() {
        round_trip(ToEngine::Welcome {
            protocol: PROTOCOL,
            app_exe: "unflub.exe".into(),
        });
        round_trip(ToEngine::Configure {
            settings: EngineSettings::default(),
        });
        round_trip(ToEngine::Shutdown);
        for m in [
            FromEngine::Hello {
                protocol: PROTOCOL,
                version: "0.1.0".into(),
            },
            FromEngine::Ready { hooked: true },
            FromEngine::Corrected {
                original: "teh".into(),
                corrected: "the".into(),
                user_rule: false,
                decide_us: 90,
                inject_us: 400,
                pill: true,
                keep_word: true,
                trigger: Trigger::Punctuation,
                source: FixSource::Os,
                category: AppCategory::CodeEditor,
            },
            FromEngine::UndoArmed,
            FromEngine::Undone {
                original: "teh".into(),
                corrected: "the".into(),
                user_rule: false,
                keep_word: true,
            },
            FromEngine::WordsChecked { count: 12 },
            FromEngine::Learned {
                word: "rmgn".into(),
            },
            FromEngine::HookReinstalled {
                reason: Some(ReinstallReason::Unlock),
            },
            FromEngine::UsageCounts {
                active_minutes: 3,
                skipped_secure: 1,
                skipped_excluded: 2,
                skipped_code: 4,
                decide_all_us: vec![80, 120],
                spellcheck_us: vec![40],
            },
            FromEngine::CorrectorPanicked,
            FromEngine::InputInterference { active: true },
        ] {
            round_trip(m);
        }
    }

    #[test]
    fn the_wire_format_is_tagged_json() {
        let json = serde_json::to_string(&FromEngine::Ready { hooked: false }).unwrap();
        assert_eq!(json, r#"{"type":"ready","hooked":false}"#);
    }

    #[test]
    fn triggers_and_reasons() {
        assert_eq!(Trigger::of(' '), Trigger::Space);
        assert_eq!(Trigger::of('\u{a0}'), Trigger::Space);
        assert_eq!(Trigger::of('\n'), Trigger::EnterTab);
        assert_eq!(Trigger::of('\t'), Trigger::EnterTab);
        for c in ['.', ',', '!', '?', ')', ';'] {
            assert_eq!(Trigger::of(c), Trigger::Punctuation, "{c}");
        }
        for (s, r) in [
            ("watchdog", ReinstallReason::Watchdog),
            ("resume", ReinstallReason::Resume),
            ("unlock", ReinstallReason::Unlock),
        ] {
            assert_eq!(ReinstallReason::parse(s), Some(r));
        }
        assert_eq!(ReinstallReason::parse("other"), None);
        // An older engine's message still reads, with no reason.
        assert_eq!(
            serde_json::from_str::<FromEngine>(r#"{"type":"hook-reinstalled"}"#).unwrap(),
            FromEngine::HookReinstalled { reason: None }
        );
    }

    /// A newer app may add settings; an older engine keeps the rest.
    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let m: ToEngine = serde_json::from_str(
            r#"{"type":"configure","settings":{"pill":{"style":"off","sparkle":true},"later":1}}"#,
        )
        .unwrap();
        let ToEngine::Configure { settings } = m else {
            panic!("{m:?}")
        };
        assert_eq!(settings.pill.style, PillStyle::Off);
        assert_eq!(settings.core, Config::default());
        assert!(serde_json::from_str::<FromEngine>(r#"{"type":"from-the-future"}"#).is_err());
    }
}
