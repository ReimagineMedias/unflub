//! Persisted user settings. The app owns this file; the engine gets a `unflub_core::Config` from it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::SystemTime,
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
    /// Windows pause shortcut. Empty means off.
    pub pause_shortcut: String,
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
    /// Keyed like the engine's dictionary. Null means added by the user, a day means learned.
    /// Older entries have no record of where they came from.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub word_sources: BTreeMap<String, Option<i64>>,
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
const MAX_SETTINGS_BYTES: u64 = 8 * 1024 * 1024;

impl Default for Settings {
    fn default() -> Self {
        let core = unflub_core::Config::default();
        Self {
            schema_version: SCHEMA_VERSION,
            enabled: core.enabled,
            pause_shortcut: String::new(),
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
            word_sources: BTreeMap::new(),
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

/// Longest personal word or shortcut, in characters. Windows' spell checker calls any word of 64+
/// UTF-16 units correct anyway. Keep `LIMITS` in `app/src/types.ts` the same.
pub const MAX_WORD_CHARS: usize = 64;
/// Longest text a shortcut becomes, in characters.
pub const MAX_REPLACEMENT_CHARS: usize = 256;
/// Longest excluded app name (a Windows path is at most 260).
pub const MAX_APP_CHARS: usize = 260;
pub const MAX_PERSONAL_WORDS: usize = 5000;
pub const MAX_USER_RULES: usize = 1000;
pub const MAX_EXCLUDED_APPS: usize = 500;

/// `text` is 1 to `max` characters with no control characters (`allow` excepted).
fn fits(text: &str, max: usize, allow: &[char]) -> bool {
    !text.is_empty()
        && text.chars().count() <= max
        && !text.chars().any(|c| c.is_control() && !allow.contains(&c))
}

pub(crate) fn word_key(word: &str) -> String {
    word.trim().to_lowercase().replace('\u{2019}', "'")
}

impl Settings {
    /// Apply only the fields supplied by the caller. Older or newer versions of the file are
    /// handled at startup, not by the live editor.
    pub fn patch(&mut self, mut patch: Map<String, Value>, from_file: bool) -> io::Result<()> {
        let mut fields = serde_json::to_value(&*self)?
            .as_object()
            .cloned()
            .ok_or_else(|| io::Error::other("Couldn't read the current settings."))?;
        let known = serde_json::to_value(Self::default())?;
        if let Some(version) = patch.remove("schemaVersion") {
            if !from_file || version.as_u64() != Some(self.schema_version.into()) {
                return Err(invalid(
                    "schemaVersion cannot be changed while unflub is running.",
                ));
            }
        }
        for (key, value) in patch {
            if known.get(&key).is_none() {
                if !from_file {
                    return Err(invalid("Unknown settings key."));
                }
            } else {
                let field = Map::from_iter([(key.clone(), value.clone())]);
                if serde_json::from_value::<Self>(Value::Object(field)).is_err() {
                    return Err(invalid(format!("Invalid value for {key}.")));
                }
            }
            fields.insert(key, value);
        }
        *self = serde_json::from_value(Value::Object(fields))?;
        Ok(())
    }

    fn checked(mut self, previous: &Self, info: crate::AppInfo) -> io::Result<Self> {
        // Normalise spelling before comparing with the recovery sanitizer. Losing entries or
        // clamping a value is appropriate for startup recovery, not a live settings change.
        self.pause_shortcut = crate::controls::normalise_shortcut(&self.pause_shortcut)
            .ok_or_else(|| {
                invalid("Invalid pauseShortcut. Use two modifiers and a letter or digit.")
            })?;
        self.language = self.language.trim().into();
        for app in &mut self.excluded_apps {
            *app = app.trim().to_lowercase();
        }
        self.excluded_apps.dedup();
        // Word sources follow the dictionary. A CLI or file edit can remove a word
        // without updating its metadata, so prune it before checking the user fields.
        let words: HashSet<_> = self
            .personal_words
            .iter()
            .map(|word| word_key(word))
            .collect();
        self.word_sources.retain(|key, _| words.contains(key));
        let clean = self.clone().sanitised();
        if clean != self {
            let Value::Object(input) = serde_json::to_value(&self)? else {
                return Err(io::Error::other("Couldn't read the settings."));
            };
            let clean = serde_json::to_value(clean)?;
            if let Some((key, _)) = input
                .iter()
                .find(|(key, value)| clean.get(*key) != Some(*value))
            {
                return Err(invalid(format!(
                    "Invalid value or limit exceeded for {key}."
                )));
            }
            return Err(invalid("Invalid settings."));
        }
        if self.personal_words.iter().any(|word| {
            word.contains(char::is_whitespace) && !previous.personal_words.contains(word)
        }) {
            return Err(invalid("Each personalWords entry must be a single word."));
        }
        if self.excluded_apps.iter().any(|app| {
            (!app.ends_with(".exe")
                || app.len() == 4
                || app.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']))
                && !previous.excluded_apps.contains(app)
        }) {
            return Err(invalid(
                "Each excludedApps entry must be an executable name ending in .exe.",
            ));
        }
        let policy = info.managed;
        for (locked, changed, key) in [
            (
                policy.disable_updates,
                self.auto_update != previous.auto_update,
                "autoUpdate",
            ),
            (
                policy.disable_updates,
                self.channel != previous.channel,
                "channel",
            ),
            (
                policy.disable_usage_stats,
                self.share_usage_stats != previous.share_usage_stats,
                "shareUsageStats",
            ),
            (
                policy.disable_crash_reports,
                self.send_crash_reports != previous.send_crash_reports,
                "sendCrashReports",
            ),
        ] {
            if locked && changed {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{key} is managed by your organization."),
                ));
            }
        }
        if !info.login_allowed && self.start_at_login != previous.start_at_login {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Start at login is available in the installed version.",
            ));
        }
        Ok(self)
    }

