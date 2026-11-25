//! Platform-free autocorrect engine.
//!
//! A platform adapter (Windows, macOS, Linux) turns OS keyboard events into [`Key`]s,
//! calls [`Engine::on_key`] synchronously from its hook, and applies the returned
//! [`Action`] by swallowing the triggering key and injecting text. Everything in this
//! crate must be fast enough to run inside a low-level keyboard hook: the budget for
//! one `on_key` call is 1 ms worst case, typically a few microseconds.
//!
//! Nothing here touches the OS. Adapters supply an optional [`SpellBackend`] (the
//! system spell checker) and a [`Context`] describing the focused text field.

#![forbid(unsafe_code)]

mod case;
pub mod config;
mod dict;
mod distance;
mod layout;
mod rank;
mod session;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use config::{Config, EngineMode, UndoMode};
pub use dict::Dictionary;
pub use session::is_word_char;

use case::Case;
use rank::Ranker;
use session::Session;

/// One keyboard event, already translated by the platform adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character that can be part of a word (letters, digits, apostrophe, hyphen).
    Char(char),
    /// A character that ends a word: space, tab, newline (Enter), or punctuation.
    Boundary(char),
    /// Backspace with no modifiers.
    Backspace,
    /// Ctrl+Backspace: the app deleted back to a word start. Exactly what it deleted varies by
    /// app (a word, its trailing spaces, a run of punctuation), so the text before the caret is
    /// unknown, but the caret now sits at a word start: the next word counts as fully seen.
    DeleteWordLeft,
    /// Anything that moves the caret or makes our view of the text stale: arrows, Home/End,
    /// mouse clicks, focus changes, shortcuts with Ctrl/Alt/Win, IME composition.
    Reset,
}

/// What the adapter knows about the focused field. Cheap to build per key.
#[derive(Debug, Clone, Default)]
pub struct Context<'a> {
    /// Executable or bundle id of the focused app, lowercased (e.g. `"code.exe"`).
    pub app: Option<&'a str>,
    /// True for password fields and other secure input. The engine never corrects or learns there.
    pub secure: bool,
}

/// What the adapter should do with the key it just reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Let the key through untouched.
    Pass,
    /// Swallow the triggering key, send `delete` backspaces, then type `insert`.
    /// `insert` already ends with the boundary character that triggered the correction.
    Replace(Replacement),
    /// Swallow the triggering Backspace and restore what the user originally typed.
    Undo(Replacement),
    /// [`UndoMode::Twice`]: swallow this Backspace and hold it; the fix (`Replacement`) may be
    /// undone by a second Backspace. Before anything else reaches the engine, the adapter
    /// calls [`Corrector::end_undo_window`] and, when it returns true, sends the held
    /// Backspace on (it then comes back as a plain Backspace). Nothing ends the hold by time:
    /// the undo window ([`Corrector::undo_window`]) only bounds when the first Backspace may be
    /// held. On a click or focus change the adapter drops it; on pause or quit it sends it.
    HoldBackspace(Replacement),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// Number of characters to delete before the caret (not counting the swallowed key).
    pub delete: usize,
    /// Text to type after deleting.
    pub insert: String,
    /// The word as the user typed it.
    pub original: String,
    /// The word after correction.
    pub corrected: String,
    /// Why the engine was confident, for the pill and the stats view.
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The user's own rule (abbreviation or learned fix).
    UserRule,
    /// Bundled dictionary ranking.
    Bundled,
    /// The OS spell checker's own auto-replace list.
    System,
}

/// The OS spell checker, implemented by each platform crate.
/// `is_correct` is called synchronously and must be fast (sub-millisecond).
/// `auto_replacement` returns the OS's own "safe to replace" fix if it has one (e.g. Windows'
/// CORRECTIVE_ACTION_REPLACE); return `None` if that lookup would be slow.
pub trait SpellBackend: Send {
    fn is_correct(&self, word: &str) -> bool;
    fn auto_replacement(&self, word: &str) -> Option<String>;
}

