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

pub mod config;

pub use config::{Config, EngineMode};

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
    _private: (),
}

impl Engine {
    /// Build an engine with the bundled English dictionary and the given config.
    pub fn new(_config: Config, _backend: Option<Box<dyn SpellBackend>>) -> Self {
        todo!("implemented in the core engine PR")
    }

    /// Feed one key. Must return within the hook budget.
    pub fn on_key(&mut self, _key: Key, _ctx: &Context<'_>) -> Action {
        todo!("implemented in the core engine PR")
    }

    /// Replace the config at runtime (settings changed).
    pub fn set_config(&mut self, _config: Config) {
        todo!("implemented in the core engine PR")
    }

    /// Drain events produced since the last call (for the pill and settings UI).
    pub fn drain_events(&mut self) -> Vec<Event> {
        todo!("implemented in the core engine PR")
    }
}
