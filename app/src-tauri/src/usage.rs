//! Anonymous usage stats: counts, coarse buckets and fixed enums, sent at most once a day while
//! "Share anonymous usage stats" is on (the default; the switch in Settings > Privacy turns it
//! off, and so does the `DisableUsageStats` machine policy). Requests carry no ID (`net.rs`).
//!
//! Only what [`Payload`] holds ever leaves the machine, and its keys are exactly the allowlist
//! in the API contract (a test checks). No words, no corrections, no app names (the engine turns
//! the focused app into a category before anything reaches this process), no paths, no dates.
//! While the switch is off nothing is counted, and turning it off deletes what was counted.
//!
//! One report covers one period: when a day of counting is over, its payload is frozen and a new
//! period starts at once. A frozen report that can't be sent yet waits (with a back-off) while
//! the next day is counted; at most one waits, and a newer one replaces it. While the remote
//! config pauses stats (or in dev builds), each finished day is dropped unsent.
//!
//! The engine thread records into [`Usage`]; the counts are kept in `usage.json` in the app data
//! folder so they survive restarts, and sent by the `unflub-stats` thread.

use crate::facts::TourState;
use crate::install::InstallSource;
use crate::settings::{Appearance, EngineMode, PillStyle, Settings, Theme};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unflub_engine::protocol::{AppCategory, FixSource, ReinstallReason, Trigger};

/// Bump with the contract.
pub const SCHEMA: u32 = 3;
/// The server refuses bigger bodies.
pub const MAX_BODY: usize = 2048;
const DAY_SECS: u64 = 24 * 60 * 60;
/// The API refuses counts above this for one day; numbers are capped here first.
const MAX_COUNT: u64 = 1_000_000;
/// Minutes in a day: the cap for `active_minutes` and `paused_minutes`.
const MAX_MINUTES: u64 = 24 * 60;

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
    pub env: Env,
    pub features: Features,
    pub updates: UpdateCounts,
}

/// Counts for the period.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counters {
    pub corrections: u64,
    pub undos: u64,
    /// Corrections the pill showed.
    pub suggestions_shown: u64,
    /// What ended a corrected word.
    pub trigger_space: u64,
    pub trigger_punctuation: u64,
    pub trigger_enter_tab: u64,
    /// A first Backspace after a fix was held (the pill asked "Undo?").
    pub undos_first_backspace: u64,
    /// Words learned after repeated undos.
    pub learned_pairs: u64,
    /// Words the user added to the personal dictionary in Settings.
    pub personal_words_added: u64,
    /// Which part of the engine made each fix.
    pub corrections_engine_core: u64,
    pub corrections_engine_os: u64,
    pub corrections_user_rule: u64,
    /// Minutes with autocorrect on and at least one key typed.
    pub active_minutes: u64,
    /// Times the user turned autocorrect off.
    pub switched_off: u64,
    /// Minutes autocorrect was off while unflub ran (filled in when the payload is built).
    pub paused_minutes: u64,
    /// Words not checked: in a password field, in an excluded app, by the code-skip rules.
    pub skipped_secure_field: u64,
    pub skipped_excluded_app: u64,
    pub skipped_code_context: u64,
    /// Corrections per kind of app.
    pub by_category_browser: u64,
    pub by_category_chat: u64,
    pub by_category_code_editor: u64,
    pub by_category_office: u64,
    pub by_category_notes: u64,
    pub by_category_email: u64,
    pub by_category_other: u64,
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

/// Microseconds, over the whole period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Perf {
    /// Decisions on a word-ending key that produced a fix.
    pub decide_p99_us: u64,
    pub decide_samples: u64,
    pub decide_p50_us: u64,
    /// Every decision on a word-ending key, fixed or not.
    pub decide_all_p99_us: u64,
    pub decide_all_samples: u64,
    /// The Windows spell checker's `Check` call alone.
    pub spellcheck_p99_us: u64,
    pub inject_p99_us: u64,
    pub idle_private_mb: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    /// unflub stopped on a panic (counted at the next start), or the engine's corrector panicked.
    pub crashes: u64,
    /// The keyboard hook was installed again (the sum of the three reasons below, plus any
    /// reinstall an older engine reported without a reason).
    pub hook_reinstalls: u64,
    /// The engine fell back to pass-through because its own injected keys stopped coming back
    /// (another program interferes with keyboard input).
    pub self_pauses: u64,
    /// The watchdog saw input the hooks missed.
    pub hook_reinstalls_watchdog: u64,
    /// The computer resumed from sleep.
    pub hook_reinstalls_resume: u64,
    /// The session was unlocked.
    pub hook_reinstalls_unlock: u64,
    /// An engine start couldn't install the keyboard hook (or never got ready).
    pub hook_unavailable: u64,
}

/// One value per report, from facts about this computer and install.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Env {
    pub install_source: InstallSource,
    pub days_since_install: &'static str,
    /// Left out when Windows doesn't say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows_build: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard_language: Option<&'static str>,
    pub ui_language: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_gb: Option<&'static str>,
    pub tour: TourState,
}

/// What [`Env`] is bucketed from. Never sent as is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvFacts {
    pub install_source: InstallSource,
    pub days_since_install: i64,
    /// The OS build number (`CurrentBuildNumber`).
    pub windows_build: Option<u32>,
    /// The `LANGID` of the foreground window's keyboard layout.
    pub keyboard_langid: Option<u16>,
    pub cpu_cores: Option<usize>,
    /// Installed memory, rounded to whole GB.
    pub ram_gb: Option<u64>,
    pub tour: TourState,
}

/// The app's UI languages (`app/src/i18n`). Anything else would be `other`.
const UI_LANGUAGE: &str = "en";

impl Env {
    pub fn of(f: &EnvFacts) -> Self {
        Self {
            install_source: f.install_source,
            days_since_install: days_bucket(f.days_since_install),
            windows_build: f.windows_build.map(build_bucket),
            keyboard_language: f.keyboard_langid.map(keyboard_language),
            ui_language: UI_LANGUAGE,
            cpu_cores: f.cpu_cores.map(cores_bucket),
            ram_gb: f.ram_gb.map(ram_bucket),
            tour: f.tour,
        }
    }
}

fn days_bucket(days: i64) -> &'static str {
    match days {
        ..=0 => "0",
        1..=6 => "1-6",
        7..=29 => "7-29",
        30..=89 => "30-89",
        _ => "90+",
    }
}