/// Words that ended without being checked, for the app's anonymous usage stats. Relaxed adds;
/// the reader takes each with `swap(0)`.
#[derive(Debug, Default)]
pub struct SkipCounts {
    /// Words typed in an app on the user's exclusion list.
    pub excluded_app: AtomicU64,
    /// Words the code-skip rules left alone (backticks, paths, `snake_case`, digits, `camelCase`,
    /// all caps).
    pub code_context: AtomicU64,
}

/// Tells when a key ends a word: a boundary right after word characters. Feed it every key.
#[derive(Debug, Clone, Copy, Default)]
pub struct WordEnds {
    open: bool,
}

impl WordEnds {
    /// True when `key` ends the word typed before it.
    pub fn ends_word(&mut self, key: Key) -> bool {
        match key {
            Key::Char(_) => {
                self.open = true;
                false
            }
            Key::Boundary(c) if is_word_char(c) => {
                self.open = true;
                false
            }
            Key::Boundary(_) => std::mem::take(&mut self.open),
            Key::Backspace => false,
            Key::DeleteWordLeft | Key::Reset => {
                self.open = false;
                false
            }
        }
    }
}

/// Events the UI may want: corrections to show in the pill, undos, learned words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Corrected(Replacement),
    Undone(Replacement),
    Learned(String),
}

/// Anything that turns keys into actions. [`Engine`] implements it; adapters can depend on
/// the trait to test their hook plumbing with a stub.
pub trait Corrector {
    /// Feed one key. Must return within the hook budget.
    fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action;

    /// Close the double-Backspace undo window. True if a Backspace was being held
    /// ([`Action::HoldBackspace`]); the adapter then sends it on, or drops it after a click.
    fn end_undo_window(&mut self) -> bool {
        false
    }

    /// How long a fix stays undoable by a double Backspace, if this corrector holds
    /// Backspaces at all.
    fn undo_window(&self) -> Option<Duration> {
        None
    }

    /// The current reset generation (see [`Engine::caret_at_word_start`]), if the corrector
    /// can use word-start hints.
    fn reset_generation(&self) -> Option<u64> {
        None
    }

    /// The adapter found the caret at a word start for reset `generation`.
    fn caret_at_word_start(&mut self, _generation: u64) {}
}

impl Corrector for Engine {
    fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        Engine::on_key(self, key, ctx)
    }

    fn end_undo_window(&mut self) -> bool {
        Engine::end_undo_window(self)
    }

    fn undo_window(&self) -> Option<Duration> {
        (self.config.undo_mode == UndoMode::Twice)
            .then(|| Duration::from_millis(u64::from(self.config.undo_window_ms)))
    }

    fn reset_generation(&self) -> Option<u64> {
        Some(Engine::reset_generation(self))
    }

    fn caret_at_word_start(&mut self, generation: u64) {
        Engine::caret_at_word_start(self, generation);
    }
}

/// The last fix, while Backspace may still undo it.
#[derive(Debug, Clone)]
struct UndoWindow {
    fix: Replacement,
    /// A first Backspace is being held ([`UndoMode::Twice`]).
    armed: bool,
}

/// The engine. One instance per process, driven from the platform hook thread.
pub struct Engine {
    config: Config,
    rules: Rules,
    backend: Option<Box<dyn SpellBackend>>,
    dict: Arc<Dictionary>,
    ranker: Ranker,
    session: Session,
    /// The last correction, while the next key may still undo it.
    undo: Option<UndoWindow>,
    /// Lowercased words the user undid often enough; never corrected again this session.
    learned: HashSet<String>,
    /// `typo\tfix` (lowercased) to times undone, seeded from `Config::undo_counts`.
    undo_counts: HashMap<String, u32>,
    events: Vec<Event>,
    word: String,
    lower: String,
    /// Words looked at on a boundary, for the app's local stats. One relaxed add per word.
    words_checked: Option<Arc<AtomicU64>>,
    /// Words not checked, by reason, for the anonymous usage stats.
    skips: Option<Arc<SkipCounts>>,
    /// Word ends seen while the app is excluded.
    excluded_words: WordEnds,
    /// A `.` ended the last key while skipping code: the word before it is decided on the next
    /// key, if that ends the token.
    dot_pending: bool,
}

