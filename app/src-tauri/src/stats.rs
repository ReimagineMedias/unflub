//! The Stats page: what unflub did for you, kept only on this computer (`stats.json` in the app
//! data folder) and never sent anywhere.
//!
//! Per local day: fixes, undos, words checked and an estimate of the keystrokes saved. With
//! "Keep a list of corrected words" on (the default) it also keeps how often each typo was
//! fixed, for the top ten. No app names, no surrounding text.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Days kept for the charts; totals keep counting forever.
const KEEP_DAYS: i64 = 60;
/// Distinct typo → fix pairs kept. The rarest go first.
const MAX_WORDS: usize = 500;
const TOP_WORDS: usize = 10;
pub const SPARK_DAYS: usize = 30;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Day {
    pub corrections: u64,
    pub undos: u64,
    pub words_checked: u64,
    pub keystrokes_saved: u64,
}

impl Day {
    fn add(&mut self, other: &Day) {
        self.corrections += other.corrections;
        self.undos += other.undos;
        self.words_checked += other.words_checked;
        self.keystrokes_saved += other.keystrokes_saved;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct StatsFile {
    /// Local day number (days since 1970-01-01 in local time) to that day's counts.
    days: BTreeMap<i64, Day>,
    total: Day,
    /// `typo\tfix` to times fixed.
    words: BTreeMap<String, u64>,
    /// First day anything was counted.
    since: Option<i64>,
}

/// One row of the top-words list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopWord {
    pub typo: String,
    pub fix: String,
    pub count: u64,
}

/// What the Stats page shows. Mirrors `StatsView` in `app/src/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    pub today: Day,
    pub week: Day,
    pub all_time: Day,
    /// Fixes per day, oldest first, today last.
    pub last30: Vec<u64>,
    pub top_words: Vec<TopWord>,
    /// Days since counting started, including today.
    pub days_counted: u64,
}

pub struct Stats {
    path: PathBuf,
    file: Mutex<StatsFile>,
    dirty: AtomicBool,
    /// Every engine adds the words it checks here; taken with `swap(0)`.
    pub words_checked: Arc<AtomicU64>,
}

/// Today's local day number.
pub fn today() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let offset = time::UtcOffset::current_local_offset().map_or(0, |o| o.whole_seconds() as i64);
    (secs + offset).div_euclid(86_400)
}