    /// Clamp values a hand-edited file could break. The lists are capped so the settings always
    /// fit in one message to the engine (`unflub_engine::frame::MAX_FRAME`), whatever was pasted.
    pub fn sanitised(mut self) -> Self {
        self.pause_shortcut =
            crate::controls::normalise_shortcut(&self.pause_shortcut).unwrap_or_default();
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.pill_opacity = self.pill_opacity.clamp(0.3, 1.0);
        self.pill_duration_ms = self.pill_duration_ms.clamp(300, 5000);
        self.min_word_len = self.min_word_len.clamp(1, 12);
        self.learn_after_undos = self.learn_after_undos.min(5);
        for app in &mut self.excluded_apps {
            *app = app.trim().to_lowercase();
        }
        self.excluded_apps.retain(|a| fits(a, MAX_APP_CHARS, &[]));
        self.excluded_apps.dedup();
        self.excluded_apps.truncate(MAX_EXCLUDED_APPS);
        let mut words = HashSet::new();
        self.personal_words.retain(|w| {
            words.len() < MAX_PERSONAL_WORDS
                && fits(w, MAX_WORD_CHARS, &[])
                && words.insert(word_key(w))
        });
        self.word_sources.retain(|key, _| words.contains(key));
        self.user_rules.retain(|(from, to)| {
            fits(from, MAX_WORD_CHARS, &[]) && fits(to, MAX_REPLACEMENT_CHARS, &['\n', '\t'])
        });
        self.user_rules.truncate(MAX_USER_RULES);
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

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
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
    /// The file couldn't be read at startup, so the settings in memory are defaults: back the
    /// file up before the first save replaces it.
    unreadable: bool,
    /// What `privacy-switches.json` holds, once this run has written it.
    switches: Option<PrivacySwitches>,
    seen: Option<FileStamp>,
    invalid_edit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    bytes: u64,
}

fn stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(FileStamp {
            modified: metadata.modified()?,
            bytes: metadata.len(),
        })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(invalid("settings.json exceeds 8 MiB."));
    }
    Ok(bytes)
}

/// The file's bytes. A file another program holds open for a moment (antivirus, a sync client)
/// gets a few more tries.
fn read_retrying(path: &Path) -> io::Result<Vec<u8>> {
    let mut tries = 0;
    loop {
        match read_bounded(path) {
            Err(e)
                if !matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) && tries < 4 =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            read => return read,
        }
    }
}

