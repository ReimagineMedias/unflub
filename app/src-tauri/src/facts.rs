//! Two local facts about this install for the usage stats' `env` group: the day unflub first ran
//! and how the first-run tour went. Kept in `install-facts.json` in the app data folder, also
//! while usage stats are off (like a setting), so turning stats on later reports them correctly.
//! Only coarse buckets of them are ever sent (`usage.rs`): never the date itself.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};

/// How the first-run tour went. Set by the settings window (`usage_event`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TourState {
    #[default]
    NotStarted,
    Completed,
    Skipped,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct File {
    /// Days since 1970-01-01 (UTC) of the first run.
    first_run_day: Option<i64>,
    tour: TourState,
}

pub struct Facts {
    path: PathBuf,
    file: Mutex<File>,
}

/// Unix seconds to whole days since 1970-01-01.
pub fn day_of(secs: u64) -> i64 {
    (secs / (24 * 60 * 60)) as i64
}

impl Facts {
    /// Load the facts from `dir`. Without a first-run day yet, `first_run_day` is kept as it.
    pub fn load(dir: PathBuf, first_run_day: i64) -> Self {
        let path = dir.join("install-facts.json");
        let mut file: File = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let facts_missing = file.first_run_day.is_none();
        file.first_run_day.get_or_insert(first_run_day);
        let facts = Self {
            path,
            file: Mutex::new(file),
        };
        if facts_missing {
            facts.save();
        }
        facts
    }

    fn save(&self) {
        let file = self.file.lock().unwrap().clone();
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        let written = serde_json::to_vec(&file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(&tmp, bytes))
            .and_then(|()| fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            eprintln!("install facts: can't save ({e})");
        }
    }

    /// Whole days since the first run (0 on that day; never negative).
    pub fn days_since_install(&self, today: i64) -> i64 {
        let first = self.file.lock().unwrap().first_run_day.unwrap_or(today);
        (today - first).max(0)
    }

    pub fn tour(&self) -> TourState {
        self.file.lock().unwrap().tour
    }

    pub fn set_tour(&self, tour: TourState) {
        {
            let mut file = self.file.lock().unwrap();
            if file.tour == tour {
                return;
            }
            file.tour = tour;
        }
        self.save();
    }
}

/// The first-run day for an install that has no facts file yet: today on a first run, else the
/// day the config folder was created (it is never recreated), else today.
pub fn first_run_day(first_run: bool, config_dir: &std::path::Path, today: i64) -> i64 {
    if first_run {
        return today;
    }
    fs::metadata(config_dir)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(today, |d| day_of(d.as_secs()).min(today))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_first_run_day_and_the_tour() {
        let dir = std::env::temp_dir().join(format!("unflub-facts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let facts = Facts::load(dir.clone(), 20_000);
        assert_eq!(facts.days_since_install(20_000), 0);
        assert_eq!(facts.days_since_install(20_045), 45);
        assert_eq!(facts.days_since_install(19_000), 0, "clock set back");
        assert_eq!(facts.tour(), TourState::NotStarted);
        facts.set_tour(TourState::Skipped);
        // A later start keeps the first day, whatever it is told.
        let again = Facts::load(dir.clone(), 20_100);
        assert_eq!(again.days_since_install(20_100), 100);
        assert_eq!(again.tour(), TourState::Skipped);
        let text = fs::read_to_string(dir.join("install-facts.json")).unwrap();
        assert_eq!(text, r#"{"first_run_day":20000,"tour":"skipped"}"#);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_upgrade_dates_the_install_from_its_config_folder() {
        let dir = std::env::temp_dir();
        let today = day_of(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        assert_eq!(first_run_day(true, &dir, today), today);
        assert!(first_run_day(false, &dir, today) <= today);
        assert_eq!(
            first_run_day(false, &dir.join("no-such-unflub-dir"), today),
            today
        );
    }
}
