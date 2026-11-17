//! How often each typo's fix was undone, kept on this computer only (`undo-counts.json` in the
//! app data folder). A typo stops being fixed once its fix was undone `learnAfterUndos` times
//! (default 2): the engine counts and decides, and gets these counts back after a restart.
//! Never sent anywhere; cleared for a word once it is learned.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Pairs kept. The rarest go first; a pair that matters is undone again soon anyway.
const MAX_PAIRS: usize = 500;

pub struct UndoCounts {
    path: PathBuf,
    /// `typo\tfix` (lowercased) to times undone.
    counts: Mutex<BTreeMap<String, u32>>,
}

fn key(typo: &str, fix: &str) -> String {
    format!("{}\t{}", typo.to_lowercase(), fix.to_lowercase())
}

impl UndoCounts {
    pub fn new(dir: &Path) -> Self {
        let path = dir.join("undo-counts.json");
        let counts = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            counts: Mutex::new(counts),
        }
    }

    /// One more undo of `typo` fixed to `fix`. Saved at once (undos are rare).
    pub fn record(&self, typo: &str, fix: &str) {
        let mut counts = self.counts.lock().unwrap();
        let k = key(typo, fix);
        *counts.entry(k.clone()).or_default() += 1;
        if counts.len() > MAX_PAIRS {
            if let Some(rarest) = counts
                .iter()
                .filter(|(other, _)| **other != k)
                .min_by_key(|(_, &c)| c)
                .map(|(other, _)| other.clone())
            {
                counts.remove(&rarest);
            }
        }
        self.save(&counts);
    }

    /// `word` was learned: its counts aren't needed any more.
    pub fn forget(&self, word: &str) {
        let mut counts = self.counts.lock().unwrap();
        let prefix = format!("{}\t", word.to_lowercase());
        let before = counts.len();
        counts.retain(|k, _| !k.starts_with(&prefix));
        if counts.len() != before {
            self.save(&counts);
        }
    }

    /// `(typo, fix, times)` for the engine.
    pub fn snapshot(&self) -> Vec<(String, String, u32)> {
        self.counts
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(k, &c)| {
                let (typo, fix) = k.split_once('\t')?;
                Some((typo.to_string(), fix.to_string(), c))
            })
            .collect()
    }

    fn save(&self, counts: &BTreeMap<String, u32>) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        let written = serde_json::to_vec(counts)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(&tmp, bytes))
            .and_then(|()| fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            eprintln!("undo counts: can't save ({e})");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_persist_and_learned_words_are_forgotten() {
        let dir = std::env::temp_dir().join(format!("unflub-undo-counts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let counts = UndoCounts::new(&dir);
        counts.record("Teh", "the");
        counts.record("teh", "The");
        counts.record("adn", "and");
        let again = UndoCounts::new(&dir);
        let mut snap = again.snapshot();
        snap.sort();
        assert_eq!(
            snap,
            vec![
                ("adn".into(), "and".into(), 1),
                ("teh".into(), "the".into(), 2)
            ]
        );
        again.forget("TEH");
        assert_eq!(
            UndoCounts::new(&dir).snapshot(),
            vec![("adn".into(), "and".into(), 1)]
        );
        let _ = fs::remove_dir_all(dir);
    }
}
