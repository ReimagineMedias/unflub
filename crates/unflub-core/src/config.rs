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
    /// BCP 47 tag for the system spell checker (e.g. `"en-US"`). The bundled engine is English
    /// only for now and ignores this.
    pub language: String,
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
