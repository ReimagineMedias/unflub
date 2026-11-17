//! User-facing behaviour settings. Serialised by the app; plain data here.

/// Who decides that a word is wrong and what to replace it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum EngineMode {
    /// The OS checker flags the word and the bundled engine agrees on a confident fix. Default.
    #[default]
    Both,
    /// Only the OS spell checker (its own auto-replace list plus confident bundled ranking of
    /// words it flags).
    SystemOnly,
    /// Only the bundled dictionary. Same behaviour on every OS.
    BundledOnly,
    /// Either engine may flag the word (the OS checker, or a word the bundled dictionary doesn't
    /// know), but the fix still has to be a confident one, as in `Both`.
    Balanced,
    /// Take the best candidate whenever any engine flags the word. Phone-like, more wrong fixes.
    Aggressive,
}

/// What Backspace right after a fix does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum UndoMode {
    /// Default. While the pill is up, the first Backspace is held (nothing is deleted) and the
    /// pill asks "Undo?" until answered (no timeout); a second Backspace undoes the fix. Any
    /// other key sends the held Backspace first, so deleting the space to add a comma still
    /// works.
    #[default]
    Twice,
    /// The first Backspace right after a fix undoes it (the old behaviour).
    Once,
    /// Backspace is always just Backspace.
    Off,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Config {
    pub enabled: bool,
    pub mode: EngineMode,
    /// 0.0..=1.0. Higher means fewer, safer corrections.
    pub confidence: f32,
    /// Words shorter than this are never corrected.
    pub min_word_len: usize,
    /// Skip tokens that look like code: camelCase, snake_case, paths, URLs, words in backticks.
    pub skip_code: bool,
    /// Lowercased executable or bundle ids where the engine stays off.
    pub excluded_apps: Vec<String>,
    /// Words never corrected (personal dictionary).
    pub personal_words: Vec<String>,
    /// Exact user rules, typed -> replacement (abbreviations and learned fixes).
    pub user_rules: Vec<(String, String)>,
    /// How Backspace undoes a fix.
    pub undo_mode: UndoMode,
    /// A typo becomes "never fix" after its fix was undone this many times (1 to 5); 0 never.
    pub learn_after_undos: u8,
    /// Undos counted so far, `(typo, fix, times)` lowercased, kept by the app between runs. The
    /// engine counts on from the highest of these and its own.
    pub undo_counts: Vec<(String, String, u32)>,
    /// Bumped by the app when the user clears their history: the engine then forgets the undo
    /// counts it holds before taking `undo_counts` (which otherwise only ever raise its own).
    pub undo_counts_epoch: u64,
    /// How long after a fix the double-Backspace undo stays open (the pill's time on screen).
    pub undo_window_ms: u32,
    /// BCP 47 tag for the system spell checker (e.g. `"en-US"`). The bundled dictionary is
    /// English only: for any other language its candidates are off (only the OS checker's own
    /// fixes apply) unless [`Config::english_for_other_languages`] or `BundledOnly`.
    pub language: String,
    /// Use the bundled English dictionary's fixes even though `language` isn't English. Off by
    /// default: it rewrites words of other languages into English (`bien` to `been`).
    pub english_for_other_languages: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: EngineMode::Both,
            confidence: 0.6,
            min_word_len: 2,
            skip_code: true,
            excluded_apps: Vec::new(),
            personal_words: Vec::new(),
            user_rules: Vec::new(),
            undo_mode: UndoMode::Twice,
            learn_after_undos: 2,
            undo_counts: Vec::new(),
            undo_counts_epoch: 0,
            undo_window_ms: 1100,
            language: "en-US".to_string(),
            english_for_other_languages: false,
        }
    }
}

/// Is `tag` (BCP 47) English? An empty tag counts as English, the bundled dictionary's language.
pub fn is_english(tag: &str) -> bool {
    let primary = tag.split(['-', '_']).next().unwrap_or("");
    primary.is_empty() || primary.eq_ignore_ascii_case("en")
}

#[cfg(test)]
mod lang_tests {
    #[test]
    fn english_tags() {
        for tag in ["en", "en-US", "EN-gb", "en_AU", ""] {
            assert!(super::is_english(tag), "{tag:?}");
        }
        for tag in ["fr-FR", "es", "de-DE", "eng", "pt-BR"] {
            assert!(!super::is_english(tag), "{tag:?}");
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn config_is_serde() {
        fn assert_serde<T: serde::Serialize + serde::de::DeserializeOwned>() {}
        assert_serde::<Config>();
        assert_serde::<EngineMode>();
    }
}