/// The contract's builds: 19045 (Windows 10 22H2), 22621 and 22631 (Windows 11 22H2, 23H2),
/// 26100 (24H2), 26200 (25H2). A build between two of them (Insider, servicing) counts as the
/// one below it, 22000 to 22620 is Windows 11 21H2 (`22000`), and anything after 26200 is
/// `other-newer`.
fn build_bucket(build: u32) -> &'static str {
    match build {
        0..=19044 => "<19045",
        19045..=21999 => "19045",
        22000..=22620 => "22000",
        22621..=22630 => "22621",
        22631..=26099 => "22631",
        26100..=26199 => "26100",
        26200 => "26200",
        _ => "other-newer",
    }
}

/// The primary language of a keyboard layout's `LANGID` (its low 10 bits).
fn keyboard_language(langid: u16) -> &'static str {
    match langid & 0x3ff {
        0x09 => "en",
        0x0c => "fr",
        0x0a => "es",
        0x07 => "de",
        0x16 => "pt",
        0x10 => "it",
        0x13 => "nl",
        _ => "other",
    }
}

fn cores_bucket(cores: usize) -> &'static str {
    match cores {
        0..=2 => "1-2",
        3..=4 => "3-4",
        5..=8 => "5-8",
        9..=16 => "9-16",
        _ => "17+",
    }
}

fn ram_bucket(gb: u64) -> &'static str {
    match gb {
        0..=4 => "<=4",
        5..=8 => "5-8",
        9..=16 => "9-16",
        17..=32 => "17-32",
        _ => "33+",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    pub settings_opens: u64,
    pub stats_page_views: u64,
    pub tour_replays: u64,
    pub theme_changes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateCounts {
    pub offered: u64,
    pub downloaded: u64,
    pub installed: u64,
    pub failed: u64,
    pub dismissed: u64,
}

/// A feature the user used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    SettingsOpen,
    StatsPageView,
    TourReplay,
    ThemeChange,
}

/// A step of the update funnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStep {
    /// A check found an update this run hadn't been offered yet.
    Offered,
    /// Downloaded and verified in the background.
    Downloaded,
    /// The installer was started.
    Installed,
    /// A download or install failed.
    Failed,
    /// The user put an offered update off.
    Dismissed,
}

/// A fix the engine made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    pub decide: Duration,
    pub inject: Duration,
    /// The pill was asked to show it.
    pub pill: bool,
    pub trigger: Trigger,
    pub source: FixSource,
    pub category: AppCategory,
}

/// What the engine counted since its last report (`FromEngine::UsageCounts`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineCounts {
    pub active_minutes: u64,
    pub skipped_secure: u64,
    pub skipped_excluded: u64,
    pub skipped_code: u64,
    pub decide_all_us: Vec<u32>,
    pub spellcheck_us: Vec<u32>,
}

/// Microsecond latencies over a whole period, in about 70 buckets: exact up to 63 us, then 16
/// buckets per power of two, so a percentile is at most ~6% below the true value. Kept sparse.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Hist {
    counts: BTreeMap<u16, u64>,
    max: u32,
}

const EXACT: u32 = 64;

fn bucket_of(us: u32) -> u16 {
    if us < EXACT {
        return us as u16;
    }
    let exp = 31 - us.leading_zeros(); // >= 6
    let sub = (us >> (exp - 4)) & 15;
    (EXACT + (exp - 6) * 16 + sub) as u16
}

/// The smallest value in bucket `b`.
fn bucket_floor(b: u16) -> u32 {
    let b = u32::from(b);
    if b < EXACT {
        return b;
    }
    let exp = (b - EXACT) / 16 + 6;
    let sub = (b - EXACT) % 16;
    (1 << exp) | (sub << (exp - 4))
}

impl Hist {
    fn record(&mut self, us: u32) {
        *self.counts.entry(bucket_of(us)).or_insert(0) += 1;
        self.max = self.max.max(us);
    }

    fn record_duration(&mut self, d: Duration) {
        self.record(u32::try_from(d.as_micros()).unwrap_or(u32::MAX));
    }

    fn count(&self) -> u64 {
        self.counts.values().sum()
    }

    /// The `p`th percentile (nearest rank, rounded down to its bucket), 0 with no samples.
    fn percentile(&self, p: f64) -> u64 {
        let total = self.count();
        if total == 0 {
            return 0;
        }
        let rank = ((p * total as f64).ceil() as u64).clamp(1, total);
        let mut seen = 0;
        for (&b, &c) in &self.counts {
            seen += c;
            if seen >= rank {
                return u64::from(bucket_floor(b).min(self.max));
            }
        }
        u64::from(self.max)
    }
}

/// What is kept on disk between sends.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Tally {
    counters: Counters,
    health: Health,
    features: Features,
    updates: UpdateCounts,
    decide: Hist,
    decide_all: Hist,
    spellcheck: Hist,
    inject: Hist,
    /// Seconds autocorrect was off this period, not counting a pause still going on.
    paused_secs: u64,
    /// While autocorrect is off: since when (folded into `paused_secs` at every save).
    paused_since: Option<u64>,
    /// When this period started (switch turned on, or the last period closed).
    since: u64,
    /// When the last report was accepted, for `X-First` (Unix seconds).
    last_report: Option<u64>,
    /// A finished period's report that hasn't been accepted yet.
    pending: Option<serde_json::Value>,
}

impl Tally {
    /// Count the pause so far into `paused_secs`.
    fn fold_pause(&mut self, now: u64) {
        if let Some(since) = self.paused_since {
            self.paused_secs += now.saturating_sub(since);
            self.paused_since = Some(now);
        }
    }

    /// A fresh period from `now`, keeping what spans periods.
    fn next_period(&self, now: u64) -> Self {
        Self {
            since: now,
            last_report: self.last_report,
            pending: self.pending.clone(),
            paused_since: self.paused_since.map(|_| now),
            ..Self::default()
        }
    }
}

