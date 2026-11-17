//! Upgrades an older settings file one schema version at a time, on the raw JSON, before serde
//! reads it into [`Settings`](crate::settings::Settings).
//!
//! To change the format: bump [`SCHEMA_VERSION`], add a `vN_to_vN+1` step below and to
//! [`STEPS`], and test it with a real file of the old shape. Steps only add or rewrite fields
//! they know; anything else in the file is left alone (unknown fields survive in
//! `Settings::extra`).

use crate::settings::{DEFAULT_EXCLUDED_APPS, SCHEMA_VERSION};
use serde_json::{json, Map, Value};

type Step = fn(&mut Map<String, Value>);

/// `STEPS[n]` upgrades version `n` to `n + 1`.
const STEPS: &[Step] = &[v0_to_v1, v1_to_v2];

const _: () = assert!(STEPS.len() == SCHEMA_VERSION as usize);

/// The file's schema version. Files written before versions existed are 0.
fn version_of(obj: &Map<String, Value>) -> u32 {
    obj.get("schemaVersion")
        .and_then(Value::as_u64)
        .map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX))
}

/// Upgrade `value` (a settings object) to [`SCHEMA_VERSION`]. Returns the version it started at
/// when anything changed. A file from a newer unflub is returned untouched.
pub fn migrate(mut value: Value) -> (Value, Option<u32>) {
    let Some(obj) = value.as_object_mut() else {
        return (value, None);
    };
    let from = version_of(obj);
    if from >= SCHEMA_VERSION {
        return (value, None);
    }
    for (version, step) in STEPS.iter().enumerate().skip(from as usize) {
        step(obj);
        obj.insert("schemaVersion".into(), json!(version + 1));
    }
    (value, Some(from))
}

/// v0 (no `schemaVersion`) to v1: update channel, the privacy switches at their
/// defaults, the welcome screen (so people see those switches once), and the default excluded
/// apps for files written before that list existed.
fn v0_to_v1(obj: &mut Map<String, Value>) {
    // Early builds wrote an empty list before terminals and password managers were excluded by
    // default. An empty list in a v0 file means "never chosen", so give it the defaults.
    let empty = obj
        .get("excludedApps")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    if empty {
        obj.insert("excludedApps".into(), json!(DEFAULT_EXCLUDED_APPS));
    }
    // Early preview builds kept an install id. The app sends no identifier any more.
    obj.remove("installId");
    obj.entry("channel").or_insert(json!("stable"));
    obj.entry("shareUsageStats").or_insert(json!(true));
    obj.entry("sendCrashReports").or_insert(json!(true));
    obj.entry("statsKeepWords").or_insert(json!(true));
    obj.entry("onboarded").or_insert(json!(false));
}

