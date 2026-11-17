//! Persisted user settings. The app owns this file; the engine gets a `unflub_core::Config` from it.

use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// The settings file's format. Bump it and add a step to `migrations.rs` whenever a change needs
/// more than serde defaults (a renamed field, a changed meaning, a value to fill in).
pub const SCHEMA_VERSION: u32 = 2;

/// What Backspace right after a fix does. Mirrors `unflub_core::UndoMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UndoMode {
    /// The first Backspace asks "Undo?", the second undoes.
    #[default]
    Twice,
    Once,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EngineMode {
    #[default]
    Both,
    SystemOnly,
    BundledOnly,
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PillStyle {
    /// Brief underline on the fixed word, plus a small capsule.
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

/// Off by default: terminals can't report password prompts (sudo, ssh), and password managers
/// hold secrets. A correction there would silently change what the user typed.
///
/// Keep `defaultSettings.excludedApps` in `app/src/types.ts` identical (same names, same order).
pub const DEFAULT_EXCLUDED_APPS: &[&str] = &[
    "windowsterminal.exe",
    "openconsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wsl.exe",
    "mintty.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "keepass.exe",
    "keepassxc.exe",
    "1password.exe",
    "bitwarden.exe",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// See [`SCHEMA_VERSION`]. Files without it are version 0.
    pub schema_version: u32,
    pub enabled: bool,
    pub start_at_login: bool,
    /// Download updates in the background once the daily check finds one. Off: the check only
    /// tells the user. Installing always waits for a restart or quit.
    pub auto_update: bool,
    pub channel: Channel,
    /// One anonymous summary a day (`usage.rs`). On by default; counts and settings only.
    pub share_usage_stats: bool,
    /// Send scrubbed crash reports without asking each time (`crash.rs`). On by default.
    pub send_crash_reports: bool,
    /// The local Stats page keeps which words were fixed, not just how many.
    pub stats_keep_words: bool,
    /// The welcome screen was closed with "Done".
    pub onboarded: bool,
    pub mode: EngineMode,
    pub confidence: f32,
    pub min_word_len: usize,
    pub skip_code: bool,
    /// How Backspace undoes a fix (schema v2; replaced `undoLearns`).
    pub undo_mode: UndoMode,
    /// A typo is never fixed again after this many undos of the same fix, 1 to 5; 0 never.
    pub learn_after_undos: u8,
    pub excluded_apps: Vec<String>,
    pub personal_words: Vec<String>,
    pub user_rules: Vec<(String, String)>,
    pub pill_style: PillStyle,
    pub pill_opacity: f32,
    pub pill_duration_ms: u32,
    pub pill_position: PillPosition,
    pub theme: Theme,
    pub appearance: Appearance,
    /// BCP 47 tag for the system spell checker, like `en-US`.
    pub language: String,
    /// Use the English dictionary's fixes even when `language` isn't English (off: only the
    /// OS checker's own fixes there). Not in the settings window yet; settings.json only.
    pub english_for_other_languages: bool,
    /// Fields this version doesn't know, from a newer unflub. Kept and written back, so running
    /// an older version for a while doesn't wipe them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

const DEFAULT_LANGUAGE: &str = "en-US";

impl Default for Settings {
    fn default() -> Self {
        let core = unflub_core::Config::default();
        Self {
            schema_version: SCHEMA_VERSION,
            enabled: core.enabled,
            start_at_login: true,
            auto_update: true,
            channel: Channel::Stable,
            share_usage_stats: true,
            send_crash_reports: true,
            stats_keep_words: true,
            onboarded: false,
            mode: EngineMode::Both,
            confidence: core.confidence,
            min_word_len: core.min_word_len,
            skip_code: core.skip_code,
            undo_mode: UndoMode::Twice,
            learn_after_undos: core.learn_after_undos,
            excluded_apps: DEFAULT_EXCLUDED_APPS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            personal_words: Vec::new(),
            user_rules: Vec::new(),
            pill_style: PillStyle::UnderlineCapsule,
            pill_opacity: 0.92,
            pill_duration_ms: 1100,
            pill_position: PillPosition::Above,
            theme: Theme::Unflub,
            appearance: Appearance::Auto,
            language: core.language,
            english_for_other_languages: core.english_for_other_languages,
            extra: serde_json::Map::new(),
        }
    }
}

impl Settings {
    /// Clamp values a hand-edited file could break.
    pub fn sanitised(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.pill_opacity = self.pill_opacity.clamp(0.3, 1.0);
        self.pill_duration_ms = self.pill_duration_ms.clamp(300, 5000);
        self.min_word_len = self.min_word_len.clamp(1, 12);
        self.learn_after_undos = self.learn_after_undos.min(5);
        for app in &mut self.excluded_apps {
            *app = app.trim().to_lowercase();
        }
        self.excluded_apps.retain(|a| !a.is_empty());
        self.excluded_apps.dedup();
        self.language = sanitised_language(&self.language);
        self
    }

    pub fn to_core(&self) -> unflub_core::Config {
        unflub_core::Config {
            enabled: self.enabled,
            mode: match self.mode {
                EngineMode::Both => unflub_core::EngineMode::Both,
                EngineMode::SystemOnly => unflub_core::EngineMode::SystemOnly,
                EngineMode::BundledOnly => unflub_core::EngineMode::BundledOnly,
                EngineMode::Balanced => unflub_core::EngineMode::Balanced,
                EngineMode::Aggressive => unflub_core::EngineMode::Aggressive,
            },
            confidence: self.confidence,
            min_word_len: self.min_word_len,
            skip_code: self.skip_code,
            excluded_apps: self.excluded_apps.clone(),
            personal_words: self.personal_words.clone(),
            user_rules: self.user_rules.clone(),
            undo_mode: match self.undo_mode {
                UndoMode::Twice => unflub_core::UndoMode::Twice,
                UndoMode::Once => unflub_core::UndoMode::Once,
                UndoMode::Off => unflub_core::UndoMode::Off,
            },
            learn_after_undos: self.learn_after_undos,
            // Filled in from the app's own counts (`undo_counts.rs`) on the way to the engine.
            undo_counts: Vec::new(),
            // The pill's time on screen is the double-Backspace window.
            undo_window_ms: self.pill_duration_ms,
            language: self.language.clone(),
            english_for_other_languages: self.english_for_other_languages,
        }
    }
}

/// A plausible BCP 47 tag (`en`, `en-US`, `sr-Latn-RS`), or the default. The tag goes to the
/// OS spell checker, which rejects languages it has no dictionary for; the engine then falls
/// back to the user's UI language.
fn sanitised_language(tag: &str) -> String {
    let tag = tag.trim();
    let plausible = (2..=35).contains(&tag.len())
        && tag
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric()));
    if plausible {
        tag.to_string()
    } else {
        DEFAULT_LANGUAGE.to_string()
    }
}

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("settings.json"),
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Missing or unreadable files fall back to defaults; a corrupt file is kept as `.bak`. An
    /// older file is migrated (`migrations.rs`) and written back.
    pub fn load(&self) -> Settings {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Settings::default();
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .filter(serde_json::Value::is_object)
            .map(crate::migrations::migrate)
            .and_then(|(value, migrated_from)| {
                let settings = serde_json::from_value::<Settings>(value).ok()?;
                Some((settings.sanitised(), migrated_from))
            });
        match parsed {
            Some((settings, migrated_from)) => {
                if let Some(from) = migrated_from {
                    // Keep the old file beside the new one, in case the migration got it wrong.
                    let _ = fs::copy(
                        &self.path,
                        self.path.with_extension(format!("json.v{from}")),
                    );
                    if let Err(e) = self.save(&settings) {
                        eprintln!("settings: saving the migrated file: {e}");
                    }
                }
                settings
            }
            None => {
                let _ = fs::rename(&self.path, self.path.with_extension("json.bak"));
                Settings::default()
            }
        }
    }

    /// Atomic write: temp file then rename, so a crash never leaves half a file.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_fills_missing_fields() {
        let partial = r#"{ "enabled": false, "pillStyle": "capsule" }"#;
        let s: Settings = serde_json::from_str(partial).unwrap();
        assert!(!s.enabled);
        assert_eq!(s.pill_style, PillStyle::Capsule);
        assert_eq!(s.theme, Theme::Unflub);
        assert!(s.auto_update, "updates are opt-out");
        assert!(s.share_usage_stats && s.send_crash_reports && s.stats_keep_words);
        assert!(!s.onboarded);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn sanitises_out_of_range_values() {
        let s = Settings {
            confidence: 4.0,
            pill_opacity: 0.0,
            excluded_apps: vec![" Code.EXE ".into(), "".into()],
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.confidence, 1.0);
        assert_eq!(s.pill_opacity, 0.3);
        assert_eq!(s.excluded_apps, vec!["code.exe"]);
    }

    #[test]
    fn sanitises_language() {
        let lang = |l: &str| {
            Settings {
                language: l.into(),
                ..Settings::default()
            }
            .sanitised()
            .language
        };
        assert_eq!(Settings::default().language, "en-US");
        assert_eq!(lang(" fr-CA "), "fr-CA");
        assert_eq!(lang("sr-Latn-RS"), "sr-Latn-RS");
        for bad in ["", "e", "en_US", "en--US", "-en", "en-", "../../x", "en US"] {
            assert_eq!(lang(bad), "en-US", "{bad:?}");
        }
        let missing: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.language, "en-US");
    }

    #[test]
    fn maps_to_core_config() {
        let s = Settings {
            enabled: false,
            mode: EngineMode::Aggressive,
            confidence: 0.25,
            min_word_len: 4,
            skip_code: false,
            undo_mode: UndoMode::Once,
            learn_after_undos: 3,
            pill_duration_ms: 2000,
            excluded_apps: vec!["code.exe".into()],
            personal_words: vec!["rmgn".into()],
            user_rules: vec![("omw".into(), "on my way".into())],
            language: "en-GB".into(),
            english_for_other_languages: true,
            ..Settings::default()
        };
        assert_eq!(
            s.to_core(),
            unflub_core::Config {
                enabled: false,
                mode: unflub_core::EngineMode::Aggressive,
                confidence: 0.25,
                min_word_len: 4,
                skip_code: false,
                excluded_apps: vec!["code.exe".into()],
                personal_words: vec!["rmgn".into()],
                user_rules: vec![("omw".into(), "on my way".into())],
                undo_mode: unflub_core::UndoMode::Once,
                learn_after_undos: 3,
                undo_counts: Vec::new(),
                undo_window_ms: 2000,
                language: "en-GB".into(),
                english_for_other_languages: true,
            }
        );
        // Defaults agree with the engine's, apart from the apps the app excludes.
        let core = Settings::default().to_core();
        assert_eq!(
            core,
            unflub_core::Config {
                excluded_apps: core.excluded_apps.clone(),
                ..unflub_core::Config::default()
            }
        );
        for (mode, want) in [
            (EngineMode::Both, unflub_core::EngineMode::Both),
            (EngineMode::SystemOnly, unflub_core::EngineMode::SystemOnly),
            (
                EngineMode::BundledOnly,
                unflub_core::EngineMode::BundledOnly,
            ),
            (EngineMode::Balanced, unflub_core::EngineMode::Balanced),
        ] {
            let s = Settings {
                mode,
                ..Settings::default()
            };
            assert_eq!(s.to_core().mode, want);
        }
    }

    /// `defaultSettings.excludedApps` in app/src/types.ts must list exactly these, in order.
    #[test]
    fn default_excluded_apps() {
        assert_eq!(
            DEFAULT_EXCLUDED_APPS,
            &[
                "windowsterminal.exe",
                "openconsole.exe",
                "conhost.exe",
                "cmd.exe",
                "powershell.exe",
                "pwsh.exe",
                "wsl.exe",
                "mintty.exe",
                "wezterm-gui.exe",
                "alacritty.exe",
                "keepass.exe",
                "keepassxc.exe",
                "1password.exe",
                "bitwarden.exe",
            ]
        );
        let s = Settings::default();
        assert_eq!(s.clone().sanitised().excluded_apps, s.excluded_apps);
    }

    #[test]
    fn corrupt_file_falls_back_and_is_kept() {
        let dir = std::env::temp_dir().join(format!("unflub-test-{}", std::process::id()));
        let store = Store::new(dir.clone());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "{ not json").unwrap();
        assert_eq!(store.load(), Settings::default());
        assert!(dir.join("settings.json.bak").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