/// A [`Config`] with its lookups (excluded apps, personal words, rules) already built, for
/// [`Engine::set_prepared`]. Build it on any thread.
pub struct PreparedConfig {
    config: Config,
    rules: Rules,
}

impl PreparedConfig {
    pub fn new(config: Config) -> Self {
        Self {
            rules: Rules::new(&config),
            config,
        }
    }
}

/// Lookups derived from [`Config`], lowercased once when the config is set.
struct Rules {
    excluded_apps: HashSet<String>,
    personal: HashSet<String>,
    user: HashMap<String, String>,
}

impl Rules {
    fn new(config: &Config) -> Self {
        Self {
            excluded_apps: config
                .excluded_apps
                .iter()
                .map(|a| a.to_lowercase())
                .collect(),
            personal: config.personal_words.iter().map(|w| normalize(w)).collect(),
            user: config
                .user_rules
                .iter()
                .map(|(from, to)| (normalize(from), to.clone()))
                .collect(),
        }
    }
}

/// Events kept for the UI if nobody drains them.
const MAX_EVENTS: usize = 256;
/// Required score margin (nats) at confidence 1.0.
const MARGIN_AT_FULL_CONFIDENCE: f32 = 2.1;

/// Can a fix that ended with `boundary` be undone? Not after Enter or Tab: the undo would type
/// them again, which sends a chat message twice or commits a spreadsheet cell.
pub fn undoable_boundary(boundary: char) -> bool {
    !matches!(boundary, '\n' | '\r' | '\t')
}

/// The key of a typo-to-fix pair in the undo counts.
fn undo_key(typo: &str, fix: &str) -> String {
    format!("{}\t{}", normalize(typo), normalize(fix))
}

fn normalize(word: &str) -> String {
    word.trim().to_lowercase().replace('\u{2019}', "'")
}

/// Lowercase ASCII letters with apostrophes only inside: the shape the bundled dictionary knows.
fn is_dictionary_shape(w: &str) -> bool {
    let b = w.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b[b.len() - 1].is_ascii_lowercase()
        && b.iter().all(|&c| c.is_ascii_lowercase() || c == b'\'')
}

impl Engine {
    /// Build an engine with the bundled English dictionary and the given config.
    pub fn new(config: Config, backend: Option<Box<dyn SpellBackend>>) -> Self {
        let mut engine = Self {
            rules: Rules::new(&config),
            config,
            backend,
            dict: Dictionary::english(),
            ranker: Ranker::new(layout::Adjacency::new(&layout::QWERTY)),
            session: Session::new(),
            undo: None,
            learned: HashSet::new(),
            undo_counts: HashMap::new(),
            events: Vec::new(),
            word: String::with_capacity(64),
            lower: String::with_capacity(64),
            words_checked: None,
            skips: None,
            excluded_words: WordEnds::default(),
            dot_pending: false,
        };
        engine.seed_undo_counts();
        engine
    }

    /// Add one to `counter` for every word the engine decides on (fixed or not). The reader
    /// takes the count with `swap(0)`.
    pub fn count_words_into(&mut self, counter: Arc<AtomicU64>) {
        self.words_checked = Some(counter);
    }

    /// Add to `counts` for every word left unchecked in an excluded app or by the code-skip
    /// rules.
    pub fn count_skips_into(&mut self, counts: Arc<SkipCounts>) {
        self.skips = Some(counts);
    }

