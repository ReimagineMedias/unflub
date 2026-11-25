//! Persisted user settings. The app owns this file; the engine gets a `unflub_core::Config` from it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

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

    #[cfg_attr(not(windows), allow(dead_code))] // only the Windows engine host uses it so far
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
            undo_counts_epoch: 0,
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

/// Backups of settings files that couldn't be read (`settings.json.bak-<UTC time>`) kept.
const BACKUPS_KEPT: usize = 3;

/// Read `obj` field by field: a value this version can't read (a wrong type, or an option from
/// a newer unflub) falls back to its default, and every other field is kept. Returns the
/// settings and the values that couldn't be read.
fn from_object(obj: Map<String, Value>) -> (Settings, Map<String, Value>) {
    if let Ok(settings) = serde_json::from_value(Value::Object(obj.clone())) {
        return (settings, Map::new());
    }
    let (mut readable, mut unread) = (Map::new(), Map::new());
    for (key, value) in obj {
        let alone = Map::from_iter([(key.clone(), value.clone())]);
        if serde_json::from_value::<Settings>(Value::Object(alone)).is_ok() {
            readable.insert(key, value);
        } else {
            unread.insert(key, value);
        }
    }
    let settings = serde_json::from_value(Value::Object(readable)).unwrap_or_default();
    (settings, unread)
}

/// What the store knows about the file beyond the settings themselves.
#[derive(Default)]
struct FileState {
    /// Values from the file this version couldn't read, by key. Each is written back while
    /// that setting still has its default, so a newer unflub finds the user's choice again.
    unread: Map<String, Value>,
}

impl FileState {
    /// Put the unread values back into `obj` (settings about to be written) where the setting
    /// still has its default. One the user has changed since is dropped.
    fn restore_unread(&mut self, obj: &mut Map<String, Value>) {
        if self.unread.is_empty() {
            return;
        }
        let defaults = serde_json::to_value(Settings::default()).unwrap_or_default();
        self.unread
            .retain(|key, _| obj.get(key).is_none_or(|v| Some(v) == defaults.get(key)));
        for (key, value) in &self.unread {
            obj.insert(key.clone(), value.clone());
        }
    }
}

