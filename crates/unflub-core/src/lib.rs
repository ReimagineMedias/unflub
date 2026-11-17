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
use std::sync::Arc;

pub use config::{Config, EngineMode};
pub use dict::Dictionary;

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
    /// Add a word so the OS checker stops flagging it (best effort).
    fn learn(&self, _word: &str) {}
}

/// Events the UI may want: corrections to show in the pill, undos, learned words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Corrected(Replacement),
    Undone(Replacement),
    Learned(String),
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
    undo: Option<Replacement>,
    /// Lowercased words the user undid; never corrected again this session.
    learned: HashSet<String>,
    events: Vec<Event>,
    word: String,
    lower: String,
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
/// How much likelier (nats) a capitalised word mid-sentence is to be a deliberate name.
const NAME_BONUS: f32 = 3.0;

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
        Self {
            rules: Rules::new(&config),
            config,
            backend,
            dict: Dictionary::english(),
            ranker: Ranker::new(layout::Adjacency::new(&layout::QWERTY)),
            session: Session::new(),
            undo: None,
            learned: HashSet::new(),
            events: Vec::new(),
            word: String::with_capacity(64),
            lower: String::with_capacity(64),
        }
    }

    /// Feed one key. Must return within the hook budget.
    pub fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        let undo = self.undo.take();
        let excluded = ctx
            .app
            .is_some_and(|app| self.rules.excluded_apps.contains(app));
        if !self.config.enabled || ctx.secure || excluded {
            // Keep nothing from secure fields or excluded apps.
            self.session.reset();
            return Action::Pass;
        }
        match key {
            Key::Reset => self.session.reset(),
            Key::Backspace => match undo {
                Some(r) => return self.undo(r),
                None => self.session.pop(),
            },
            Key::Char(c) => self.session.push(c),
            Key::Boundary(c) if session::is_word_char(c) => self.session.push(c),
            Key::Boundary(c) => {
                let skip = self.config.skip_code && session::is_glue(c);
                if !skip {
                    if let Some(r) = self.decide(c) {
                        self.session.pop_n(r.delete);
                        self.session.push_str(&r.insert);
                        self.push_event(Event::Corrected(r.clone()));
                        self.undo = Some(r.clone());
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
        self.rules = Rules::new(&config);
        self.config = config;
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
        let mut insert = r.original.clone();
        insert.push(boundary);
        let undo = Replacement {
            delete: r.corrected.chars().count() + 1,
            insert,
            original: r.original,
            corrected: r.corrected,
            source: r.source,
        };
        self.session.pop_n(undo.delete);
        self.session.push_str(&undo.insert);
        if self.config.undo_learns && undo.source != Source::UserRule {
            self.learned.insert(normalize(&undo.original));
            self.push_event(Event::Learned(undo.original.clone()));
        }
        self.push_event(Event::Undone(undo.clone()));
        Action::Undo(undo)
    }

    /// Decides whether the word before the caret should be replaced, given boundary `c`.
    fn decide(&mut self, boundary: char) -> Option<Replacement> {
        let cur = self.session.current()?;
        let skip_code = self.config.skip_code;
        if skip_code && (self.session.backticks || cur.code_like) {
            return None;
        }
        if skip_code
            && self
                .session
                .chars(cur.start, cur.end)
                .any(|c| c == '_' || c.is_ascii_digit())
        {
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
            return self.replacement(to, boundary, Source::UserRule);
        }
        if word.chars().count() < self.config.min_word_len
            || case == Case::Mixed
            || self.rules.personal.contains(&self.lower)
            || !is_dictionary_shape(&self.lower)
        {
            return None;
        }
        let (source, fixed) = self.detect_and_rank(&cur)?;
        let fixed = match source {
            // The OS returns the replacement already cased.
            Source::System => fixed,
            _ => {
                let fixed = if fixed == "i" || fixed.starts_with("i'") {
                    case::apply(Case::Title, &fixed)
                } else {
                    fixed
                };
                case::apply(case, &fixed)
            }
        };
        self.replacement(fixed, boundary, source)
    }

    /// Runs detection and candidate ranking for the current word per the engine mode.
    fn detect_and_rank(&mut self, cur: &session::Current) -> Option<(Source, String)> {
        let lower = self.lower.as_bytes();
        let known = self.dict.id(lower).is_some();
        let name_like = !cur.sentence_start && case::pattern(&self.word) == Case::Title;
        let keep = Some(if name_like { NAME_BONUS } else { 0.0 });
        let flagged_by_system = |backend: &Option<Box<dyn SpellBackend>>, word: &str| {
            backend.as_ref().map(|b| !b.is_correct(word))
        };
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

    fn replacement(
        &self,
        corrected: String,
        boundary: char,
        source: Source,
    ) -> Option<Replacement> {
        if corrected == self.word {
            return None;
        }
        let mut insert = String::with_capacity(corrected.len() + boundary.len_utf8());
        insert.push_str(&corrected);
        insert.push(boundary);
        Some(Replacement {
            delete: self.word.chars().count(),
            insert,
            original: self.word.clone(),
            corrected,
            source,
        })
    }
}