pub struct Usage {
    path: PathBuf,
    on: AtomicBool,
    /// Autocorrect is off (mirrors the setting, so a new tally knows).
    paused: AtomicBool,
    tally: Mutex<Tally>,
    dirty: AtomicBool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
            paused: AtomicBool::new(false),
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
        let now = now_secs();
        let mut tally = self.tally.lock().unwrap();
        *tally = Tally {
            since: now,
            paused_since: (on && self.paused.load(Ordering::Relaxed)).then_some(now),
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

    /// A fix landed.
    pub fn correction(&self, fix: &Fix) {
        self.record(|t| {
            let c = &mut t.counters;
            c.corrections += 1;
            c.suggestions_shown += u64::from(fix.pill);
            *match fix.trigger {
                Trigger::Space => &mut c.trigger_space,
                Trigger::Punctuation => &mut c.trigger_punctuation,
                Trigger::EnterTab => &mut c.trigger_enter_tab,
            } += 1;
            *match fix.source {
                FixSource::Core => &mut c.corrections_engine_core,
                FixSource::Os => &mut c.corrections_engine_os,
                FixSource::UserRule => &mut c.corrections_user_rule,
            } += 1;
            *match fix.category {
                AppCategory::Browser => &mut c.by_category_browser,
                AppCategory::Chat => &mut c.by_category_chat,
                AppCategory::CodeEditor => &mut c.by_category_code_editor,
                AppCategory::Office => &mut c.by_category_office,
                AppCategory::Notes => &mut c.by_category_notes,
                AppCategory::Email => &mut c.by_category_email,
                AppCategory::Other => &mut c.by_category_other,
            } += 1;
            t.decide.record_duration(fix.decide);
            t.inject.record_duration(fix.inject);
        });
    }

    pub fn undo(&self) {
        self.record(|t| t.counters.undos += 1);
    }

    pub fn undo_armed(&self) {
        self.record(|t| t.counters.undos_first_backspace += 1);
    }

    pub fn learned_pair(&self) {
        self.record(|t| t.counters.learned_pairs += 1);
    }

    pub fn personal_words_added(&self, n: u64) {
        if n > 0 {
            self.record(|t| t.counters.personal_words_added += n);
        }
    }

    /// Autocorrect was turned off (`true`) or on. Counts the switch-off and the paused time.
    pub fn set_paused(&self, paused: bool) {
        let was = self.paused.swap(paused, Ordering::Relaxed);
        self.record(|t| {
            let now = now_secs();
            t.fold_pause(now);
            if paused && !was {
                t.counters.switched_off += 1;
            }
            t.paused_since = paused.then_some(now);
        });
    }

    /// At startup: autocorrect is off (`true`) or on, without counting a switch. Time while
    /// unflub wasn't running is never paused time.
    pub fn start_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
        self.record(|t| t.paused_since = paused.then(now_secs));
    }

    pub fn engine_counts(&self, e: &EngineCounts) {
        self.record(|t| {
            let c = &mut t.counters;
            c.active_minutes += e.active_minutes;
            c.skipped_secure_field += e.skipped_secure;
            c.skipped_excluded_app += e.skipped_excluded;
            c.skipped_code_context += e.skipped_code;
            for &us in &e.decide_all_us {
                t.decide_all.record(us);
            }
            for &us in &e.spellcheck_us {
                t.spellcheck.record(us);
            }
        });
    }

    pub fn crash(&self) {
        self.record(|t| t.health.crashes += 1);
    }

    pub fn hook_reinstall(&self, reason: Option<ReinstallReason>) {
        self.record(|t| {
            let h = &mut t.health;
            h.hook_reinstalls += 1;
            match reason {
                Some(ReinstallReason::Watchdog) => h.hook_reinstalls_watchdog += 1,
                Some(ReinstallReason::Resume) => h.hook_reinstalls_resume += 1,
                Some(ReinstallReason::Unlock) => h.hook_reinstalls_unlock += 1,
                None => {}
            }
        });
    }

    pub fn hook_unavailable(&self) {
        self.record(|t| t.health.hook_unavailable += 1);
    }

    pub fn self_pause(&self) {
        self.record(|t| t.health.self_pauses += 1);
    }

    pub fn feature(&self, feature: Feature) {
        self.record(|t| {
            let f = &mut t.features;
            *match feature {
                Feature::SettingsOpen => &mut f.settings_opens,
                Feature::StatsPageView => &mut f.stats_page_views,
                Feature::TourReplay => &mut f.tour_replays,
                Feature::ThemeChange => &mut f.theme_changes,
            } += 1;
        });
    }

    pub fn update(&self, step: UpdateStep) {
        self.record(|t| {
            let u = &mut t.updates;
            *match step {
                UpdateStep::Offered => &mut u.offered,
                UpdateStep::Downloaded => &mut u.downloaded,
                UpdateStep::Installed => &mut u.installed,
                UpdateStep::Failed => &mut u.failed,
                UpdateStep::Dismissed => &mut u.dismissed,
            } += 1;
        });
    }

    /// The payload the current period would post, from the counts so far.
    pub fn payload(&self, settings: &Settings, version: &str, env: &EnvFacts) -> Payload {
        let tally = self.tally.lock().unwrap();
        build_payload(
            &tally,
            settings,
            version,
            crate::system::private_memory_mb().unwrap_or(0),
            env,
            now_secs(),
        )
    }

    /// Write the counts if they changed.
    pub fn flush(&self) {
        if !self.is_on() {
            return;
        }
        {
            let mut tally = self.tally.lock().unwrap();
            if tally.paused_since.is_some() {
                tally.fold_pause(now_secs());
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let tally = self.tally.lock().unwrap().clone();
        let written = serde_json::to_vec(&tally)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::files::replace(&self.path, &bytes));
        if let Err(e) = written {
            eprintln!("usage stats: can't save ({e})");
        }
    }

    /// A day of counting has passed (with `jitter` added so installs don't all send at once).
    fn due(&self, now: u64, jitter: u64) -> bool {
        let since = self.tally.lock().unwrap().since;
        now < since || now - since >= DAY_SECS + jitter
    }

    /// End the period: freeze its report (replacing one still waiting) and start the next.
    fn close_period(&self, settings: &Settings, version: &str, env: &EnvFacts) {
        let now = now_secs();
        let mut tally = self.tally.lock().unwrap();
        tally.fold_pause(now);
        let payload = build_payload(
            &tally,
            settings,
            version,
            crate::system::private_memory_mb().unwrap_or(0),
            env,
            now,
        );
        let mut next = tally.next_period(now);
        next.pending = serde_json::to_value(&payload).ok();
        *tally = next;
        drop(tally);
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();
    }

    /// End the period without a report (sending is paused).
    fn drop_period(&self) {
        let now = now_secs();
        let mut tally = self.tally.lock().unwrap();
        let next = tally.next_period(now);
        *tally = next;
        drop(tally);
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();
    }

    /// The finished period's report waiting to be sent.
    fn pending(&self) -> Option<serde_json::Value> {
        self.tally.lock().unwrap().pending.clone()
    }

    /// Which of `week` and `month` this report is the first of (`X-First`, contract section 2).
    fn first(&self, now: u64) -> Vec<&'static str> {
        crate::net::first_report(self.tally.lock().unwrap().last_report, now)
    }