/// v1 to v2: undo became double-Backspace (`undoMode`, default "twice") and learning waits for
/// repeated undos (`learnAfterUndos`). `undoLearns: false` keeps meaning "never learn"; words
/// already in the personal dictionary are left alone.
fn v1_to_v2(obj: &mut Map<String, Value>) {
    let learned = obj
        .remove("undoLearns")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    obj.entry("undoMode").or_insert(json!("twice"));
    obj.entry("learnAfterUndos")
        .or_insert(json!(if learned { 2 } else { 0 }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Channel, Settings};

    /// A settings file as the first preview builds wrote it.
    const V0: &str = r#"{
        "enabled": true,
        "startAtLogin": true,
        "autoUpdate": false,
        "mode": "aggressive",
        "confidence": 0.4,
        "minWordLen": 3,
        "skipCode": true,
        "undoLearns": true,
        "excludedApps": [],
        "personalWords": ["rmgn"],
        "userRules": [["omw", "on my way"]],
        "pillStyle": "capsule",
        "pillOpacity": 0.8,
        "pillDurationMs": 1200,
        "pillPosition": "below",
        "theme": "ink",
        "appearance": "dark",
        "language": "en-GB"
    }"#;

    fn load(json: &str) -> (Settings, Option<u32>) {
        let (value, from) = migrate(serde_json::from_str(json).unwrap());
        (serde_json::from_value(value).unwrap(), from)
    }

    #[test]
    fn v0_to_v1() {
        let (s, from) = load(V0);
        assert_eq!(from, Some(0));
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        assert_eq!(s.excluded_apps, DEFAULT_EXCLUDED_APPS);
        assert_eq!(s.channel, Channel::Stable);
        assert!(s.share_usage_stats);
        assert!(s.send_crash_reports);
        assert!(s.stats_keep_words);
        assert!(!s.onboarded, "existing users see the welcome once");
        // The user's choices are untouched.
        assert!(!s.auto_update);
        assert_eq!(s.personal_words, ["rmgn"]);
        assert_eq!(s.min_word_len, 3);
        assert_eq!(s.language, "en-GB");
        assert!(s.extra.is_empty(), "{:?}", s.extra.keys());
    }

    #[test]
    fn v1_to_v2_adds_the_undo_settings() {
        use crate::settings::UndoMode;
        let v1 = r#"{ "schemaVersion": 1, "undoLearns": true, "personalWords": ["teh"] }"#;
        let (s, from) = load(v1);
        assert_eq!(from, Some(1));
        assert_eq!((s.undo_mode, s.learn_after_undos), (UndoMode::Twice, 2));
        assert_eq!(s.personal_words, ["teh"], "learned words stay");
        assert!(!s.extra.contains_key("undoLearns"));
        // "Undo doesn't teach" stays that way.
        let (s, _) = load(r#"{ "schemaVersion": 1, "undoLearns": false }"#);
        assert_eq!((s.undo_mode, s.learn_after_undos), (UndoMode::Twice, 0));
        // A file that already says otherwise keeps it.
        let (s, _) = load(r#"{ "schemaVersion": 1, "undoMode": "off", "learnAfterUndos": 4 }"#);
        assert_eq!((s.undo_mode, s.learn_after_undos), (UndoMode::Off, 4));
    }

    #[test]
    fn v0_keeps_a_chosen_exclude_list() {
        let (s, _) = load(r#"{ "excludedApps": ["code.exe"] }"#);
        assert_eq!(s.excluded_apps, ["code.exe"]);
    }

    /// An install id from an early preview build is dropped, not kept as an unknown field.
    #[test]
    fn v0_drops_an_old_install_id() {
        let (s, _) = load(r#"{ "installId": "0b5f3a52-5d2e-4f0b-9a3e-1c2d3e4f5a6b" }"#);
        assert!(!s.extra.contains_key("installId"));
    }

    #[test]
    fn current_files_are_left_alone() {
        let current = format!(r#"{{ "schemaVersion": {SCHEMA_VERSION}, "excludedApps": [] }}"#);
        let (s, from) = load(&current);
        assert_eq!(from, None);
        assert!(s.excluded_apps.is_empty());
    }

    /// A file from a newer unflub: nothing is migrated or lost, and saving writes the unknown
    /// fields and the newer version back.
    #[test]
    fn newer_files_keep_unknown_fields() {
        let newer = r#"{ "schemaVersion": 7, "enabled": false, "futureThing": { "a": [1, 2] }, "otherFlag": true }"#;
        let (s, from) = load(newer);
        assert_eq!(from, None);
        assert!(!s.enabled);
        assert_eq!(s.schema_version, 7);
        assert_eq!(s.extra["futureThing"], json!({ "a": [1, 2] }));
        assert_eq!(s.extra["otherFlag"], json!(true));
        let written: Value = serde_json::to_value(&s).unwrap();
        assert_eq!(written["futureThing"], json!({ "a": [1, 2] }));
        assert_eq!(written["schemaVersion"], json!(7));
    }

    #[test]
    fn store_migrates_and_backs_up() {
        let dir = std::env::temp_dir().join(format!("unflub-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), V0).unwrap();
        let store = crate::settings::Store::new(dir.clone());
        let s = store.load();
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        assert!(dir.join("settings.json.v0").exists());
        // Written back: the next load changes nothing.
        let again = store.load();
        assert_eq!(again, s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