    fn skipped(&self, counter: fn(&SkipCounts) -> &AtomicU64) {
        if let Some(skips) = &self.skips {
            counter(skips).fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Feed one key. Must return within the hook budget.
    pub fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        let undo = match (key, self.undo.take()) {
            (Key::Backspace, undo) => undo,
            (_, Some(w)) if w.armed => {
                // The adapter should have sent the held Backspace on first
                // (`end_undo_window`); if it didn't, count it as delivered.
                self.session.pop();
                None
            }
            _ => None,
        };
        let dot = std::mem::take(&mut self.dot_pending);
        let excluded = ctx
            .app
            .is_some_and(|app| self.rules.excluded_apps.contains(app));
        if !self.config.enabled || ctx.secure || excluded {
            // Keep nothing from secure fields or excluded apps.
            self.session.reset();
            if self.config.enabled && !ctx.secure && self.excluded_words.ends_word(key) {
                self.skipped(|s| &s.excluded_app);
            }
            return Action::Pass;
        }
        self.excluded_words = WordEnds::default();
        match key {
            Key::Reset => self.session.reset(),
            Key::DeleteWordLeft => self.session.reset_at_word_start(),
            Key::Backspace => match (self.config.undo_mode, undo) {
                (UndoMode::Off, _) | (_, None) => self.session.pop(),
                (UndoMode::Once, Some(w)) => return self.undo(w.fix),
                (UndoMode::Twice, Some(w)) if w.armed => return self.undo(w.fix),
                (UndoMode::Twice, Some(w)) => {
                    let fix = w.fix.clone();
                    self.undo = Some(UndoWindow {
                        fix: w.fix,
                        armed: true,
                    });
                    return Action::HoldBackspace(fix);
                }
            },
            Key::Char(c) => self.session.push(c),
            Key::Boundary(c) if session::is_word_char(c) => self.session.push(c),
            Key::Boundary(c) => {
                let skip = self.config.skip_code && session::is_glue(c);
                if !skip {
                    if self.config.skip_code && c == '.' && !dot {
                        // `folake.adeyemi@`, `uottawa.ca`, `postcss.config.js`: whether a word
                        // ends at a `.` is only known at the next key. Decide then, and never if
                        // a letter or digit carries the token on.
                        self.dot_pending = true;
                        self.session.push(c);
                        return Action::Pass;
                    }
                    let fix = if dot {
                        self.session.pop();
                        let fix = self.decide(c, ".");
                        self.session.push('.');
                        fix
                    } else {
                        self.decide(c, "")
                    };
                    if let Some(r) = fix {
                        self.session.pop_n(r.delete);
                        self.session.push_str(&r.insert);
                        self.push_event(Event::Corrected(r.clone()));
                        // Undoing would have to type the boundary again, and Enter or Tab
                        // may already have sent a message or moved to another cell.
                        if undoable_boundary(c) && self.config.undo_mode != UndoMode::Off {
                            self.undo = Some(UndoWindow {
                                fix: r.clone(),
                                armed: false,
                            });
                        }
                        return Action::Replace(r);
                    }
                }
                self.session.push(c);
            }
        }
        Action::Pass
    }

    /// Replace the config at runtime (settings changed).
    pub fn set_config(&mut self, config: Config) {
        drop(self.set_prepared(PreparedConfig::new(config)));
    }

    /// Replace the config with one whose lookups were built elsewhere, and hand back the old
    /// one, so neither building nor freeing them (milliseconds for a large personal dictionary)
    /// happens on the caller's thread. See `unflub_windows::LiveEngine`.
    pub fn set_prepared(&mut self, next: PreparedConfig) -> PreparedConfig {
        let old = PreparedConfig {
            config: std::mem::replace(&mut self.config, next.config),
            rules: std::mem::replace(&mut self.rules, next.rules),
        };
        if self.config.undo_counts_epoch != old.config.undo_counts_epoch {
            // The user cleared their history.
            self.undo_counts.clear();
        }
        self.seed_undo_counts();
        old
    }

    /// Merge `Config::undo_counts` in. Counts only go up: an older snapshot from the app never
    /// lowers what was counted here.
    fn seed_undo_counts(&mut self) {
        for (typo, fix, times) in &self.config.undo_counts {
            let count = self.undo_counts.entry(undo_key(typo, fix)).or_insert(0);
            *count = (*count).max(*times);
        }
    }

    /// Bumped by every reset (a click, focus change, arrow key, or deleting past what was seen).
    pub fn reset_generation(&self) -> u64 {
        self.session.generation
    }

    /// The first word after a reset is normally never corrected: the caret may be in the
    /// middle of a word we never saw. The adapter can find out, off the hook thread, that the
    /// text before the caret ended at a word boundary (empty field, or whitespace before the
    /// caret) when reset `generation` happened. If that is still the current generation and the
    /// first word's boundary hasn't been typed yet, that word now counts as fully observed.
    /// A late, stale or missing answer changes nothing. Returns whether it was taken.
    pub fn caret_at_word_start(&mut self, generation: u64) -> bool {
        generation == self.session.generation && self.session.starts_at_word_start()
    }

    /// Close the double-Backspace undo window ([`UndoMode::Twice`] only; `Once` stays open
    /// until the next key, as before). True if a Backspace was being held: the adapter sends it
    /// on (it comes back as a plain Backspace), or drops it after a click.
    pub fn end_undo_window(&mut self) -> bool {
        if self.config.undo_mode != UndoMode::Twice {
            return false;
        }
        self.undo.take().is_some_and(|w| w.armed)
    }

    /// Times this typo-to-fix pair has been undone, for tests and diagnostics.
    pub fn undo_count(&self, typo: &str, fix: &str) -> u32 {
        self.undo_counts
            .get(&undo_key(typo, fix))
            .copied()
            .unwrap_or(0)
    }

    /// Drain events produced since the last call (for the pill and settings UI).
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn push_event(&mut self, event: Event) {
        if self.events.len() >= MAX_EVENTS {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    fn undo(&mut self, r: Replacement) -> Action {
        let boundary = r.insert.chars().last().unwrap_or(' ');
        if !undoable_boundary(boundary) {
            // Never reached (no undo is offered after Enter or Tab), and never press them.
            self.session.pop();
            return Action::Pass;
        }
        // What was typed after the word (the boundary, and a `.` before it) comes back as is.
        let after = r.insert.chars().skip(r.corrected.chars().count());
        let mut insert = r.original.clone();
        insert.extend(after);
        let undo = Replacement {
            delete: r.insert.chars().count(),
            insert,
            original: r.original,
            corrected: r.corrected,
            source: r.source,
        };
        self.session.pop_n(undo.delete);
        self.session.push_str(&undo.insert);
        let threshold = u32::from(self.config.learn_after_undos.min(5));
        // Only counted while learning is on, and never for a user's own shortcut (they edit it
        // in Settings): a count nothing will use is a typed word kept for nothing.
        if threshold > 0 && undo.source != Source::UserRule {
            let count = self
                .undo_counts
                .entry(undo_key(&undo.original, &undo.corrected))
                .or_insert(0);
            *count += 1;
            if *count >= threshold {
                self.learned.insert(normalize(&undo.original));
                self.push_event(Event::Learned(undo.original.clone()));
            }
        }
        self.push_event(Event::Undone(undo.clone()));
        Action::Undo(undo)
    }

    /// Decides whether the word before the caret should be replaced, given boundary `c`.
    /// `tail` was typed after the word before the boundary (a `.` whose decision waited).
    fn decide(&mut self, boundary: char, tail: &str) -> Option<Replacement> {
        let cur = self.session.current()?;
        if let Some(counter) = &self.words_checked {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        let skip_code = self.config.skip_code;
        if skip_code && (self.session.in_backticks() || cur.code_like) {
            self.skipped(|s| &s.code_context);
            return None;
        }
        if skip_code
            && self
                .session
                .chars(cur.start, cur.end)
                .any(|c| c == '_' || c.is_ascii_digit())
        {
            self.skipped(|s| &s.code_context);
            return None;
        }
        // Leading apostrophes and hyphens (quotes, dashes) stay put; only the word is replaced.
        self.word.clear();
        self.word.extend(
            self.session
                .chars(cur.start, cur.end)
                .skip_while(|&c| !c.is_alphanumeric()),
        );
        let word = self.word.as_str();
        if !word.chars().last().is_some_and(char::is_alphanumeric) {
            return None;
        }
        let case = case::pattern(word);
        if skip_code && matches!(case, Case::Upper | Case::Mixed) {
            self.skipped(|s| &s.code_context);
            return None;
        }
        self.lower.clear();
        self.lower
            .extend(word.chars().flat_map(char::to_lowercase).map(|c| {
                if c == '\u{2019}' {
                    '\''
                } else {
                    c
                }
            }));
        if self.learned.contains(&self.lower) {
            return None;
        }
        if let Some(to) = self.rules.user.get(&self.lower) {
            let to = if to.chars().any(char::is_uppercase) {
                to.clone()
            } else {
                case::apply(case, to)
            };
            return self.replacement(to, tail, boundary, Source::UserRule);
        }
        if word.chars().count() < self.config.min_word_len
            || case == Case::Mixed
            || self.rules.personal.contains(&self.lower)
            || !is_dictionary_shape(&self.lower)
        {
            return None;
        }
        // Possessives (`sister's`, `Collins's`): a known or capitalised base is a real word, and
        // ranking the whole form only finds plurals and bare names (`sisters`, `Collins`).
        // A typo in the base (`sistr's`) is fixed on the base, keeping the `'s` as typed.
        if let Some(base_len) = self.lower.strip_suffix("'s").map(str::len) {
            let base_known = self.dict.id(&self.lower.as_bytes()[..base_len]).is_some();
            if base_len == 0 || base_known || case != Case::Lower {
                return None;
            }
            let cut = word.char_indices().rev().nth(1).map_or(0, |(i, _)| i);
            let suffix = word[cut..].to_string();
            let (source, fixed) = self.rank_possessive_base(&cur, base_len)?;
            let mut fixed = self.cased(source, fixed, case);
            fixed.push_str(&suffix);
            return self.replacement(fixed, tail, boundary, source);
        }
        let (source, fixed) = self.detect_and_rank(&cur)?;
        let fixed = self.cased(source, fixed, case);
        self.replacement(fixed, tail, boundary, source)
    }

    /// A ranked fix in the case the user typed (the OS returns its fixes already cased).
    fn cased(&self, source: Source, fixed: String, case: Case) -> String {
        match source {
            Source::System => fixed,
            _ => {
                let fixed = if fixed == "i" || fixed.starts_with("i'") {
                    case::apply(Case::Title, &fixed)
                } else {
                    fixed
                };
                case::apply(case, &fixed)
            }
        }
    }

    /// Detect and rank the base of a possessive (the word minus its last two characters,
    /// `'s` or `’s`; `base_len` bytes of `self.lower`).
    fn rank_possessive_base(
        &mut self,
        cur: &session::Current,
        base_len: usize,
    ) -> Option<(Source, String)> {
        let word = std::mem::take(&mut self.word);
        let lower = std::mem::take(&mut self.lower);
        let cut = word.char_indices().rev().nth(1).map_or(0, |(i, _)| i);
        self.word.push_str(&word[..cut]);
        self.lower.push_str(&lower[..base_len]);
        let ranked = self.detect_and_rank(cur);
        self.word = word;
        self.lower = lower;
        ranked
    }

    /// Runs detection and candidate ranking for the current word per the engine mode.
    fn detect_and_rank(&mut self, cur: &session::Current) -> Option<(Source, String)> {
        let lower = self.lower.as_bytes();
        let known = self.dict.id(lower).is_some();
        // A capitalised word mid-sentence is most likely a name (`Longbourn`, `Meryton`): no
        // fix. At a sentence start, or right after a reset where nothing was seen before it,
        // capitals say nothing (`Teh` is still fixed).
        let name_like = !cur.sentence_start && case::pattern(&self.word) == Case::Title;
        if name_like {
            return None;
        }
        let keep = Some(0.0);
        let flagged_by_system = |backend: &Option<Box<dyn SpellBackend>>, word: &str| {
            backend.as_ref().map(|b| !b.is_correct(word))
        };
        // The bundled dictionary is English. For another language only the OS checker's own
        // fixes apply, unless the user asked for English fixes too (or chose BundledOnly).
        let bundled = config::is_english(&self.config.language)
            || self.config.english_for_other_languages
            || self.config.mode == EngineMode::BundledOnly;
        if !bundled {
            if flagged_by_system(&self.backend, &self.word) != Some(true) {
                return None;
            }
            let fix = self.backend.as_ref()?.auto_replacement(&self.word)?;
            return (fix != self.word && !fix.is_empty()).then_some((Source::System, fix));
        }
        let (flagged, system_fix, margin_scale, leave_alone) = match self.config.mode {
            EngineMode::BundledOnly => (!known, false, 1.0, keep),
            EngineMode::Both => {
                let flagged = flagged_by_system(&self.backend, &self.word).unwrap_or(!known);
                (flagged, false, 1.0, keep)
            }
            EngineMode::SystemOnly => {
                let flagged = flagged_by_system(&self.backend, &self.word).unwrap_or(false);
                (flagged, true, 1.0, keep)
            }
            EngineMode::Balanced => {
                // Never a word the OS checker calls correct; without one, the bundled
                // dictionary decides.
                let flagged = flagged_by_system(&self.backend, &self.word).unwrap_or(!known);
                (flagged, true, 1.0, keep)
            }
            EngineMode::Aggressive => {
                let flagged =
                    flagged_by_system(&self.backend, &self.word).unwrap_or(false) || !known;
                (flagged, true, 0.0, None)
            }
        };
        if !flagged {
            return None;
        }
        if system_fix {
            if let Some(fix) = self
                .backend
                .as_ref()
                .and_then(|b| b.auto_replacement(&self.word))
            {
                if fix != self.word && !fix.is_empty() {
                    return Some((Source::System, fix));
                }
            }
        }
        let prev = cur.prev.and_then(|(s, e)| {
            let mut buf = [0u8; distance::MAX_LEN];
            let mut n = 0;
            for c in self.session.chars(s, e) {
                let c = if c == '\u{2019}' {
                    '\''
                } else {
                    c.to_ascii_lowercase()
                };
                if n == buf.len() || !c.is_ascii() {
                    return None;
                }
                buf[n] = c as u8;
                n += 1;
            }
            self.dict.id(&buf[..n])
        });
        let choice = self.ranker.best(&self.dict, lower, prev, leave_alone)?;
        let required =
            margin_scale * self.config.confidence.clamp(0.0, 1.0) * MARGIN_AT_FULL_CONFIDENCE;
        (choice.margin > required).then(|| (Source::Bundled, self.dict.word(choice.id).to_string()))
    }

    /// The word is replaced by `corrected`; `tail` and the boundary are typed again after it.
    fn replacement(
        &self,
        corrected: String,
        tail: &str,
        boundary: char,
        source: Source,
    ) -> Option<Replacement> {
        if corrected == self.word {
            return None;
        }
        let mut insert = String::with_capacity(corrected.len() + tail.len() + boundary.len_utf8());
        insert.push_str(&corrected);
        insert.push_str(tail);
        insert.push(boundary);
        Some(Replacement {
            delete: self.word.chars().count() + tail.chars().count(),
            insert,
            original: self.word.clone(),
            corrected,
            source,
        })
    }
}