    /// Drop a report the server refused as malformed (400 or 413): sending it again would fail
    /// the same way. It doesn't count as reported.
    fn dropped(&self) {
        self.tally.lock().unwrap().pending = None;
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();
    }

    /// The waiting report was accepted.
    fn sent(&self) {
        {
            let mut tally = self.tally.lock().unwrap();
            tally.pending = None;
            tally.last_report = Some(now_secs());
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();
    }
}

fn cap(n: u64) -> u64 {
    n.min(MAX_COUNT)
}

/// Every count capped as the API expects.
fn capped(c: &Counters, paused_minutes: u64) -> Counters {
    Counters {
        corrections: cap(c.corrections),
        undos: cap(c.undos),
        suggestions_shown: cap(c.suggestions_shown),
        trigger_space: cap(c.trigger_space),
        trigger_punctuation: cap(c.trigger_punctuation),
        trigger_enter_tab: cap(c.trigger_enter_tab),
        undos_first_backspace: cap(c.undos_first_backspace),
        learned_pairs: cap(c.learned_pairs),
        personal_words_added: cap(c.personal_words_added),
        corrections_engine_core: cap(c.corrections_engine_core),
        corrections_engine_os: cap(c.corrections_engine_os),
        corrections_user_rule: cap(c.corrections_user_rule),
        active_minutes: c.active_minutes.min(MAX_MINUTES),
        switched_off: cap(c.switched_off),
        paused_minutes: paused_minutes.min(MAX_MINUTES),
        skipped_secure_field: cap(c.skipped_secure_field),
        skipped_excluded_app: cap(c.skipped_excluded_app),
        skipped_code_context: cap(c.skipped_code_context),
        by_category_browser: cap(c.by_category_browser),
        by_category_chat: cap(c.by_category_chat),
        by_category_code_editor: cap(c.by_category_code_editor),
        by_category_office: cap(c.by_category_office),
        by_category_notes: cap(c.by_category_notes),
        by_category_email: cap(c.by_category_email),
        by_category_other: cap(c.by_category_other),
    }
}

fn build_payload(
    tally: &Tally,
    settings: &Settings,
    version: &str,
    private_mb: u64,
    env: &EnvFacts,
    now: u64,
) -> Payload {
    let ongoing = tally.paused_since.map_or(0, |s| now.saturating_sub(s));
    let h = &tally.health;
    let f = &tally.features;
    let u = &tally.updates;
    Payload {
        schema: SCHEMA,
        version: version.to_string(),
        target: crate::net::target(),
        period: "day",
        counters: capped(&tally.counters, (tally.paused_secs + ongoing) / 60),
        settings: SettingsSnapshot::of(settings),
        perf: Perf {
            decide_p99_us: cap(tally.decide.percentile(0.99)),
            decide_samples: cap(tally.decide.count()),
            decide_p50_us: cap(tally.decide.percentile(0.50)),
            decide_all_p99_us: cap(tally.decide_all.percentile(0.99)),
            decide_all_samples: cap(tally.decide_all.count()),
            spellcheck_p99_us: cap(tally.spellcheck.percentile(0.99)),
            inject_p99_us: cap(tally.inject.percentile(0.99)),
            idle_private_mb: cap(private_mb),
        },
        health: Health {
            crashes: cap(h.crashes),
            hook_reinstalls: cap(h.hook_reinstalls),
            self_pauses: cap(h.self_pauses),
            hook_reinstalls_watchdog: cap(h.hook_reinstalls_watchdog),
            hook_reinstalls_resume: cap(h.hook_reinstalls_resume),
            hook_reinstalls_unlock: cap(h.hook_reinstalls_unlock),
            hook_unavailable: cap(h.hook_unavailable),
        },
        env: Env::of(env),
        features: Features {
            settings_opens: cap(f.settings_opens),
            stats_page_views: cap(f.stats_page_views),
            tour_replays: cap(f.tour_replays),
            theme_changes: cap(f.theme_changes),
        },
        updates: UpdateCounts {
            offered: cap(u.offered),
            downloaded: cap(u.downloaded),
            installed: cap(u.installed),
            failed: cap(u.failed),
            dismissed: cap(u.dismissed),
        },
    }
}

/// Stats go out only when the user's switch (and policy) allows it and the remote config hasn't
/// paused them. Remote config can stop sending; it can never start it.
pub fn may_send(user_on: bool, flags: crate::remote::RemoteFlags) -> bool {
    user_on && flags.stats_enabled
}

/// The server's answer to an accepted post: `202 { "accepted": true }`. Per-IP limits answer with
/// an HTTP error instead (Cloudflare WAF or the Workers daily limit).
#[derive(Deserialize)]
struct Answer {
    accepted: bool,
}

/// What to do after a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Counted: the report is done.
    Accepted,
    /// The server can never accept this report (400, 413, or the body was too large to send):
    /// drop it.
    Rejected,
    /// Network error, rate limit, 5xx or an unexpected answer: try again after a back-off.
    Retry,
}

impl Outcome {
    fn of(status: u16, accepted: Option<bool>) -> Self {
        match (status, accepted) {
            (202, Some(true)) => Self::Accepted,
            (400 | 413 | 422, _) => Self::Rejected,
            _ => Self::Retry,
        }
    }
}

/// Wait between failed sends: 10 minutes, doubling to one day, reset by any other outcome. Kept in
/// memory only, so a restart tries again at the next tick.
#[derive(Debug, Default)]
struct Backoff {
    wait: u64,
    until: u64,
}

impl Backoff {
    const MIN: u64 = 10 * 60;

    fn ready(&self, now: u64) -> bool {
        now >= self.until
    }