pub struct Store {
    path: PathBuf,
    /// Held for every write, and across read, change and write in [`Store::update`].
    writing: Mutex<FileState>,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("settings.json"),
            writing: Mutex::new(FileState::default()),
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Missing or unreadable files fall back to defaults; a file that isn't a JSON object is
    /// moved to a timestamped backup (the last [`BACKUPS_KEPT`] are kept). A field this version
    /// can't read gets its default and the rest of the file is kept (see [`from_object`]). An
    /// older file is migrated (`migrations.rs`) and written back.
    pub fn load(&self) -> Settings {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Settings::default();
        };
        // Notepad's "UTF-8 with BOM".
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let parsed = serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
            .map(crate::migrations::migrate)
            .and_then(|(value, migrated_from)| match value {
                Value::Object(obj) => Some((from_object(obj), migrated_from)),
                _ => None,
            });
        match parsed {
            Some(((settings, unread), migrated_from)) => {
                let settings = settings.sanitised();
                if !unread.is_empty() {
                    let keys: Vec<_> = unread.keys().collect();
                    eprintln!(
                        "settings: using defaults for {keys:?}, which this version can't read"
                    );
                }
                self.lock().unread = unread;
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
                match self.back_up(|from, to| fs::rename(from, to)) {
                    Ok(to) => eprintln!("settings: unreadable, moved to {}", to.display()),
                    Err(e) => eprintln!("settings: unreadable, and can't move it aside: {e}"),
                }
                Settings::default()
            }
        }
    }

    /// Move or copy (`how`) the settings file to `settings.json.bak-<UTC time>`, then delete all
    /// but the newest [`BACKUPS_KEPT`] of those.
    fn back_up(&self, how: impl FnOnce(&Path, &Path) -> io::Result<()>) -> io::Result<PathBuf> {
        let now = time::OffsetDateTime::now_utc();
        let stamp = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}.{:06}",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
            now.microsecond()
        );
        let prefix = "settings.json.bak-";
        let mut to = self.path.with_file_name(format!("{prefix}{stamp}"));
        let mut n = 1;
        while to.exists() {
            n += 1;
            to = self.path.with_file_name(format!("{prefix}{stamp}-{n}"));
        }
        how(&self.path, &to)?;
        if let Some(dir) = self.path.parent() {
            let mut backups: Vec<_> = fs::read_dir(dir)?
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(prefix))
                .collect();
            // Oldest first: the names sort by time.
            backups.sort();
            let surplus = backups.len().saturating_sub(BACKUPS_KEPT);
            for name in &backups[..surplus] {
                let _ = fs::remove_file(dir.join(name));
            }
        }
        Ok(to)
    }

    fn lock(&self) -> MutexGuard<'_, FileState> {
        self.writing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Atomic and flushed (`files::replace`), so a crash or power loss never leaves half a file.
    /// Saves never overlap.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        self.write(&mut self.lock(), settings)
    }

    /// Change the settings in `current` and save them, as one step: `change` sees the latest
    /// settings even when another thread just changed them, and the file and `current` always
    /// end up with the same value. Nothing changes in memory when the save fails. Returns the
    /// settings before and after.
    pub fn update(
        &self,
        current: &Mutex<Settings>,
        change: impl FnOnce(&mut Settings),
    ) -> io::Result<(Settings, Settings)> {
        let mut file = self.lock();
        let prev = current.lock().unwrap().clone();
        let mut next = prev.clone();
        change(&mut next);
        let next = next.sanitised();
        self.write(&mut file, &next)?;
        *current.lock().unwrap() = next.clone();
        Ok((prev, next))
    }

    fn write(&self, file: &mut FileState, settings: &Settings) -> io::Result<()> {
        let mut value = serde_json::to_value(settings)?;
        if let Value::Object(obj) = &mut value {
            file.restore_unread(obj);
        }
        crate::files::replace(&self.path, &serde_json::to_vec_pretty(&value)?)
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
                undo_counts_epoch: 0,
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

    /// Review #141: saves from many threads at once all succeed, none is lost, and the file
    /// ends up with what memory holds.
    #[test]
    fn concurrent_updates_are_serialised() {
        let dir = std::env::temp_dir().join(format!("unflub-settings-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = std::sync::Arc::new(Store::new(dir.clone()));
        let current = std::sync::Arc::new(Mutex::new(Settings::default()));
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let (store, current) = (store.clone(), current.clone());
                std::thread::spawn(move || {
                    for i in 0..300 {
                        store
                            .update(&current, |s| s.personal_words.push(format!("w{t}x{i}")))
                            .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let in_memory = current.lock().unwrap().clone();
        assert_eq!(in_memory.personal_words.len(), 8 * 300, "no update lost");
        assert_eq!(store.load(), in_memory, "disk and memory agree");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["settings.json"], "no temp file left");
        fs::remove_dir_all(dir).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("unflub-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("settings.json.bak-"))
            .collect();
        names.sort();
        names
    }

    /// Review #88: every corruption keeps its own backup, up to the last three.
    #[test]
    fn corrupt_files_fall_back_and_keep_the_last_three_backups() {
        let dir = temp_dir("settings-corrupt");
        let store = Store::new(dir.clone());
        for i in 0..5 {
            fs::write(dir.join("settings.json"), format!("{{ broken {i}")).unwrap();
            assert_eq!(store.load(), Settings::default());
            assert!(!dir.join("settings.json").exists(), "moved aside");
        }
        let kept = backups(&dir);
        assert_eq!(kept.len(), BACKUPS_KEPT);
        let contents: Vec<_> = kept
            .iter()
            .map(|n| fs::read_to_string(dir.join(n)).unwrap())
            .collect();
        assert_eq!(contents, ["{ broken 2", "{ broken 3", "{ broken 4"]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #88: a wrong-typed field resets only itself; personal words, shortcuts, excluded
    /// apps and the welcome stay.
    #[test]
    fn one_bad_field_keeps_the_rest() {
        let dir = temp_dir("settings-badfield");
        fs::write(
            dir.join("settings.json"),
            r#"{ "schemaVersion": 2, "onboarded": true, "confidence": "0.8", "theme": "ink",
                 "personalWords": ["rmgn"], "userRules": [["omw", "on my way"]],
                 "excludedApps": ["code.exe"], "pillOpacity": 0.5 }"#,
        )
        .unwrap();
        let store = Store::new(dir.clone());
        let s = store.load();
        assert!(s.onboarded);
        assert_eq!(s.confidence, Settings::default().confidence);
        assert_eq!(s.theme, Theme::Ink);
        assert_eq!(s.personal_words, ["rmgn"]);
        assert_eq!(s.user_rules, [("omw".to_string(), "on my way".to_string())]);
        assert_eq!(s.excluded_apps, ["code.exe"]);
        assert_eq!(s.pill_opacity, 0.5);
        assert!(backups(&dir).is_empty(), "nothing was lost, so no backup");
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #88: a file from a newer unflub (a beta, then back to stable) with an option this
    /// version doesn't know keeps everything, and saving writes that option back until the user
    /// picks another.
    #[test]
    fn an_unknown_option_from_a_newer_version_survives() {
        let dir = temp_dir("settings-newer");
        fs::write(
            dir.join("settings.json"),
            r#"{ "schemaVersion": 3, "onboarded": true, "theme": "midnight",
                 "personalWords": ["rmgn"], "futureThing": 1 }"#,
        )
        .unwrap();
        let store = Store::new(dir.clone());
        let current = Mutex::new(store.load());
        {
            let s = current.lock().unwrap();
            assert_eq!(s.theme, Theme::Unflub, "the default, for this version");
            assert_eq!(s.personal_words, ["rmgn"]);
            assert!(s.onboarded);
        }
        let on_disk = || -> Value {
            serde_json::from_slice(&fs::read(dir.join("settings.json")).unwrap()).unwrap()
        };
        store.update(&current, |s| s.enabled = false).unwrap();
        let written = on_disk();
        assert_eq!(written["theme"], "midnight");
        assert_eq!(written["schemaVersion"], 3);
        assert_eq!(written["futureThing"], 1);
        assert_eq!(written["enabled"], false);
        // Loading it again gives the same settings.
        assert_eq!(Store::new(dir.clone()).load(), *current.lock().unwrap());
        // The user picks a theme here: that choice wins from now on.
        store.update(&current, |s| s.theme = Theme::Paper).unwrap();
        store.update(&current, |s| s.theme = Theme::Unflub).unwrap();
        assert_eq!(on_disk()["theme"], "unflub");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_utf8_bom_is_fine() {
        let dir = temp_dir("settings-bom");
        fs::write(
            dir.join("settings.json"),
            "\u{feff}{ \"schemaVersion\": 2, \"personalWords\": [\"rmgn\"] }",
        )
        .unwrap();
        assert_eq!(Store::new(dir.clone()).load().personal_words, ["rmgn"]);
        assert!(backups(&dir).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
