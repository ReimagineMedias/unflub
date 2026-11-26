//! The tour's practice box. The settings window is unflub's own, and the keyboard hook leaves
//! unflub's own windows alone (typing your words or app names in Settings shouldn't get them
//! "fixed"), so the tour runs a separate engine on the keys typed in that box instead: the web
//! view sends each key here, applies the answer to the box and draws the pill itself. Nothing is
//! injected, and nothing learned here reaches the real personal dictionary.

use serde::Serialize;
use std::sync::{Mutex, MutexGuard};
use unflub_core::{Action, Config, Context, Engine, EngineMode, Key, UndoMode};

#[derive(Default)]
pub struct Tour(Mutex<Option<Engine>>);

/// What the box should do with a key. Mirrors `TourAction` in `app/src/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TourAction {
    /// Type the key as usual.
    Pass,
    /// Delete `delete` characters before the caret, then type `insert` (the key included).
    Replace {
        delete: usize,
        insert: String,
        original: String,
        corrected: String,
    },
    /// The same, putting the user's word back.
    Undo {
        delete: usize,
        insert: String,
        original: String,
        corrected: String,
    },
}

/// The practice box always demonstrates the same correction and undo gesture.
fn engine_for() -> Engine {
    let config = Config {
        mode: EngineMode::BundledOnly,
        min_word_len: 3,
        undo_mode: UndoMode::Once,
        learn_after_undos: 1,
        ..Config::default()
    };
    let mut engine = Engine::new(config, None);
    // Start as if the box began at a line start, so the first word counts.
    engine.on_key(Key::Boundary('\n'), &Context::default());
    engine
}

/// `"Backspace"`, `"Reset"`, or one typed character.
fn key_of(key: &str) -> Option<Key> {
    match key {
        "Backspace" => Some(Key::Backspace),
        "Reset" => Some(Key::Reset),
        _ => {
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Some(if c.is_alphanumeric() || c == '\'' || c == '-' {
                Key::Char(c)
            } else {
                Key::Boundary(c)
            })
        }
    }
}

impl Tour {
    fn lock_engine(&self) -> MutexGuard<'_, Option<Engine>> {
        self.0.lock().unwrap_or_else(|poisoned| {
            let mut engine = poisoned.into_inner();
            // Discard any partially applied key and clear the poison so later keys can proceed.
            *engine = None;
            self.0.clear_poison();
            engine
        })
    }

    /// A fresh practice box.
    pub fn reset(&self) {
        *self.lock_engine() = Some(engine_for());
    }

    pub fn key(&self, key: &str) -> TourAction {
        let Some(key) = key_of(key) else {
            return TourAction::Pass;
        };
        let mut guard = self.lock_engine();
        let engine = guard.get_or_insert_with(engine_for);
        let action = engine.on_key(key, &Context::default());
        if let Action::Undo(r) = &action {
            // The box applies every undo it is given.
            engine.undo_landed(r);
        }
        // Learned words stay in this engine; the real dictionary never hears about them.
        engine.drain_events();
        match action {
            // Not in Once mode, which the box uses.
            Action::Pass | Action::HoldBackspace(_) => TourAction::Pass,
            Action::Replace(r) => TourAction::Replace {
                delete: r.delete,
                insert: r.insert,
                original: r.original,
                corrected: r.corrected,
            },
            Action::Undo(r) => TourAction::Undo {
                delete: r.delete,
                insert: r.insert,
                original: r.original,
                corrected: r.corrected,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_into(tour: &Tour, text: &str, out: &mut String) -> Vec<TourAction> {
        let mut actions = Vec::new();
        for c in text.chars() {
            let key = if c == '\u{8}' {
                "Backspace".to_string()
            } else {
                c.to_string()
            };
            let action = tour.key(&key);
            match &action {
                TourAction::Pass if c == '\u{8}' => {
                    out.pop();
                }
                TourAction::Pass => out.push(c),
                TourAction::Replace { delete, insert, .. }
                | TourAction::Undo { delete, insert, .. } => {
                    for _ in 0..*delete {
                        out.pop();
                    }
                    out.push_str(insert);
                }
            }
            actions.push(action);
        }
        actions
    }

    #[test]
    fn fixes_undoes_and_remembers() {
        let tour = Tour::default();
        tour.reset();
        let mut text = String::new();
        let actions = type_into(&tour, "teh ", &mut text);
        assert_eq!(text, "the ");
        assert!(
            matches!(actions.last(), Some(TourAction::Replace { corrected, .. }) if corrected == "the")
        );
        // Backspace right after the fix puts the word back.
        type_into(&tour, "\u{8}", &mut text);
        assert_eq!(text, "teh ");
        // And it is remembered: the next "teh" stays.
        type_into(&tour, "teh ", &mut text);
        assert_eq!(text, "teh teh ");
    }

    #[test]
    fn a_new_box_fixes_teh_even_before_reset() {
        let tour = Tour::default();
        let mut text = String::new();
        type_into(&tour, "teh ", &mut text);
        assert_eq!(text, "the ");
    }

    #[test]
    fn learning_stays_in_the_box() {
        let tour = Tour::default();
        tour.reset();
        let mut text = String::new();
        type_into(&tour, "teh \u{8}", &mut text);
        tour.reset();
        let mut again = String::new();
        type_into(&tour, "teh ", &mut again);
        assert_eq!(again, "the ", "a fresh box forgets");
    }

    #[test]
    fn a_poisoned_practice_engine_is_rebuilt() {
        let tour = Tour::default();
        tour.key("x");
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = tour.0.lock().unwrap();
            panic!("simulated practice failure");
        }));
        assert!(failed.is_err());
        assert!(tour.0.is_poisoned());

        let mut text = String::new();
        type_into(&tour, "teh ", &mut text);
        assert_eq!(text, "the ");
        assert!(!tour.0.is_poisoned());
    }

    #[test]
    fn odd_keys_pass() {
        assert_eq!(key_of("ArrowLeft"), None);
        assert_eq!(key_of(""), None);
        assert_eq!(key_of("a"), Some(Key::Char('a')));
        assert_eq!(key_of(" "), Some(Key::Boundary(' ')));
    }
}