    fn failed(&mut self, now: u64) {
        self.wait = (self.wait * 2).clamp(Self::MIN, DAY_SECS);
        self.until = now + self.wait;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// `POST https://api.rmgn.net/unflub/v1/events`.
fn events_url() -> String {
    format!("{}/events", crate::links::API_BASE)
}

/// Post one report.
async fn send(client: &reqwest::Client, report: &serde_json::Value) -> Outcome {
    let body = match serde_json::to_vec(report) {
        Ok(b) if b.len() <= MAX_BODY => b,
        _ => return Outcome::Rejected,
    };
    let url = events_url();
    match client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(r) if r.status() == reqwest::StatusCode::ACCEPTED => {
            let accepted = r.json::<Answer>().await.ok().map(|a| a.accepted);
            Outcome::of(202, accepted)
        }
        Ok(r) => {
            eprintln!("usage stats: {}", r.status());
            Outcome::of(r.status().as_u16(), None)
        }
        Err(e) => {
            eprintln!("usage stats: {}", e.without_url());
            Outcome::Retry
        }
    }
}

/// The `unflub-stats` thread: every ten minutes it saves the local stats and the usage counts;
/// once a day it closes the period and sends its report while the switch is on and the server
/// accepts them. Sending is for release builds only.
pub fn spawn(app: tauri::AppHandle) {
    use crate::stats::Stats;
    use tauri::Manager;
    const TICK: Duration = Duration::from_secs(10 * 60);
    let spawned = std::thread::Builder::new()
        .name("unflub-stats".into())
        .spawn(move || {
            let jitter = crate::net::jitter(Duration::from_secs(2 * 60 * 60)).as_secs();
            let mut backoff = Backoff::default();
            loop {
                std::thread::sleep(TICK);
                let usage = app.state::<Usage>();
                app.state::<Stats>().flush();
                usage.flush();
                let flags = app.state::<crate::remote::Remote>().flags();
                // `is_on` already includes the user's switch and the machine policy.
                let allowed = !cfg!(debug_assertions) && may_send(usage.is_on(), flags);
                if usage.due(now_secs(), jitter) {
                    if allowed {
                        let settings = crate::current_settings(&app);
                        let version = app.package_info().version.to_string();
                        usage.close_period(&settings, &version, &crate::env_facts(&app));
                    } else {
                        usage.drop_period();
                    }
                }
                if !allowed || !backoff.ready(now_secs()) {
                    continue;
                }
                let Some(report) = usage.pending() else {
                    continue;
                };
                // No identity headers on stats, not even the version (it is in the body); only
                // whether this is the first report of the ISO week or month.
                let headers = crate::net::first_header(&usage.first(now_secs()));
                let Ok(client) = crate::net::client(headers) else {
                    continue;
                };
                match tauri::async_runtime::block_on(send(&client, &report)) {
                    Outcome::Accepted => {
                        backoff.reset();
                        usage.sent();
                    }
                    Outcome::Rejected => {
                        backoff.reset();
                        usage.dropped();
                    }
                    Outcome::Retry => backoff.failed(now_secs()),
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

    #[test]
    fn outcome_retries_only_what_can_succeed_later() {
        assert_eq!(Outcome::of(202, Some(true)), Outcome::Accepted);
        assert_eq!(Outcome::of(202, Some(false)), Outcome::Retry);
        assert_eq!(Outcome::of(202, None), Outcome::Retry);
        for bad in [400, 413, 422] {
            assert_eq!(Outcome::of(bad, None), Outcome::Rejected, "{bad}");
        }
        for later in [429, 500, 502, 503, 404] {
            assert_eq!(Outcome::of(later, None), Outcome::Retry, "{later}");
        }
    }

    #[test]
    fn backoff_doubles_to_a_day_and_resets() {
        let mut b = Backoff::default();
        assert!(b.ready(0));
        b.failed(1_000);
        assert!(!b.ready(1_000 + Backoff::MIN - 1));
        assert!(b.ready(1_000 + Backoff::MIN));
        let mut waits = Vec::new();
        for _ in 0..12 {
            b.failed(0);
            waits.push(b.wait);
        }
        assert_eq!(waits[0], 2 * Backoff::MIN);
        assert_eq!(*waits.last().unwrap(), DAY_SECS);
        assert!(waits.windows(2).all(|w| w[1] >= w[0]));
        b.reset();
        assert!(b.ready(0));
    }

    /// The contract's allowlist (`D:\Code\rmgn-api\docs\unflub-contract.md`, section 2, schema
    /// 3), as dotted paths. Changing this list means changing the contract and the server first.
    const ALLOWLIST: &[&str] = &[
        "schema",
        "version",
        "target",
        "period",
        "counters",
        "counters.corrections",
        "counters.undos",
        "counters.suggestions_shown",
        "counters.trigger_space",
        "counters.trigger_punctuation",
        "counters.trigger_enter_tab",
        "counters.undos_first_backspace",
        "counters.learned_pairs",
        "counters.personal_words_added",
        "counters.corrections_engine_core",
        "counters.corrections_engine_os",
        "counters.corrections_user_rule",
        "counters.active_minutes",
        "counters.switched_off",
        "counters.paused_minutes",
        "counters.skipped_secure_field",
        "counters.skipped_excluded_app",
        "counters.skipped_code_context",
        "counters.by_category_browser",
        "counters.by_category_chat",
        "counters.by_category_code_editor",
        "counters.by_category_office",
        "counters.by_category_notes",
        "counters.by_category_email",
        "counters.by_category_other",
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
        "perf.decide_samples",
        "perf.decide_p50_us",
        "perf.decide_all_p99_us",
        "perf.decide_all_samples",
        "perf.spellcheck_p99_us",
        "perf.inject_p99_us",
        "perf.idle_private_mb",
        "health",
        "health.crashes",
        "health.hook_reinstalls",
        "health.self_pauses",
        "health.hook_reinstalls_watchdog",
        "health.hook_reinstalls_resume",
        "health.hook_reinstalls_unlock",
        "health.hook_unavailable",
        "env",
        "env.install_source",
        "env.days_since_install",
        "env.windows_build",
        "env.keyboard_language",
        "env.ui_language",
        "env.cpu_cores",
        "env.ram_gb",
        "env.tour",
        "features",
        "features.settings_opens",
        "features.stats_page_views",
        "features.tour_replays",
        "features.theme_changes",
        "updates",
        "updates.offered",
        "updates.downloaded",
        "updates.installed",
        "updates.failed",
        "updates.dismissed",
    ];

    /// The enum fields and every value the contract allows for them.
    const ENUMS: &[(&str, &[&str])] = &[
        (
            "/env/install_source",
            &[
                "installer",
                "winget",
                "chocolatey",
                "scoop",
                "store",
                "portable",
            ],
        ),
        (
            "/env/days_since_install",
            &["0", "1-6", "7-29", "30-89", "90+"],
        ),
        (
            "/env/windows_build",
            &[
                "<19045",
                "19045",
                "22000",
                "22621",
                "22631",
                "26100",
                "26200",
                "other-newer",
            ],
        ),
        (
            "/env/keyboard_language",
            &["en", "fr", "es", "de", "pt", "it", "nl", "other"],
        ),
        ("/env/ui_language", &["en", "other"]),
        ("/env/cpu_cores", &["1-2", "3-4", "5-8", "9-16", "17+"]),
        ("/env/ram_gb", &["<=4", "5-8", "9-16", "17-32", "33+"]),
        ("/env/tour", &["completed", "skipped", "not_started"]),
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

    fn env_facts() -> EnvFacts {
        EnvFacts {
            install_source: InstallSource::Installer,
            days_since_install: 12,
            windows_build: Some(26100),
            keyboard_langid: Some(0x0c0c),
            cpu_cores: Some(8),
            ram_gb: Some(16),
            tour: TourState::Completed,
        }
    }

    fn fix(source: FixSource, category: AppCategory) -> Fix {
        Fix {
            decide: Duration::from_micros(90),
            inject: Duration::from_micros(400),
            pill: true,
            trigger: Trigger::Space,
            source,
            category,
        }
    }

    fn sample_tally() -> Tally {
        let mut t = Tally {
            counters: Counters {
                corrections: 42,
                undos: 3,
                suggestions_shown: 40,
                ..Counters::default()
            },
            health: Health {
                hook_reinstalls: 1,
                hook_reinstalls_resume: 1,
                ..Health::default()
            },
            ..Tally::default()
        };
        for us in [80, 90, 95, 400] {
            t.decide.record(us);
            t.inject.record(us * 4);
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
        let p = build_payload(&sample_tally(), &settings, "0.1.0", 20, &env_facts(), 0);
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
            ("/schema", Value::from(3)),
            ("/settings/engine", Value::from("both")),
            ("/settings/pill", Value::from("underline-capsule")),
            ("/settings/theme", Value::from("unflub")),
            ("/settings/appearance", Value::from("auto")),
            ("/target", Value::from(crate::net::target())),
            ("/period", Value::from("day")),
            ("/env/install_source", Value::from("installer")),
            ("/env/days_since_install", Value::from("7-29")),
            ("/env/windows_build", Value::from("26100")),
            ("/env/keyboard_language", Value::from("fr")),
            ("/env/ui_language", Value::from("en")),
            ("/env/cpu_cores", Value::from("5-8")),
            ("/env/ram_gb", Value::from("9-16")),
            ("/env/tour", Value::from("completed")),
        ] {
            assert_eq!(json.pointer(path), Some(&value), "{path}");
        }
        assert_eq!(json["settings"]["personal_words_count"], 1);
        assert_eq!(json["perf"]["decide_p99_us"], 400);
        assert_eq!(json["perf"]["decide_samples"], 4);
        assert_eq!(json["perf"]["decide_p50_us"], 88, "90 us in its ~6% bucket");
        assert_eq!(json["perf"]["idle_private_mb"], 20);
        assert_eq!(json["health"]["hook_reinstalls_resume"], 1);
    }

    #[test]
    fn every_enum_value_is_in_the_contract() {
        let settings = Settings::default();
        let mut facts = Vec::new();
        for source in [
            InstallSource::Installer,
            InstallSource::Winget,
            InstallSource::Chocolatey,
            InstallSource::Scoop,
            InstallSource::Store,
            InstallSource::Portable,
        ] {
            facts.push(EnvFacts {
                install_source: source,
                ..env_facts()
            });
        }
        for days in [-3, 0, 1, 6, 7, 29, 30, 89, 90, 4000] {
            facts.push(EnvFacts {
                days_since_install: days,
                ..env_facts()
            });
        }
        for build in [
            0,
            17763,
            19044,
            19045,
            22000,
            22621,
            22631,
            22635,
            26100,
            26120,
            26200,
            26300,
            u32::MAX,
        ] {
            facts.push(EnvFacts {
                windows_build: Some(build),
                ..env_facts()
            });
        }
        for langid in [
            0x0409, 0x0809, 0x040c, 0x0c0a, 0x0407, 0x0416, 0x0816, 0x0410, 0x0413, 0x0411, 0,
        ] {
            facts.push(EnvFacts {
                keyboard_langid: Some(langid),
                ..env_facts()
            });
        }
        for cores in [0, 1, 2, 3, 4, 5, 8, 9, 16, 17, 128] {
            facts.push(EnvFacts {
                cpu_cores: Some(cores),
                ..env_facts()
            });
        }
        for gb in [0, 4, 5, 8, 9, 16, 17, 32, 33, 1024] {
            facts.push(EnvFacts {
                ram_gb: Some(gb),
                ..env_facts()
            });
        }
        for tour in [
            TourState::NotStarted,
            TourState::Completed,
            TourState::Skipped,
        ] {
            facts.push(EnvFacts {
                tour,
                ..env_facts()
            });
        }
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        for f in &facts {
            let json = serde_json::to_value(build_payload(
                &Tally::default(),
                &settings,
                "0.1.0",
                20,
                f,
                0,
            ))
            .unwrap();
            for (path, allowed) in ENUMS {
                let v = json.pointer(path).and_then(Value::as_str).unwrap();
                assert!(allowed.contains(&v), "{path} = {v}");
                seen.insert((path.to_string(), v.to_string()));
            }
        }
        // Every allowed value is reachable (but `other` for the UI language, which has one
        // locale today).
        for (path, allowed) in ENUMS {
            for v in *allowed {
                if (*path, *v) == ("/env/ui_language", "other") {
                    continue;
                }
                assert!(
                    seen.contains(&(path.to_string(), v.to_string())),
                    "{path} never {v}"
                );
            }
        }
    }

    #[test]
    fn buckets_have_the_right_edges() {
        assert_eq!(days_bucket(0), "0");
        assert_eq!(days_bucket(1), "1-6");
        assert_eq!(days_bucket(6), "1-6");
        assert_eq!(days_bucket(7), "7-29");
        assert_eq!(days_bucket(30), "30-89");
        assert_eq!(days_bucket(90), "90+");
        assert_eq!(build_bucket(19044), "<19045");
        assert_eq!(build_bucket(19045), "19045");
        assert_eq!(build_bucket(21999), "19045");
        assert_eq!(build_bucket(22000), "22000", "Windows 11 21H2");
        assert_eq!(build_bucket(22620), "22000");
        assert_eq!(build_bucket(22621), "22621");
        assert_eq!(build_bucket(22631), "22631");
        assert_eq!(build_bucket(22635), "22631", "an Insider build of 23H2");
        assert_eq!(build_bucket(26100), "26100");
        assert_eq!(build_bucket(26200), "26200");
        assert_eq!(build_bucket(26300), "other-newer");
        assert_eq!(keyboard_language(0x0409), "en");
        assert_eq!(keyboard_language(0x1009), "en", "English (Canada)");
        assert_eq!(keyboard_language(0x0c0c), "fr", "French (Canada)");
        assert_eq!(keyboard_language(0x0416), "pt");
        assert_eq!(keyboard_language(0x0419), "other", "Russian");
        assert_eq!(cores_bucket(2), "1-2");
        assert_eq!(cores_bucket(3), "3-4");
        assert_eq!(cores_bucket(16), "9-16");
        assert_eq!(cores_bucket(17), "17+");
        assert_eq!(ram_bucket(4), "<=4");
        assert_eq!(ram_bucket(8), "5-8");
        assert_eq!(ram_bucket(16), "9-16");
        assert_eq!(ram_bucket(32), "17-32");
        assert_eq!(ram_bucket(64), "33+");
    }

    #[test]
    fn payload_fits_the_size_limit() {
        let mut t = sample_tally();
        let max = u64::MAX;
        let c = &mut t.counters;
        for n in [
            &mut c.corrections,
            &mut c.undos,
            &mut c.suggestions_shown,
            &mut c.trigger_space,
            &mut c.trigger_punctuation,
            &mut c.trigger_enter_tab,
            &mut c.undos_first_backspace,
            &mut c.learned_pairs,
            &mut c.personal_words_added,
            &mut c.corrections_engine_core,
            &mut c.corrections_engine_os,
            &mut c.corrections_user_rule,
            &mut c.active_minutes,
            &mut c.switched_off,
            &mut c.skipped_secure_field,
            &mut c.skipped_excluded_app,
            &mut c.skipped_code_context,
            &mut c.by_category_browser,
            &mut c.by_category_chat,
            &mut c.by_category_code_editor,
            &mut c.by_category_office,
            &mut c.by_category_notes,
            &mut c.by_category_email,
            &mut c.by_category_other,
        ] {
            *n = max;
        }
        t.health = Health {
            crashes: max,
            hook_reinstalls: max,
            self_pauses: max,
            hook_reinstalls_watchdog: max,
            hook_reinstalls_resume: max,
            hook_reinstalls_unlock: max,
            hook_unavailable: max,
        };
        t.features = Features {
            settings_opens: max,
            stats_page_views: max,
            tour_replays: max,
            theme_changes: max,
        };
        t.updates = UpdateCounts {
            offered: max,
            downloaded: max,
            installed: max,
            failed: max,
            dismissed: max,
        };
        t.paused_secs = max / 2;
        for h in [
            &mut t.decide,
            &mut t.decide_all,
            &mut t.spellcheck,
            &mut t.inject,
        ] {
            h.record(u32::MAX);
        }
        let settings = Settings {
            mode: EngineMode::BundledOnly,
            pill_style: PillStyle::UnderlineCapsule,
            ..Settings::default()
        };
        let env = EnvFacts {
            install_source: InstallSource::Chocolatey,
            windows_build: Some(26300),
            tour: TourState::NotStarted,
            ..env_facts()
        };
        let p = build_payload(&t, &settings, "100.200.300-beta.1000", u64::MAX, &env, 0);
        let json = serde_json::to_value(&p).unwrap();
        // Capped as the API expects.
        assert_eq!(json["counters"]["corrections"], MAX_COUNT);
        assert_eq!(json["counters"]["active_minutes"], MAX_MINUTES);
        assert_eq!(json["counters"]["paused_minutes"], MAX_MINUTES);
        assert_eq!(json["perf"]["decide_p99_us"], MAX_COUNT);
        let len = serde_json::to_vec(&p).unwrap().len();
        assert!(len <= MAX_BODY, "{len} bytes");
    }

    #[test]
    fn percentiles_by_nearest_rank_within_a_bucket() {
        let mut h = Hist::default();
        assert_eq!(h.percentile(0.99), 0);
        h.record(5);
        assert_eq!(h.percentile(0.99), 5);
        let mut h = Hist::default();
        for us in 1..=100 {
            h.record(us);
        }
        assert_eq!(h.percentile(0.99), 96, "99 in the 96-99 bucket");
        assert_eq!(h.percentile(0.50), 50, "exact below 64");
        assert_eq!(h.count(), 100);
        // Buckets: monotonic, each floor at or below its values, within ~6%.
        let mut last = 0;
        for us in (0..70_000u32).chain([1 << 20, u32::MAX]) {
            let b = bucket_of(us);
            assert!(b >= last, "{us}");
            let floor = bucket_floor(b);
            assert!(
                floor <= us && f64::from(us - floor) <= f64::from(us) * 0.0625,
                "{us}"
            );
            last = b;
        }
        // A whole day of samples, none rolling off: the period's p99, not the last few hundred.
        let mut h = Hist::default();
        for _ in 0..9_900 {
            h.record(80);
        }
        for _ in 0..100 {
            h.record(5_000);
        }
        for _ in 0..1_000 {
            h.record(80);
        }
        assert_eq!(h.count(), 11_000);
        assert_eq!(h.percentile(0.99), 80);
        assert!(h.percentile(0.995) >= 4_700);
    }

    #[test]
    fn counts_only_while_on_and_forgets_when_turned_off() {
        let dir = std::env::temp_dir().join(format!("unflub-usage-{}", std::process::id()));
        let usage = Usage::new(dir.clone(), false);
        usage.correction(&fix(FixSource::Core, AppCategory::Browser));
        assert_eq!(usage.tally.lock().unwrap().counters, Counters::default());

        usage.set_on(true);
        usage.correction(&fix(FixSource::Core, AppCategory::Browser));
        usage.correction(&Fix {
            pill: false,
            trigger: Trigger::EnterTab,
            ..fix(FixSource::UserRule, AppCategory::Chat)
        });
        usage.undo();
        usage.flush();
        // Survives a restart.
        let again = Usage::new(dir.clone(), true);
        let t = again.tally.lock().unwrap().clone();
        let c = &t.counters;
        assert_eq!((c.corrections, c.undos, c.suggestions_shown), (2, 1, 1));
        assert_eq!((c.trigger_space, c.trigger_enter_tab), (1, 1));
        assert_eq!((c.corrections_engine_core, c.corrections_user_rule), (1, 1));
        assert_eq!((c.by_category_browser, c.by_category_chat), (1, 1));
        assert_eq!(t.decide.count(), 2);

        again.set_on(false);
        assert!(!dir.join("usage.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn health_counts_reinstalls_by_reason() {
        let usage = Usage::new(std::env::temp_dir().join("unflub-usage-health"), false);
        usage.set_on(true);
        usage.hook_reinstall(Some(ReinstallReason::Watchdog));
        usage.hook_reinstall(Some(ReinstallReason::Watchdog));
        usage.hook_reinstall(Some(ReinstallReason::Unlock));
        usage.hook_reinstall(None);
        usage.hook_unavailable();
        let h = usage.tally.lock().unwrap().health.clone();
        assert_eq!(h.hook_reinstalls, 4);
        assert_eq!(
            (
                h.hook_reinstalls_watchdog,
                h.hook_reinstalls_resume,
                h.hook_reinstalls_unlock
            ),
            (2, 0, 1)
        );
        assert_eq!(h.hook_unavailable, 1, "not a reinstall");
        usage.set_on(false);
    }

    #[test]
    fn engine_counts_features_and_updates_add_up() {
        let usage = Usage::new(std::env::temp_dir().join("unflub-usage-adds"), false);
        usage.set_on(true);
        usage.engine_counts(&EngineCounts {
            active_minutes: 3,
            skipped_secure: 1,
            skipped_excluded: 2,
            skipped_code: 4,
            decide_all_us: vec![50, 60, 62],
            spellcheck_us: vec![40],
        });
        usage.undo_armed();
        usage.learned_pair();
        usage.personal_words_added(2);
        usage.personal_words_added(0);
        usage.feature(Feature::SettingsOpen);
        usage.feature(Feature::ThemeChange);
        usage.update(UpdateStep::Offered);
        usage.update(UpdateStep::Dismissed);
        let p = build_payload(
            &usage.tally.lock().unwrap(),
            &Settings::default(),
            "0.1.0",
            0,
            &env_facts(),
            now_secs(),
        );
        let c = &p.counters;
        assert_eq!(
            (
                c.active_minutes,
                c.skipped_secure_field,
                c.skipped_excluded_app,
                c.skipped_code_context
            ),
            (3, 1, 2, 4)
        );
        assert_eq!(
            (
                c.undos_first_backspace,
                c.learned_pairs,
                c.personal_words_added
            ),
            (1, 1, 2)
        );
        assert_eq!(
            (p.perf.decide_all_samples, p.perf.decide_all_p99_us),
            (3, 62)
        );
        assert_eq!(p.perf.spellcheck_p99_us, 40);
        assert_eq!(
            (p.features.settings_opens, p.features.theme_changes),
            (1, 1)
        );
        assert_eq!((p.updates.offered, p.updates.dismissed), (1, 1));
        usage.set_on(false);
    }

    #[test]
    fn paused_time_and_switch_offs() {
        let usage = Usage::new(std::env::temp_dir().join("unflub-usage-pause"), false);
        usage.set_on(true);
        usage.set_paused(true);
        usage.set_paused(true);
        {
            let mut t = usage.tally.lock().unwrap();
            t.paused_since = Some(t.paused_since.unwrap() - 600);
            assert_eq!(t.counters.switched_off, 1, "once per switch");
        }
        usage.set_paused(false);
        let t = usage.tally.lock().unwrap().clone();
        assert!((600..=602).contains(&t.paused_secs), "{}", t.paused_secs);
        assert_eq!(t.paused_since, None);
        let p = build_payload(
            &t,
            &Settings::default(),
            "0.1.0",
            0,
            &env_facts(),
            now_secs(),
        );
        assert_eq!(p.counters.paused_minutes, 10);
        // A pause still going on counts up to the report.
        let since = now_secs() - 120;
        let t = Tally {
            paused_since: Some(since),
            ..Tally::default()
        };
        let p = build_payload(
            &t,
            &Settings::default(),
            "0.1.0",
            0,
            &env_facts(),
            since + 120,
        );
        assert_eq!(p.counters.paused_minutes, 2);
        // Turning stats on while paused starts counting the pause from then.
        usage.set_paused(true);
        usage.set_on(false);
        usage.set_on(true);
        assert!(usage.tally.lock().unwrap().paused_since.is_some());
        usage.set_on(false);
    }

    #[test]
    fn events_go_to_the_api() {
        assert_eq!(events_url(), "https://api.rmgn.net/unflub/v1/events");
    }

    /// One report covers one period: a report that fails to send waits, while the next day is
    /// counted on its own.
    #[test]
    fn a_closed_period_waits_while_the_next_is_counted() {
        let dir = std::env::temp_dir().join(format!("unflub-usage-period-{}", std::process::id()));
        let usage = Usage::new(dir.clone(), true);
        usage.correction(&fix(FixSource::Core, AppCategory::Other));
        usage.tally.lock().unwrap().last_report = Some(1_600_000_000);
        usage.close_period(&Settings::default(), "0.1.0", &env_facts());
        let report = usage.pending().expect("frozen");
        assert_eq!(report["counters"]["corrections"], 1);
        assert_eq!(report["schema"], 3);
        // The send fails (Retry leaves it); meanwhile the next period counts from zero.
        usage.correction(&fix(FixSource::Core, AppCategory::Other));
        usage.correction(&fix(FixSource::Core, AppCategory::Other));
        assert_eq!(usage.pending(), Some(report.clone()));
        assert_eq!(usage.tally.lock().unwrap().counters.corrections, 2);
        assert!(!usage.due(now_secs(), 0), "the new period just started");
        // Survives a restart.
        usage.flush();
        let again = Usage::new(dir.clone(), true);
        assert_eq!(again.pending(), Some(report));
        again.sent();
        assert_eq!(again.pending(), None);
        assert!(again.tally.lock().unwrap().last_report > Some(1_600_000_000));
        assert_eq!(again.tally.lock().unwrap().counters.corrections, 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_rejected_report_is_dropped_but_not_counted_as_sent() {
        let dir = std::env::temp_dir().join(format!("unflub-usage-drop-{}", std::process::id()));
        let usage = Usage::new(dir.clone(), true);
        usage.correction(&fix(FixSource::Core, AppCategory::Other));
        usage.tally.lock().unwrap().last_report = Some(1_600_000_000);
        usage.close_period(&Settings::default(), "0.1.0", &env_facts());
        usage.dropped();
        let t = usage.tally.lock().unwrap().clone();
        assert_eq!(t.pending, None);
        assert_eq!(t.counters, Counters::default());
        assert_eq!(t.last_report, Some(1_600_000_000));
        drop(t);
        assert!(!usage.due(now_secs(), 0));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_paused_period_is_dropped_unsent() {
        let usage = Usage::new(
            std::env::temp_dir().join("unflub-usage-paused-remote"),
            false,
        );
        usage.set_on(true);
        usage.correction(&fix(FixSource::Core, AppCategory::Other));
        usage.drop_period();
        let t = usage.tally.lock().unwrap().clone();
        assert_eq!((t.pending, t.counters), (None, Counters::default()));
        usage.set_on(false);
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