/// The file's text: UTF-8 with or without a BOM, or UTF-16 with a BOM (Notepad's "Unicode").
fn decode(bytes: &[u8]) -> Option<String> {
    let utf16 = |bytes: &[u8], from: fn([u8; 2]) -> u16| {
        let pairs = bytes.chunks_exact(2);
        if !pairs.remainder().is_empty() {
            return None;
        }
        let units: Vec<u16> = pairs.map(|c| from([c[0], c[1]])).collect();
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
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

/// The usage-stats and crash-report switches as last saved, in a file of their own
/// (`privacy-switches.json`) so that an off survives anything that happens to settings.json:
/// corruption, a field this version can't read, a file another program holds. A switch that is
/// off in either file loads as off. New installs have neither file and start with both on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PrivacySwitches {
    share_usage_stats: bool,
    send_crash_reports: bool,
}

impl Default for PrivacySwitches {
    fn default() -> Self {
        Self::of(&Settings::default())
    }
}

impl PrivacySwitches {
    fn of(settings: &Settings) -> Self {
        Self {
            share_usage_stats: settings.share_usage_stats,
            send_crash_reports: settings.send_crash_reports,
        }
    }

    fn read(path: &Path) -> Option<Self> {
        let bytes = read_retrying(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn write(&self, path: &Path) -> io::Result<()> {
        crate::files::replace(path, &serde_json::to_vec_pretty(self)?)
    }
}

pub struct Store {
    path: PathBuf,
    privacy_path: PathBuf,
    /// Held for every write, and across read, change and write in [`Store::update`].
    writing: Mutex<FileState>,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("settings.json"),
            privacy_path: dir.join("privacy-switches.json"),
            writing: Mutex::new(FileState::default()),
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// A missing file gives the defaults. A file that can't be opened (another program holds
    /// it) also runs on the defaults, but is left alone and backed up before the first save
    /// replaces it. A file that isn't a JSON object is moved to a timestamped backup (the last
    /// [`BACKUPS_KEPT`] are kept). A field this version can't read gets its default and the
    /// rest of the file is kept (see [`from_object`]). An older file is migrated
    /// (`migrations.rs`) and written back. Whatever happened to the file, usage stats and crash
    /// reports stay off if the user turned them off ([`PrivacySwitches`]).
    pub fn load(&self) -> Settings {
        let (mut settings, migrated_from, lost) = self.read_file();
        match PrivacySwitches::read(&self.privacy_path) {
            Some(saved) => {
                self.lock().switches = Some(saved);
                settings.share_usage_stats &= saved.share_usage_stats;
                settings.send_crash_reports &= saved.send_crash_reports;
            }
            // Written before settings.json ever changes in this version, so an off already in
            // the file is kept too.
            None => {
                // The first start of this version, and the file couldn't tell us what the user
                // chose: off, rather than an on they may have turned off.
                if lost.share_usage_stats {
                    settings.share_usage_stats = false;
                }
                if lost.send_crash_reports {
                    settings.send_crash_reports = false;
                }
                let switches = PrivacySwitches::of(&settings);
                match switches.write(&self.privacy_path) {
                    Ok(()) => self.lock().switches = Some(switches),
                    Err(e) => eprintln!("settings: can't save the privacy switches: {e}"),
                }
            }
        }
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

    /// The settings in the file, the schema version they were migrated from, and which privacy
    /// switches the file holds but couldn't be read (marked `true`).
    fn read_file(&self) -> (Settings, Option<u32>, PrivacySwitches) {
        self.lock().seen = stamp(&self.path).ok().flatten();
        let none_lost = PrivacySwitches {
            share_usage_stats: false,
            send_crash_reports: false,
        };
        let all_lost = PrivacySwitches {
            share_usage_stats: true,
            send_crash_reports: true,
        };
        let bytes = match read_retrying(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (Settings::default(), None, none_lost)
            }
            Err(e) => {
                eprintln!("settings: can't read the file ({e}); using defaults, and backing it up before saving over it");
                self.lock().unreadable = true;
                return (Settings::default(), None, all_lost);
            }
        };
        let parsed = decode(&bytes)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
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
                let lost = PrivacySwitches {
                    share_usage_stats: unread.contains_key("shareUsageStats"),
                    send_crash_reports: unread.contains_key("sendCrashReports"),
                };
                self.lock().unread = unread;
                (settings, migrated_from, lost)
            }
            None => {
                match self.back_up(|from, to| fs::rename(from, to)) {
                    Ok(to) => {
                        self.lock().seen = None;
                        eprintln!("settings: unreadable, moved to {}", to.display());
                    }
                    Err(e) => {
                        eprintln!("settings: unreadable, and can't move it aside yet: {e}");
                        self.lock().unreadable = true;
                    }
                }
                (Settings::default(), None, all_lost)
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
        self.update_with(current, |_, next| {
            change(next);
            Ok(next.clone().sanitised())
        })
    }

    pub fn update_checked(
        &self,
        current: &Mutex<Settings>,
        info: crate::AppInfo,
        change: impl FnOnce(&mut Settings) -> io::Result<()>,
    ) -> io::Result<(Settings, Settings)> {
        self.update_with(current, |prev, next| {
            change(next)?;
            next.clone().checked(prev, info)
        })
    }

    fn update_with(
        &self,
        current: &Mutex<Settings>,
        change: impl FnOnce(&Settings, &mut Settings) -> io::Result<Settings>,
    ) -> io::Result<(Settings, Settings)> {
        let mut file = self.lock();
        let prev = current.lock().unwrap().clone();
        let mut next = prev.clone();
        let next = change(&prev, &mut next)?;
        if !file.unreadable && stamp(&self.path)? != file.seen {
            return Err(io::Error::other(
                "settings.json has a pending edit. Try again shortly.",
            ));
        }
        if file.invalid_edit {
            return Err(invalid(
                "Fix the invalid edit in settings.json before saving.",
            ));
        }
        self.write(&mut file, &next)?;
        *current.lock().unwrap() = next.clone();
        Ok((prev, next))
    }

    pub fn reload_pending(&self) -> io::Result<bool> {
        let file = self.lock();
        Ok(stamp(&self.path)? != file.seen)
    }

    /// Accept a stable editor save. A bad edit leaves the last working settings in effect.
    /// Remember that file revision to report the error once.
    pub fn reload(
        &self,
        current: &Mutex<Settings>,
        info: crate::AppInfo,
    ) -> io::Result<Option<(Settings, Settings)>> {
        let mut file = self.lock();
        let changed = stamp(&self.path)?;
        if changed == file.seen {
            return Ok(None);
        }
        let previous_stamp = file.seen;
        if changed.is_none() {
            file.seen = None;
            file.invalid_edit = false;
            return Err(invalid("settings.json was removed."));
        }
        let bytes = read_bounded(&self.path);
        if stamp(&self.path)? != changed {
            return Ok(None);
        }
        let bytes = match bytes {
            Err(e) if e.kind() != io::ErrorKind::InvalidData => return Err(e),
            result => {
                file.seen = changed;
                file.invalid_edit = true;
                result?
            }
        };
        let text =
            decode(&bytes).ok_or_else(|| invalid("settings.json must use UTF-8 or UTF-16."))?;
        let patch: Map<String, Value> = serde_json::from_str(&text).map_err(|e| {
            invalid(format!(
                "Invalid settings.json at line {}, column {}.",
                e.line(),
                e.column()
            ))
        })?;
        let prev = current.lock().unwrap().clone();
        let mut next = prev.clone();
        next.patch(patch.clone(), true)?;
        let next = next.checked(&prev, info)?;
        if stamp(&self.path)? != changed {
            return Ok(None);
        }
        // A valid patch is saved as the complete settings, so omitted fields survive a restart.
        let saved_unread = file.unread.clone();
        let was_unreadable = file.unreadable;
        file.unreadable = false;
        file.unread.retain(|key, _| !patch.contains_key(key));
        if let Err(e) = self.write(&mut file, &next) {
            file.unread = saved_unread;
            file.unreadable = was_unreadable;
            file.seen = previous_stamp;
            return Err(e);
        }
        file.invalid_edit = false;
        *current.lock().unwrap() = next.clone();
        Ok(Some((prev, next)))
    }

    fn write(&self, file: &mut FileState, settings: &Settings) -> io::Result<()> {
        let mut value = serde_json::to_value(settings)?;
        if let Value::Object(obj) = &mut value {
            file.restore_unread(obj);
        }
        let bytes = serde_json::to_vec_pretty(&value)?;
        if bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(invalid("settings.json exceeds 8 MiB."));
        }
        if file.unreadable {
            // Never replace a file we couldn't read without a copy of it.
            match self.back_up(|from, to| fs::copy(from, to).map(drop)) {
                Ok(to) => eprintln!("settings: the unreadable file is kept as {}", to.display()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!(
                            "settings.json can't be read or backed up, so it isn't replaced ({e})"
                        ),
                    ))
                }
            }
            file.unreadable = false;
        }
        // First, so the file can't say "on" while this still says "off" from before.
        let before = file.switches;
        let switches = PrivacySwitches::of(settings);
        if file.switches != Some(switches) {
            match switches.write(&self.privacy_path) {
                Ok(()) => file.switches = Some(switches),
                Err(e) => eprintln!("settings: can't save the privacy switches: {e}"),
            }
        }
        let written = crate::files::replace(&self.path, &bytes);
        if written.is_ok() {
            file.seen = stamp(&self.path).ok().flatten();
        }
        if written.is_err() && file.switches != before {
            // The settings didn't change, so neither do the switches: the file must keep
            // matching memory, or the next save would write memory's value as a new choice.
            match before.map(|b| b.write(&self.privacy_path).map(|()| b)) {
                Some(Ok(b)) => file.switches = Some(b),
                _ => file.switches = None,
            }
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> crate::AppInfo {
        crate::AppInfo {
            managed: crate::managed::Managed::default(),
            install_source: crate::install::InstallSource::Installer,
            login_allowed: true,
        }
    }

    #[test]
    fn bad_live_values_never_reach_memory_or_disk() {
        let dir = temp_dir("settings-checked");
        let store = Store::new(dir.clone());
        let initial = Settings::default();
        store.save(&initial).unwrap();
        let current = Mutex::new(initial.clone());
        let original = fs::read(&store.path).unwrap();
        let mut info = installed();
        info.managed.disable_updates = true;
        info.managed.disable_usage_stats = true;
        for (key, value) in [
            ("enabled", serde_json::json!("false")),
            (
                "personalWords",
                serde_json::json!(["a".repeat(MAX_WORD_CHARS + 1)]),
            ),
            ("personalWords", serde_json::json!(["two words"])),
            ("excludedApps", serde_json::json!(["C:\\bad.exe"])),
            ("pillDurationMs", serde_json::json!(299)),
            ("pauseShortcut", serde_json::json!("Ctrl+A")),
            ("shareUsageStats", serde_json::json!(false)),
            ("channel", serde_json::json!("beta")),
            ("schemaVersion", serde_json::json!(SCHEMA_VERSION)),
            ("enabeld", serde_json::json!(false)),
        ] {
            let patch = Map::from_iter([(key.to_owned(), value)]);
            assert!(
                store
                    .update_checked(&current, info, |s| s.patch(patch, false))
                    .is_err(),
                "{key}"
            );
            assert_eq!(*current.lock().unwrap(), initial);
            assert_eq!(fs::read(&store.path).unwrap(), original);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_words_through_cli_and_file_patches_prunes_their_sources() {
        let dir = temp_dir("settings-remove-word-sources");
        let store = Store::new(dir.clone());
        let initial = Settings {
            personal_words: vec!["CLI".into(), "Editor".into(), "O’Clock".into()],
            word_sources: [
                ("cli".into(), None),
                ("editor".into(), Some(7)),
                ("o'clock".into(), Some(42)),
            ]
            .into(),
            ..Settings::default()
        };
        store.save(&initial).unwrap();
        let current = Mutex::new(initial);
        let info = installed();
        let cli_patch = Map::from_iter([(
            "personalWords".into(),
            serde_json::json!(["Editor", "O’Clock"]),
        )]);
        let (_, saved) = store
            .update_checked(&current, info, |s| s.patch(cli_patch, false))
            .unwrap();
        assert!(!saved.word_sources.contains_key("cli"));
        assert_eq!(saved.word_sources["editor"], Some(7));

        crate::files::replace(
            &store.path,
            br#"{"personalWords":["O'CLOCK"],"wordSources":{"editor":7,"o'clock":42}}"#,
        )
        .unwrap();
        let (_, saved) = store.reload(&current, info).unwrap().unwrap();
        assert_eq!(saved.personal_words, ["O'CLOCK"]);
        assert_eq!(saved.word_sources, [("o'clock".into(), Some(42))].into());
        let (_, saved) = store
            .update_checked(&current, info, |s| {
                s.patch(
                    Map::from_iter([("enabled".into(), Value::Bool(false))]),
                    false,
                )
            })
            .unwrap();
        assert!(!saved.enabled);
        assert_eq!(Store::new(dir.clone()).load(), saved);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reload_keeps_the_last_good_settings_and_does_not_loop_on_its_own_save() {
        let dir = temp_dir("settings-reload");
        let store = Store::new(dir.clone());
        let initial = Settings {
            personal_words: vec!["unflub".into()],
            ..Settings::default()
        };
        store.save(&initial).unwrap();
        let current = Mutex::new(initial.clone());
        let mut info = installed();
        info.managed.disable_usage_stats = true;
        for bytes in [
            br#"{"enabled":"#.as_slice(),
            br#"{"enabled":false,"shareUsageStats":false}"#,
            br#"{"enabled":false,"pillDurationMs":6000}"#,
        ] {
            crate::files::replace(&store.path, bytes).unwrap();
            assert!(store.reload_pending().unwrap());
            assert!(store.update_checked(&current, info, |_| Ok(())).is_err());
            assert!(store.reload(&current, info).is_err());
            assert!(!store.reload_pending().unwrap());
            assert!(store.update_checked(&current, info, |_| Ok(())).is_err());
            assert_eq!(*current.lock().unwrap(), initial);
            assert_eq!(fs::read(&store.path).unwrap(), bytes);
        }

        let edit = br#"{"enabled":false,"excludedApps":[" Code.EXE "],"newerPreference":42}"#;
        crate::files::replace(&store.path, edit).unwrap();
        let (before, saved) = store.reload(&current, info).unwrap().unwrap();
        assert_eq!(before, initial);
        assert!(!saved.enabled);
        assert_eq!(saved.excluded_apps, ["code.exe"]);
        assert_eq!(saved.personal_words, initial.personal_words);
        assert_eq!(saved.extra["newerPreference"], 42);
        assert_eq!(*current.lock().unwrap(), saved);
        assert_eq!(Store::new(dir.clone()).load(), saved);
        assert!(store.reload(&current, info).unwrap().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

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

    /// Review #85: an oversized personal word or list never stops the settings reaching the
    /// engine, which takes at most `MAX_FRAME` bytes in one message.
    #[test]
    fn the_largest_settings_still_fit_one_engine_message() {
        // Four bytes in UTF-8 each, the most a character takes.
        let wide = |n: usize, tag: usize| format!("{tag}{}", "\u{1D4B3}".repeat(n - 6));
        let s = Settings {
            personal_words: (0..MAX_PERSONAL_WORDS + 50)
                .map(|i| wide(MAX_WORD_CHARS, 100_000 + i))
                .collect(),
            user_rules: (0..MAX_USER_RULES + 50)
                .map(|i| {
                    (
                        wide(MAX_WORD_CHARS, 100_000 + i),
                        wide(MAX_REPLACEMENT_CHARS, 100_000 + i),
                    )
                })
                .collect(),
            excluded_apps: (0..MAX_EXCLUDED_APPS + 50)
                .map(|i| wide(MAX_APP_CHARS, 100_000 + i))
                .collect(),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.personal_words.len(), MAX_PERSONAL_WORDS);
        assert_eq!(s.user_rules.len(), MAX_USER_RULES);
        assert_eq!(s.excluded_apps.len(), MAX_EXCLUDED_APPS);
        // Undo counts are capped at 500 pairs (`undo_counts.rs`).
        let undo_counts = (0..500)
            .map(|i| {
                (
                    wide(MAX_WORD_CHARS, 100_000 + i),
                    wide(MAX_WORD_CHARS, 200_000 + i),
                    9,
                )
            })
            .collect();
        let settings = crate::engine::engine_settings(&s, &Default::default(), undo_counts);
        let mut out = Vec::new();
        unflub_engine::frame::write(
            &mut out,
            &unflub_engine::protocol::ToEngine::Configure { settings },
        )
        .expect("fits one message");

        let s = Settings {
            personal_words: vec![
                "rmgn".into(),
                "x".repeat(5 << 20),
                "".into(),
                "a\u{0}b".into(),
            ],
            user_rules: vec![
                ("omw".into(), "on my way".into()),
                ("addr".into(), "1 Main St\nOttawa".into()),
                ("big".into(), "y".repeat(MAX_REPLACEMENT_CHARS + 1)),
            ],
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.personal_words, ["rmgn"], "a 5 MB word is dropped");
        assert_eq!(s.user_rules.len(), 2);
    }

    /// The window's input limits match these.
    #[test]
    fn the_window_uses_the_same_limits() {
        let types = include_str!("../../src/types.ts");
        let want = format!(
            "export const LIMITS = {{ word: {MAX_WORD_CHARS}, replacement: {MAX_REPLACEMENT_CHARS}, app: {MAX_APP_CHARS}, words: {MAX_PERSONAL_WORDS}, rules: {MAX_USER_RULES} }} as const;"
        );
        assert!(types.contains(&want), "{want}");
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
        let temp_files = fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "tmp")
            })
            .count();
        assert_eq!(temp_files, 0, "no temp file left");
        fs::remove_dir_all(dir).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("unflub-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What a file that couldn't be read loads as: the defaults, with usage stats and crash
    /// reports off since the file may have held an off (#134).
    fn lost_file_defaults() -> Settings {
        Settings {
            share_usage_stats: false,
            send_crash_reports: false,
            ..Settings::default()
        }
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
            assert_eq!(store.load(), lost_file_defaults());
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

    /// Review #87: Notepad's "Unicode" (UTF-16 with a BOM) loads.
    #[test]
    fn utf16_files_load() {
        let dir = temp_dir("settings-utf16");
        let json = r#"{ "schemaVersion": 2, "onboarded": true, "personalWords": ["rmgn"], "theme": "ink" }"#;
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(json.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        fs::write(dir.join("settings.json"), le).unwrap();
        let s = Store::new(dir.clone()).load();
        assert_eq!(
            (s.personal_words.as_slice(), s.theme),
            (&["rmgn".to_string()][..], Theme::Ink)
        );
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(json.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        fs::write(dir.join("settings.json"), be).unwrap();
        assert!(Store::new(dir.clone()).load().onboarded);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #87: a file that isn't text is moved aside, not overwritten.
    #[test]
    fn a_file_that_isnt_text_is_backed_up() {
        let dir = temp_dir("settings-binary");
        fs::write(dir.join("settings.json"), [0x7B, 0xC3, 0x28, 0x7D]).unwrap();
        assert_eq!(Store::new(dir.clone()).load(), lost_file_defaults());
        let kept = backups(&dir);
        assert_eq!(kept.len(), 1);
        assert_eq!(
            fs::read(dir.join(&kept[0])).unwrap(),
            [0x7B, 0xC3, 0x28, 0x7D]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #87: a file another program holds open at startup is never replaced without a
    /// backup: saves fail while it can't be copied, and the first one after keeps a copy.
    #[cfg(windows)]
    #[test]
    fn a_locked_file_is_backed_up_before_it_is_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("settings-locked");
        let original = r#"{ "schemaVersion": 2, "personalWords": ["rmgn"] }"#;
        fs::write(dir.join("settings.json"), original).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.join("settings.json"))
            .unwrap();
        let store = Store::new(dir.clone());
        let current = Mutex::new(store.load());
        assert_eq!(*current.lock().unwrap(), lost_file_defaults());
        assert!(store.update(&current, |s| s.enabled = false).is_err());
        assert!(current.lock().unwrap().enabled, "nothing changed");
        drop(lock);
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            original
        );
        store.update(&current, |s| s.enabled = false).unwrap();
        let kept = backups(&dir);
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read_to_string(dir.join(&kept[0])).unwrap(), original);
        // Only the first save backs up.
        store.update(&current, |s| s.enabled = true).unwrap();
        assert_eq!(backups(&dir).len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #134: once the user turns usage stats or crash reports off, no corruption,
    /// unreadable field or missing file turns them back on. New installs start with both on.
    #[test]
    fn privacy_opt_outs_survive_a_reset() {
        let dir = temp_dir("settings-optout");
        let fresh = Store::new(dir.clone()).load();
        assert!(
            fresh.share_usage_stats && fresh.send_crash_reports,
            "on for new installs"
        );

        let store = Store::new(dir.clone());
        let current = Mutex::new(store.load());
        store
            .update(&current, |s| {
                s.share_usage_stats = false;
                s.send_crash_reports = false;
            })
            .unwrap();
        let off = |s: Settings| !s.share_usage_stats && !s.send_crash_reports;
        for broken in [
            "{ not json",
            r#"{ "schemaVersion": 2, "shareUsageStats": "no", "sendCrashReports": 0 }"#,
            r#"{ "schemaVersion": 9 }"#,
        ] {
            fs::write(dir.join("settings.json"), broken).unwrap();
            assert!(off(Store::new(dir.clone()).load()), "{broken}");
        }
        let _ = fs::remove_file(dir.join("settings.json"));
        assert!(off(Store::new(dir.clone()).load()), "file deleted");

        // Turning one back on is the user's choice too.
        let store = Store::new(dir.clone());
        let current = Mutex::new(store.load());
        store
            .update(&current, |s| s.share_usage_stats = true)
            .unwrap();
        let again = Store::new(dir.clone()).load();
        assert!(again.share_usage_stats && !again.send_crash_reports);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #134: an off saved by a version before `privacy-switches.json` is kept from the
    /// first start of this one, before anything is saved.
    #[test]
    fn an_older_opt_out_is_kept_from_the_first_start() {
        let dir = temp_dir("settings-optout-upgrade");
        fs::write(
            dir.join("settings.json"),
            r#"{ "schemaVersion": 2, "shareUsageStats": false, "sendCrashReports": true }"#,
        )
        .unwrap();
        assert!(!Store::new(dir.clone()).load().share_usage_stats);
        fs::write(dir.join("settings.json"), "{ broken").unwrap();
        let s = Store::new(dir.clone()).load();
        assert!(!s.share_usage_stats && s.send_crash_reports);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #134: on the first start of this version, a file that can't say what the user
    /// chose loads those switches as off, never as the default on.
    #[test]
    fn an_unreadable_choice_on_the_first_start_loads_as_off() {
        let dir = temp_dir("settings-optout-lost");
        fs::write(dir.join("settings.json"), "{ broken").unwrap();
        let s = Store::new(dir.clone()).load();
        assert!(
            !s.share_usage_stats && !s.send_crash_reports,
            "corrupt file"
        );
        fs::remove_dir_all(&dir).unwrap();

        let dir = temp_dir("settings-optout-lost-field");
        fs::write(
            dir.join("settings.json"),
            r#"{ "schemaVersion": 2, "shareUsageStats": "no" }"#,
        )
        .unwrap();
        let s = Store::new(dir.clone()).load();
        assert!(
            !s.share_usage_stats && s.send_crash_reports,
            "one bad field"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #134: a save that fails leaves the switches as they were, so memory and the
    /// switches file agree and the next save doesn't record the old value as a new choice.
    #[cfg(windows)]
    #[test]
    fn a_failed_save_leaves_the_privacy_switches_alone() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("settings-optout-failed");
        let store = Store::new(dir.clone());
        let current = Mutex::new(store.load());
        store.save(&current.lock().unwrap()).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.join("settings.json"))
            .unwrap();
        assert!(store
            .update(&current, |s| s.share_usage_stats = false)
            .is_err());
        drop(lock);
        let on_disk = PrivacySwitches::read(&dir.join("privacy-switches.json")).unwrap();
        assert!(on_disk.share_usage_stats, "rolled back with the settings");
        assert!(current.lock().unwrap().share_usage_stats);
        // A retry works and sticks.
        store
            .update(&current, |s| s.share_usage_stats = false)
            .unwrap();
        store.update(&current, |s| s.enabled = false).unwrap();
        assert!(!Store::new(dir.clone()).load().share_usage_stats);
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
