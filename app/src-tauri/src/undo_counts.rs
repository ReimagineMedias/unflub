//! How often each typo's fix was undone, kept on this computer only. A typo stops being fixed
//! once its fix was undone `learnAfterUndos` times (default 2): the engine counts and decides,
//! and gets these counts back after a restart. Never sent anywhere; cleared for a word once it
//! is learned.
//!
//! The pairs are typed words, so they are only kept when they will be used and the user lets
//! unflub keep words: nothing is recorded while learning is off (`lib.rs`), and they are written
//! to `undo-counts.json` only while the Stats page's "Keep a list" is on (otherwise they live in
//! memory for this run). Clearing history clears them here and, through a new [`epoch`], in the
//! running engine.
//!
//! [`epoch`]: UndoCounts::epoch

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// Pairs kept. The rarest go first; a pair that matters is undone again soon anyway.
const MAX_PAIRS: usize = 500;

pub struct UndoCounts {
    path: PathBuf,
    /// `typo\tfix` (lowercased) to times undone.
    counts: Mutex<BTreeMap<String, u32>>,
    /// Written to disk (the user keeps words).
    persist: AtomicBool,
    /// Bumped by [`UndoCounts::clear`]; the engine forgets its own counts when it changes.
    epoch: AtomicU64,
}

fn key(typo: &str, fix: &str) -> String {
    format!("{}\t{}", typo.to_lowercase(), fix.to_lowercase())
}

impl UndoCounts {
    /// `persist`: the user keeps words (Stats > Keep a list). Without it any file is deleted.
    pub fn new(dir: &Path, persist: bool) -> Self {
        let path = dir.join("undo-counts.json");
        let counts = if persist {
            fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        } else {
            let _ = fs::remove_file(&path);
            BTreeMap::new()
        };
        Self {
            path,
            counts: Mutex::new(counts),
            persist: AtomicBool::new(persist),
            epoch: AtomicU64::new(0),
        }
    }

    /// Keep the pairs on disk or not. Turning it off deletes the file; this run keeps counting
    /// in memory.
    pub fn set_persist(&self, persist: bool) {
        self.persist.store(persist, Ordering::Relaxed);
        let counts = self.counts.lock().unwrap();
        if persist {
            self.save(&counts);
        } else {
            let _ = fs::remove_file(&self.path);
        }
    }

    /// Forget every pair, on disk and (at the next settings push, through [`Self::epoch`]) in
    /// the engine.
    pub fn clear(&self) {
        self.counts.lock().unwrap().clear();
        self.epoch.fetch_add(1, Ordering::Relaxed);
        let _ = fs::remove_file(&self.path);
    }

    /// Changes on every [`Self::clear`].
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Relaxed)
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
    #[cfg_attr(not(windows), allow(dead_code))]
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
        if !self.persist.load(Ordering::Relaxed) {
            return;
        }
        let written = serde_json::to_vec(counts)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::files::replace(&self.path, &bytes));
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
        let counts = UndoCounts::new(&dir, true);
        counts.record("Teh", "the");
        counts.record("teh", "The");
        counts.record("adn", "and");
        let again = UndoCounts::new(&dir, true);
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
            UndoCounts::new(&dir, true).snapshot(),
            vec![("adn".into(), "and".into(), 1)]
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Review finding 5: pairs stay off disk unless the user keeps words, and clearing history
    /// clears them everywhere.
    #[test]
    fn pairs_are_kept_only_when_words_are_and_clear_with_history() {
        let dir = std::env::temp_dir().join(format!("unflub-undo-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("undo-counts.json");
        let counts = UndoCounts::new(&dir, false);
        counts.record("teh", "the");
        assert!(!file.exists(), "not written while words aren't kept");
        assert_eq!(counts.snapshot().len(), 1, "still counted for this run");
        counts.set_persist(true);
        assert!(file.exists());
        counts.set_persist(false);
        assert!(!file.exists(), "switching it off deletes the file");
        counts.set_persist(true);
        let epoch = counts.epoch();
        counts.clear();
        assert!(counts.snapshot().is_empty());
        assert!(!file.exists());
        assert_ne!(counts.epoch(), epoch, "the engine forgets its counts too");
        // Starting with words not kept deletes an old file.
        counts.record("adn", "and");
        assert!(file.exists());
        assert!(UndoCounts::new(&dir, false).snapshot().is_empty());
        assert!(!file.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
