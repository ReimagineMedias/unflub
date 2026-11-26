//! Add a reviewed batch of personal words and shortcuts without replacing existing entries.

use crate::settings::{
    fits, word_key, Settings, MAX_PERSONAL_WORDS, MAX_REPLACEMENT_CHARS, MAX_USER_RULES,
    MAX_WORD_CHARS,
};
use serde_json::{json, Map};
use std::{collections::HashSet, io};

#[derive(serde::Deserialize)]
pub(crate) struct Additions {
    pub(crate) words: Vec<String>,
    pub(crate) shortcuts: Vec<(String, String)>,
}

fn token(text: &str) -> bool {
    fits(text, MAX_WORD_CHARS, &[]) && !text.chars().any(char::is_whitespace)
}

impl Additions {
    /// The preview is a promise about what gets added. If something changed during review,
    /// reject the whole batch so the user can check it again.
    pub(crate) fn append_to(self, settings: &mut Settings) -> io::Result<()> {
        let conflict = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "dictionary changed or an addition was invalid",
            )
        };
        if settings
            .personal_words
            .len()
            .saturating_add(self.words.len())
            > MAX_PERSONAL_WORDS
            || settings
                .user_rules
                .len()
                .saturating_add(self.shortcuts.len())
                > MAX_USER_RULES
        {
            return Err(conflict());
        }
        let mut keys: HashSet<_> = settings
            .personal_words
            .iter()
            .chain(settings.user_rules.iter().map(|(from, _)| from))
            .map(|w| word_key(w))
            .collect();
        if !self
            .words
            .iter()
            .all(|w| token(w) && keys.insert(word_key(w)))
            || !self.shortcuts.iter().all(|(from, to)| {
                token(from)
                    && fits(to, MAX_REPLACEMENT_CHARS, &['\n', '\t'])
                    && !to.trim().is_empty()
                    && keys.insert(word_key(from))
            })
        {
            return Err(conflict());
        }
        let added_sources: Vec<_> = self
            .words
            .iter()
            .map(|word| (word_key(word), None))
            .collect();
        let mut words = settings.personal_words.clone();
        words.extend(self.words);
        let mut shortcuts = settings.user_rules.clone();
        shortcuts.extend(self.shortcuts);
        settings.patch(
            Map::from_iter([
                ("personalWords".into(), json!(words)),
                ("userRules".into(), json!(shortcuts)),
            ]),
            false,
        )?;
        settings.word_sources.extend(added_sources);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(words: &[&str], shortcuts: &[(&str, &str)]) -> Additions {
        Additions {
            words: words.iter().map(|w| (*w).into()).collect(),
            shortcuts: shortcuts
                .iter()
                .map(|(from, to)| ((*from).into(), (*to).into()))
                .collect(),
        }
    }

    #[test]
    fn append_preserves_existing_entries_and_word_sources() {
        let mut settings = Settings {
            personal_words: vec!["existing".into()],
            word_sources: [("existing".into(), Some(42))].into(),
            user_rules: vec![("omw".into(), "on my way".into())],
            ..Settings::default()
        };
        batch(&["Tauri"], &[("brb", "be right back\n")])
            .append_to(&mut settings)
            .unwrap();
        assert_eq!(settings.personal_words, ["existing", "Tauri"]);
        assert_eq!(settings.word_sources["existing"], Some(42));
        assert_eq!(settings.word_sources["tauri"], None);
        assert_eq!(settings.user_rules[0].1, "on my way");
        assert_eq!(settings.user_rules[1].1, "be right back\n");
    }

    #[test]
    fn a_new_conflict_or_invalid_row_does_not_partly_apply() {
        let mut settings = Settings {
            user_rules: vec![("o’clock".into(), "existing".into())],
            ..Settings::default()
        };
        for draft in [
            batch(&["valid"], &[("O'CLOCK", "replacement")]),
            batch(&["once", "ONCE"], &[]),
            batch(&["O'CLOCK"], &[]),
            batch(&["brb"], &[("BRB", "be right back")]),
            batch(&["two words"], &[]),
            batch(&["valid"], &[("brb", "tab follows\r")]),
            batch(&["valid"], &[("brb", " ")]),
        ] {
            assert!(draft.append_to(&mut settings).is_err());
            assert!(settings.personal_words.is_empty());
            assert_eq!(settings.user_rules[0].1, "existing");
            assert_eq!(settings.user_rules.len(), 1);
        }
    }

    #[test]
    fn reject_a_batch_that_no_longer_fits() {
        let mut settings = Settings {
            personal_words: (0..MAX_PERSONAL_WORDS)
                .map(|n| format!("word{n}"))
                .collect(),
            ..Settings::default()
        };
        assert!(batch(&["extra"], &[("brb", "be right back")])
            .append_to(&mut settings)
            .is_err());
        assert!(settings.user_rules.is_empty());
        assert_eq!(settings.personal_words.len(), MAX_PERSONAL_WORDS);
    }
}
