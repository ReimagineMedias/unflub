//! Drives an [`Engine`] like a platform adapter would and keeps the resulting text.

#![allow(dead_code)]

use unflub_core::{Action, Config, Context, Engine, EngineMode, Key, Replacement};

pub fn key_for(c: char) -> Key {
    if c.is_alphanumeric() || c == '\'' || c == '-' {
        Key::Char(c)
    } else {
        Key::Boundary(c)
    }
}

pub struct Typist {
    pub engine: Engine,
    pub text: String,
    /// Act like a hook that refuses to type every undo (see [`Action::Undo`]).
    pub refuse_undos: bool,
}

impl Typist {
    pub fn new(config: Config) -> Self {
        Self::with_engine(Engine::new(config, None))
    }

    pub fn with_engine(engine: Engine) -> Self {
        let mut t = Self {
            engine,
            text: String::new(),
            refuse_undos: false,
        };
        // Start the field with a known boundary so the first word counts as fully seen.
        t.key(Key::Boundary('\n'));
        t.text.clear();
        t
    }

    pub fn bundled() -> Self {
        Self::new(Config {
            mode: EngineMode::BundledOnly,
            ..Config::default()
        })
    }

    pub fn key_in(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        let action = self.engine.on_key(key, ctx);
        match &action {
            Action::Pass => match key {
                Key::Char(c) | Key::Boundary(c) => self.text.push(c),
                Key::Backspace => {
                    self.text.pop();
                }
                Key::Reset => {}
                // Like Windows edit controls: spaces left of the caret, then the word.
                Key::DeleteWordLeft => {
                    while self.text.ends_with(char::is_whitespace) {
                        self.text.pop();
                    }
                    while self
                        .text
                        .ends_with(|c: char| c.is_alphanumeric() || c == '\'')
                    {
                        self.text.pop();
                    }
                }
            },
            Action::Replace(r) => self.apply(r),
            Action::Undo(_) if self.refuse_undos => {
                // Like a hook veto (auto-repeat, modifier held, dry run): the Backspace goes
                // through as a plain one and the hook resets the engine.
                self.text.pop();
                self.engine.on_key(Key::Reset, ctx);
            }
            Action::Undo(r) => {
                self.apply(r);
                self.engine.undo_landed(r);
            }
            // Held: nothing reaches the field yet.
            Action::HoldBackspace(_) => {}
        }
        action
    }

    fn apply(&mut self, r: &Replacement) {
        for _ in 0..r.delete {
            self.text.pop();
        }
        self.text.push_str(&r.insert);
    }

    /// What the adapter does before any other key, and on pause or quit: send a
    /// held Backspace on. Returns whether one was held.
    pub fn release(&mut self) -> bool {
        let held = self.engine.end_undo_window();
        if held {
            assert_eq!(
                self.key(Key::Backspace),
                Action::Pass,
                "a plain Backspace now"
            );
        }
        held
    }

    /// A key as the adapter delivers it: a held Backspace goes first, unless this is the
    /// second Backspace.
    pub fn press(&mut self, key: Key) -> Action {
        if key != Key::Backspace {
            self.release();
        }
        self.key(key)
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.key_in(key, &Context::default())
    }

    pub fn type_in(&mut self, s: &str, ctx: &Context<'_>) -> &str {
        for c in s.chars() {
            self.key_in(key_for(c), ctx);
        }
        &self.text
    }

    pub fn type_str(&mut self, s: &str) -> &str {
        self.type_in(s, &Context::default())
    }
}

/// Types `prev word ` (or just `word `) into a fresh bundled-only engine and returns how
/// `word` came out.
pub fn fix(prev: Option<&str>, word: &str) -> String {
    let mut t = Typist::bundled();
    if let Some(p) = prev {
        t.type_str(p);
        t.type_str(" ");
    }
    let start = t.text.len();
    t.type_str(word);
    t.type_str(" ");
    t.text[start..].trim_end_matches(' ').to_string()
}
