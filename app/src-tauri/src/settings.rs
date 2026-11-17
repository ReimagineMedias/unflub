//! Persisted user settings. The app owns this file; the engine gets a `unflub_core::Config` from it.

use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EngineMode {
    #[default]
    Both,
    SystemOnly,
    BundledOnly,
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
    pub enabled: bool,
    pub start_at_login: bool,
    pub mode: EngineMode,
    pub confidence: f32,
    pub min_word_len: usize,
    pub skip_code: bool,
    pub undo_learns: bool,
    pub excluded_apps: Vec<String>,
    pub personal_words: Vec<String>,
    pub user_rules: Vec<(String, String)>,
    pub pill_style: PillStyle,
    pub pill_opacity: f32,
    pub pill_duration_ms: u32,
    pub pill_position: PillPosition,
    pub theme: Theme,
    pub appearance: Appearance,
}

impl Default for Settings {
    fn default() -> Self {
        let core = unflub_core::Config::default();
        Self {
            enabled: core.enabled,
            start_at_login: true,
            mode: EngineMode::Both,
            confidence: core.confidence,
            min_word_len: core.min_word_len,
            skip_code: core.skip_code,
            undo_learns: core.undo_learns,
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
        for app in &mut self.excluded_apps {
            *app = app.trim().to_lowercase();
        }
        self.excluded_apps.retain(|a| !a.is_empty());
        self.excluded_apps.dedup();
        self
    }

    #[expect(
        dead_code,
        reason = "wired to the engine once the Windows adapter lands"
    )]
    pub fn to_core(&self) -> unflub_core::Config {
        unflub_core::Config {
            enabled: self.enabled,
            mode: match self.mode {
                EngineMode::Both => unflub_core::EngineMode::Both,
                EngineMode::SystemOnly => unflub_core::EngineMode::SystemOnly,
                EngineMode::BundledOnly => unflub_core::EngineMode::BundledOnly,
                EngineMode::Aggressive => unflub_core::EngineMode::Aggressive,
            },
            confidence: self.confidence,
            min_word_len: self.min_word_len,
            skip_code: self.skip_code,
            excluded_apps: self.excluded_apps.clone(),
            personal_words: self.personal_words.clone(),
            user_rules: self.user_rules.clone(),
            undo_learns: self.undo_learns,
        }
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

    /// Missing or unreadable files fall back to defaults; a corrupt file is kept as `.bak`.
    pub fn load(&self) -> Settings {
        match fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(s) => s.sanitised(),
                Err(_) => {
                    let _ = fs::rename(&self.path, self.path.with_extension("json.bak"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
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