/// Keys a person would press to fix `typo` into `fix` by hand: back to the first wrong letter,
/// then type the rest.
pub fn keystrokes_saved(typo: &str, fix: &str) -> u64 {
    let common = typo
        .chars()
        .zip(fix.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let back = typo.chars().count() - common;
    let retype = fix.chars().count() - common;
    (back + retype) as u64
}

fn word_key(typo: &str, fix: &str) -> String {
    format!("{typo}\t{fix}")
}

impl Stats {
    pub fn new(dir: PathBuf) -> Self {
        let path = dir.join("stats.json");
        let file = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            file: Mutex::new(file),
            dirty: AtomicBool::new(false),
            words_checked: Arc::new(AtomicU64::new(0)),
        }
    }

    fn with_day(&self, day: i64, f: impl FnOnce(&mut StatsFile, &mut Day)) {
        let mut file = self.file.lock().unwrap();
        file.since.get_or_insert(day);
        let mut d = file.days.get(&day).copied().unwrap_or_default();
        let before = d;
        f(&mut file, &mut d);
        // Totals move by the same amount as the day.
        file.total.corrections += d.corrections - before.corrections;
        file.total.undos += d.undos - before.undos;
        file.total.words_checked += d.words_checked - before.words_checked;
        let saved = i128::from(d.keystrokes_saved) - i128::from(before.keystrokes_saved);
        file.total.keystrokes_saved =
            (i128::from(file.total.keystrokes_saved) + saved).max(0) as u64;
        file.days.insert(day, d);
        file.days.retain(|&k, _| k > day - KEEP_DAYS);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// A fix landed. `keep_word`: remember the pair for the top-words list.
    pub fn correction(&self, typo: &str, fix: &str, keep_word: bool) {
        self.correction_on(today(), typo, fix, keep_word);
    }

    fn correction_on(&self, day: i64, typo: &str, fix: &str, keep_word: bool) {
        let saved = keystrokes_saved(&typo.to_lowercase(), &fix.to_lowercase());
        self.with_day(day, |file, d| {
            d.corrections += 1;
            d.keystrokes_saved += saved;
            if keep_word {
                let key = word_key(&typo.to_lowercase(), &fix.to_lowercase());
                *file.words.entry(key.clone()).or_default() += 1;
                if file.words.len() > MAX_WORDS {
                    // Forget the rarest other pair, so a new typo gets a chance to climb.
                    if let Some(rarest) = file
                        .words
                        .iter()
                        .filter(|(k, _)| **k != key)
                        .min_by_key(|(_, &c)| c)
                        .map(|(k, _)| k.clone())
                    {
                        file.words.remove(&rarest);
                    }
                }
            }
        });
    }

    /// A fix was undone: it saved nothing after all.
    pub fn undo(&self, typo: &str, fix: &str) {
        self.undo_on(today(), typo, fix);
    }

    fn undo_on(&self, day: i64, typo: &str, fix: &str) {
        let saved = keystrokes_saved(&typo.to_lowercase(), &fix.to_lowercase());
        self.with_day(day, |file, d| {
            d.undos += 1;
            d.keystrokes_saved = d.keystrokes_saved.saturating_sub(saved);
            let key = word_key(&typo.to_lowercase(), &fix.to_lowercase());
            if let Some(c) = file.words.get_mut(&key) {
                *c -= 1;
                if *c == 0 {
                    file.words.remove(&key);
                }
            }
        });
    }

    /// Move the engine's word count into today.
    fn take_words_checked(&self, day: i64) {
        let n = self.words_checked.swap(0, Ordering::Relaxed);
        if n > 0 {
            self.with_day(day, |_, d| d.words_checked += n);
        }
    }

    /// "Keep a list of corrected words" was turned off.
    pub fn forget_words(&self) {
        self.file.lock().unwrap().words.clear();
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();
    }

    /// "Clear history".
    pub fn clear(&self) {
        self.words_checked.store(0, Ordering::Relaxed);
        *self.file.lock().unwrap() = StatsFile::default();
        let _ = fs::remove_file(&self.path);
    }

    pub fn view(&self) -> StatsView {
        let day = today();
        self.take_words_checked(day);
        self.view_on(day)
    }

    fn view_on(&self, day: i64) -> StatsView {
        let file = self.file.lock().unwrap();
        let at = |d: i64| file.days.get(&d).copied().unwrap_or_default();
        let mut week = Day::default();
        for d in day - 6..=day {
            week.add(&at(d));
        }
        let last30 = (day - SPARK_DAYS as i64 + 1..=day)
            .map(|d| at(d).corrections)
            .collect();
        let mut top: Vec<TopWord> = file
            .words
            .iter()
            .filter_map(|(k, &count)| {
                let (typo, fix) = k.split_once('\t')?;
                Some(TopWord {
                    typo: typo.into(),
                    fix: fix.into(),
                    count,
                })
            })
            .collect();
        top.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.typo.cmp(&b.typo)));
        top.truncate(TOP_WORDS);
        StatsView {
            today: at(day),
            week,
            all_time: file.total,
            last30,
            top_words: top,
            days_counted: file.since.map_or(0, |s| (day - s + 1).max(1) as u64),
        }
    }

    /// Write `stats.json` if anything changed.
    pub fn flush(&self) {
        self.take_words_checked(today());
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let file = self.file.lock().unwrap().clone();
        let written = serde_json::to_vec(&file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::files::replace(&self.path, &bytes));
        if let Err(e) = written {
            eprintln!("stats: can't save ({e})");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("unflub-stats-{name}-{}", std::process::id()))
    }

    #[test]
    fn estimates_keystrokes() {
        assert_eq!(keystrokes_saved("teh", "the"), 4);
        // Back to the "i", then "eive".
        assert_eq!(keystrokes_saved("recieve", "receive"), 8);
        assert_eq!(keystrokes_saved("adn", "and"), 4);
        assert_eq!(keystrokes_saved("helo", "hello"), 3);
    }

    #[test]
    fn aggregates_days_weeks_and_totals() {
        let stats = Stats::new(temp("agg"));
        let day = 20_000;
        stats.correction_on(day - 10, "teh", "the", true);
        stats.correction_on(day - 3, "adn", "and", true);
        stats.correction_on(day, "teh", "the", true);
        stats.correction_on(day, "Teh", "The", true);
        stats.undo_on(day, "adn", "and");
        stats.words_checked.fetch_add(100, Ordering::Relaxed);
        stats.take_words_checked(day);

        let v = stats.view_on(day);
        assert_eq!(v.today.corrections, 2);
        assert_eq!(v.today.undos, 1);
        assert_eq!(v.today.words_checked, 100);
        assert_eq!(v.week.corrections, 3);
        assert_eq!(v.all_time.corrections, 4);
        assert_eq!(v.all_time.undos, 1);
        assert_eq!(v.last30.len(), SPARK_DAYS);
        assert_eq!(v.last30[SPARK_DAYS - 1], 2);
        assert_eq!(v.last30[SPARK_DAYS - 11], 1);
        assert_eq!(v.days_counted, 11);
        // The undo took "adn → and" off the list; case doesn't split a pair.
        assert_eq!(
            v.top_words,
            vec![TopWord {
                typo: "teh".into(),
                fix: "the".into(),
                count: 3
            }]
        );
        // 4 keys for each of three teh fixes; the adn fix (4) was undone.
        assert_eq!(v.all_time.keystrokes_saved, 12);
    }

    #[test]
    fn counts_only_without_the_word_list() {
        let stats = Stats::new(temp("nowords"));
        stats.correction_on(1, "teh", "the", false);
        let v = stats.view_on(1);
        assert_eq!(v.today.corrections, 1);
        assert!(v.top_words.is_empty());
        stats.correction_on(1, "teh", "the", true);
        stats.forget_words();
        assert!(stats.view_on(1).top_words.is_empty());
        let _ = fs::remove_dir_all(temp("nowords"));
    }

    #[test]
    fn old_days_roll_off_but_totals_stay() {
        let stats = Stats::new(temp("roll"));
        stats.correction_on(1, "teh", "the", true);
        stats.correction_on(1 + KEEP_DAYS + 5, "teh", "the", true);
        let file = stats.file.lock().unwrap().clone();
        assert_eq!(file.days.len(), 1);
        assert_eq!(file.total.corrections, 2);
    }

    #[test]
    fn persists_and_clears() {
        let dir = temp("persist");
        let stats = Stats::new(dir.clone());
        stats.correction_on(today(), "teh", "the", true);
        stats.flush();
        let again = Stats::new(dir.clone());
        assert_eq!(again.view().all_time.corrections, 1);
        again.clear();
        assert_eq!(again.view().all_time.corrections, 0);
        assert!(!dir.join("stats.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn word_list_is_capped() {
        let stats = Stats::new(temp("cap"));
        for i in 0..(MAX_WORDS + 50) {
            stats.correction_on(1, &format!("w{i}"), "x", true);
        }
        stats.correction_on(1, "teh", "the", true);
        stats.correction_on(1, "teh", "the", true);
        let file = stats.file.lock().unwrap();
        assert!(file.words.len() <= MAX_WORDS);
        assert_eq!(file.words.get("teh\tthe"), Some(&2));
    }
}
