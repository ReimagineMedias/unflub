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
    /// Backspace right after a correction restores the original and stops correcting that word.
    pub undo_learns: bool,
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
            undo_learns: true,
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
