//! Anonymous usage stats: a handful of counts, sent at most once a day while "Share anonymous
//! usage stats" is on (the default, chosen on the welcome screen; one switch turns it off, and
//! so does the `DisableUsageStats` machine policy). Requests carry no ID (`net.rs`).
//!
//! Only what [`Payload`] holds ever leaves the machine, and its keys are exactly the allowlist
//! in the API contract (a test checks). No words, no corrections, no app names, no paths. While
//! the switch is off nothing is counted, and turning it off deletes what was counted.
//!
//! The engine thread records into [`Usage`]; the counts are kept in `usage.json` in the app data
//! folder so they survive restarts, and sent by the `unflub-stats` thread.

use crate::settings::{Appearance, EngineMode, PillStyle, Settings, Theme};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Bump with the contract.
pub const SCHEMA: u32 = 1;
/// The server refuses bigger bodies.
pub const MAX_BODY: usize = 2048;
const DAY_SECS: u64 = 24 * 60 * 60;
/// Latency samples kept for the p99s. Enough for a day of normal typing; older ones roll off.
const MAX_SAMPLES: usize = 512;

/// The body of `POST /unflub/v1/events`. Every field here is in the contract's allowlist, and
/// nothing else may be added without changing the contract first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Payload {
    pub schema: u32,
    pub version: String,
    pub target: &'static str,
    pub period: &'static str,
    pub counters: Counters,
    pub settings: SettingsSnapshot,
    pub perf: Perf,
    pub health: Health,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Counters {
    pub corrections: u64,
    pub undos: u64,
    /// Corrections the pill showed.
    pub suggestions_shown: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsSnapshot {
    pub engine: EngineMode,
    pub pill: PillStyle,
    pub theme: Theme,
    pub appearance: Appearance,
    pub excluded_count: u64,
    pub personal_words_count: u64,
    pub rules_count: u64,
    pub auto_update: bool,
}

impl SettingsSnapshot {
    pub fn of(s: &Settings) -> Self {
        Self {
            engine: s.mode,
            pill: s.pill_style,
            theme: s.theme,
            appearance: s.appearance,
            excluded_count: s.excluded_apps.len() as u64,
            personal_words_count: s.personal_words.len() as u64,
            rules_count: s.user_rules.len() as u64,
            auto_update: s.auto_update,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Perf {
    pub decide_p99_us: u64,
    pub inject_p99_us: u64,
    pub idle_private_mb: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// unflub stopped on a panic (counted at the next start), or the engine's corrector panicked.
    pub crashes: u64,
    /// The adapter's watchdog installed the keyboard hook again (Windows dropped it, or the
    /// computer resumed or was unlocked).
    pub hook_reinstalls: u64,
    /// The engine fell back to pass-through because its own injected keys stopped coming back
    /// (another program interferes with keyboard input).
    #[serde(default)]
    pub self_pauses: u64,
}

/// What is kept on disk between sends.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Tally {
    counters: Counters,
    health: Health,
    /// Microseconds, newest last.
    decide_us: Vec<u32>,
    inject_us: Vec<u32>,
    /// When counting started (switch turned on, or the last accepted send).
    since: u64,
}

pub struct Usage {
    path: PathBuf,
    on: AtomicBool,
    tally: Mutex<Tally>,
    dirty: AtomicBool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn push_sample(samples: &mut Vec<u32>, d: Duration) {
    if samples.len() >= MAX_SAMPLES {
        samples.remove(0);
    }
    samples.push(u32::try_from(d.as_micros()).unwrap_or(u32::MAX));
}

/// The 99th percentile (nearest rank), 0 with no samples.
fn p99(samples: &[u32]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = ((sorted.len() as f64) * 0.99).ceil() as usize;
    u64::from(sorted[rank.clamp(1, sorted.len()) - 1])
}

impl Usage {
    /// `on` is the `share_usage_stats` setting at startup.
    pub fn new(dir: PathBuf, on: bool) -> Self {
        let path = dir.join("usage.json");
        let tally = if on {
            fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_else(|| Tally {
                    since: now_secs(),
                    ..Tally::default()
                })
        } else {
            let _ = fs::remove_file(&path);
            Tally::default()
        };
        Self {
            path,
            on: AtomicBool::new(on),
            tally: Mutex::new(tally),
            dirty: AtomicBool::new(false),
        }
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// The switch changed. Off forgets everything counted so far.
    pub fn set_on(&self, on: bool) {
        if self.on.swap(on, Ordering::Relaxed) == on {
            return;
        }
        let mut tally = self.tally.lock().unwrap();
        *tally = Tally {
            since: now_secs(),
            ..Tally::default()
        };
        if on {
            self.dirty.store(true, Ordering::Relaxed);
        } else {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn record(&self, f: impl FnOnce(&mut Tally)) {
        if !self.is_on() {
            return;
        }
        f(&mut self.tally.lock().unwrap());
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// A fix landed. `pill` is true when the pill was asked to show it.
    pub fn correction(&self, decide: Duration, inject: Duration, pill: bool) {
        self.record(|t| {
            t.counters.corrections += 1;
            t.counters.suggestions_shown += u64::from(pill);
            push_sample(&mut t.decide_us, decide);
            push_sample(&mut t.inject_us, inject);
        });
    }

    pub fn undo(&self) {
        self.record(|t| t.counters.undos += 1);
    }

    pub fn crash(&self) {
        self.record(|t| t.health.crashes += 1);
    }

    pub fn hook_reinstall(&self) {
        self.record(|t| t.health.hook_reinstalls += 1);
    }

    pub fn self_pause(&self) {
        self.record(|t| t.health.self_pauses += 1);
    }

    /// The payload the next send would post, from the counts so far.
    pub fn payload(&self, settings: &Settings, version: &str) -> Payload {
        let tally = self.tally.lock().unwrap();
        build_payload(
            &tally,
            settings,
            version,
            crate::system::private_memory_mb().unwrap_or(0),
        )
    }

    /// Write the counts if they changed.
    pub fn flush(&self) {
        if !self.is_on() || !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let tally = self.tally.lock().unwrap().clone();
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        let written = serde_json::to_vec(&tally)
            .map_err(std::io::Error::other)
            .and_then(|bytes| fs::write(&tmp, bytes))
            .and_then(|()| fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            eprintln!("usage stats: can't save ({e})");
        }
    }

    /// A day of counting has passed (with `jitter` added so installs don't all send at once).
    fn due(&self, now: u64, jitter: u64) -> bool {
        let since = self.tally.lock().unwrap().since;
        now < since || now - since >= DAY_SECS + jitter
    }

    /// Start a new day after an accepted send.
    fn sent(&self) {
        let mut tally = self.tally.lock().unwrap();
        *tally = Tally {
            since: now_secs(),
            ..Tally::default()
        };
        drop(tally);
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();
    }
}

fn build_payload(tally: &Tally, settings: &Settings, version: &str, private_mb: u64) -> Payload {
    Payload {
        schema: SCHEMA,
        version: version.to_string(),
        target: crate::net::target(),
        period: "day",
        counters: tally.counters.clone(),
        settings: SettingsSnapshot::of(settings),
        perf: Perf {
            decide_p99_us: p99(&tally.decide_us),
            inject_p99_us: p99(&tally.inject_us),
            idle_private_mb: private_mb,
        },
        health: tally.health.clone(),
    }
}

/// Stats go out only when the user's switch (and policy) allows it and the remote config hasn't
/// paused them. Remote config can stop sending; it can never start it.
pub fn may_send(user_on: bool, flags: crate::remote::RemoteFlags) -> bool {
    user_on && flags.stats_enabled
}

/// The server's answer: `202 { "accepted": true }`, or `accepted: false` when its flood limit
/// dropped the post (sent again at the next tick).
#[derive(Deserialize)]
struct Answer {
    accepted: bool,
}

/// Post one payload. True when the server accepted it.
async fn send(client: &reqwest::Client, payload: &Payload) -> bool {
    let body = match serde_json::to_vec(payload) {
        Ok(b) if b.len() <= MAX_BODY => b,
        _ => return false,
    };
    let url = format!("{}/events", crate::links::API_BASE);
    match client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(r) if r.status() == reqwest::StatusCode::ACCEPTED => {
            r.json::<Answer>().await.is_ok_and(|a| a.accepted)
        }
        Ok(r) => {
            eprintln!("usage stats: {}", r.status());
            false
        }
        Err(e) => {
            eprintln!("usage stats: {}", e.without_url());
            false
        }
    }
}

/// The `unflub-stats` thread: every ten minutes it saves the local stats and the usage counts;
/// once a day it sends the usage stats while the switch is on and the server accepts them.
/// Sending is for release builds only.
pub fn spawn(app: tauri::AppHandle) {
    use crate::stats::Stats;
    use tauri::Manager;
    const TICK: Duration = Duration::from_secs(10 * 60);
    let spawned = std::thread::Builder::new()
        .name("unflub-stats".into())
        .spawn(move || {
            let jitter = crate::net::jitter(Duration::from_secs(2 * 60 * 60)).as_secs();
            loop {
                std::thread::sleep(TICK);
                let usage = app.state::<Usage>();
                app.state::<Stats>().flush();
                usage.flush();
                let flags = app.state::<crate::remote::Remote>().flags();
                // `is_on` already includes the user's switch and the machine policy.
                if cfg!(debug_assertions)
                    || !may_send(usage.is_on(), flags)
                    || !usage.due(now_secs(), jitter)
                {
                    continue;
                }
                let settings = crate::current_settings(&app);
                let version = app.package_info().version.to_string();
                // No identity headers on stats, not even the version (it is in the body).
                let Ok(client) = crate::net::client(Vec::new()) else {
                    continue;
                };
                let payload = usage.payload(&settings, &version);
                if tauri::async_runtime::block_on(send(&client, &payload)) {
                    usage.sent();
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("usage stats thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeSet;

    /// The contract's allowlist (`D:\Code\rmgn-api\docs\unflub-contract.md`, section 4), as
    /// dotted paths. Changing this list means changing the contract and the server first.
    const ALLOWLIST: &[&str] = &[
        "schema",
        "version",
        "target",
        "period",
        "counters",
        "counters.corrections",
        "counters.undos",
        "counters.suggestions_shown",
        "settings",
        "settings.engine",
        "settings.pill",
        "settings.theme",
        "settings.appearance",
        "settings.excluded_count",
        "settings.personal_words_count",
        "settings.rules_count",
        "settings.auto_update",
        "perf",
        "perf.decide_p99_us",
        "perf.inject_p99_us",
        "perf.idle_private_mb",
        "health",
        "health.crashes",
        "health.hook_reinstalls",
        "health.self_pauses",
    ];

    fn paths(value: &Value, prefix: &str, out: &mut BTreeSet<String>) {
        if let Value::Object(map) = value {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                paths(v, &path, out);
                out.insert(path);
            }
        }
    }

    fn sample_tally() -> Tally {
        let mut t = Tally {
            counters: Counters {
                corrections: 42,
                undos: 3,
                suggestions_shown: 40,
            },
            health: Health {
                crashes: 0,
                hook_reinstalls: 1,
                self_pauses: 0,
            },
            ..Tally::default()
        };
        for us in [80, 90, 95, 400] {
            push_sample(&mut t.decide_us, Duration::from_micros(us));
            push_sample(&mut t.inject_us, Duration::from_micros(us * 4));
        }
        t
    }

    #[test]
    fn payload_keys_are_exactly_the_allowlist() {
        let settings = Settings {
            personal_words: vec!["secretword".into()],
            user_rules: vec![("omw".into(), "on my way".into())],
            ..Settings::default()
        };
        let p = build_payload(&sample_tally(), &settings, "0.1.0", 20);
        let json = serde_json::to_value(&p).unwrap();
        let mut keys = BTreeSet::new();
        paths(&json, "", &mut keys);
        let want: BTreeSet<String> = ALLOWLIST.iter().map(|s| s.to_string()).collect();
        assert_eq!(keys, want);

        // Nothing the user typed or chose by name gets in, only counts and fixed enums.
        let text = json.to_string();
        for leak in ["secretword", "omw", "on my way", "exe", "\\\\", "/Users"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        for (path, value) in [
            ("/settings/engine", "both"),
            ("/settings/pill", "underline-capsule"),
            ("/settings/theme", "unflub"),
            ("/settings/appearance", "auto"),
            ("/target", crate::net::target()),
            ("/period", "day"),
        ] {
            assert_eq!(json.pointer(path).and_then(Value::as_str), Some(value));
        }
        assert_eq!(json["settings"]["personal_words_count"], 1);
        assert_eq!(json["perf"]["decide_p99_us"], 400);
        assert_eq!(json["perf"]["idle_private_mb"], 20);
    }

    #[test]
    fn payload_fits_the_size_limit() {
        let mut t = sample_tally();
        t.counters.corrections = u64::MAX;
        t.counters.undos = u64::MAX;
        t.counters.suggestions_shown = u64::MAX;
        t.health.crashes = u64::MAX;
        t.health.hook_reinstalls = u64::MAX;
        let settings = Settings {
            mode: EngineMode::BundledOnly,
            ..Settings::default()
        };
        let p = build_payload(&t, &settings, "100.200.300-beta.1000", u64::MAX);
        assert!(serde_json::to_vec(&p).unwrap().len() <= MAX_BODY);
    }

    #[test]
    fn p99_by_nearest_rank() {
        assert_eq!(p99(&[]), 0);
        assert_eq!(p99(&[5]), 5);
        let hundred: Vec<u32> = (1..=100).collect();
        assert_eq!(p99(&hundred), 99);
        let mut samples = Vec::new();
        for i in 0..(MAX_SAMPLES as u64 + 10) {
            push_sample(&mut samples, Duration::from_micros(i));
        }
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert_eq!(samples[0], 10, "oldest samples roll off");
    }

    #[test]
    fn counts_only_while_on_and_forgets_when_turned_off() {
        let dir = std::env::temp_dir().join(format!("unflub-usage-{}", std::process::id()));
        let usage = Usage::new(dir.clone(), false);
        usage.correction(Duration::from_micros(90), Duration::from_micros(400), true);
        assert_eq!(usage.tally.lock().unwrap().counters, Counters::default());

        usage.set_on(true);
        usage.correction(Duration::from_micros(90), Duration::from_micros(400), true);
        usage.correction(Duration::from_micros(90), Duration::from_micros(400), false);
        usage.undo();
        usage.flush();
        // Survives a restart.
        let again = Usage::new(dir.clone(), true);
        let c = again.tally.lock().unwrap().counters.clone();
        assert_eq!((c.corrections, c.undos, c.suggestions_shown), (2, 1, 1));

        again.set_on(false);
        assert!(!dir.join("usage.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sends_once_a_day() {
        let usage = Usage::new(std::env::temp_dir().join("unflub-usage-due"), false);
        let since = 1_800_000_000;
        usage.tally.lock().unwrap().since = since;
        assert!(!usage.due(since + 60, 0));
        assert!(usage.due(since + DAY_SECS, 0));
        assert!(!usage.due(since + DAY_SECS, 3600));
        assert!(
            usage.due(since - 10, 0),
            "a clock set back doesn't stop sending"
        );
    }
}
