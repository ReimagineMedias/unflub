//! The tour's practice box. The settings window is unflub's own, and the keyboard hook leaves
//! unflub's own windows alone (typing your words or app names in Settings shouldn't get them
//! "fixed"), so the tour runs a separate engine on the keys typed in that box instead: the web
//! view sends each key here, applies the answer to the box and draws the pill itself. Nothing is
//! injected, and nothing learned here reaches the real personal dictionary.
//!
//! The box undoes the way unflub does out of the box (#54): the first Backspace after a fix is
//! held and the pill asks "Undo?", as the keyboard hook does, and the second puts the word back.
//! Here the hold only exists in this state: the box hasn't deleted anything, and the next key
//! other than a second Backspace sends it on first ([`TourAction::Release`]).

use serde::Serialize;
use std::sync::{Mutex, MutexGuard};
use unflub_core::{Action, Config, Context, Engine, EngineMode, Key, UndoMode};

#[derive(Default)]
pub struct Tour(Mutex<Option<Practice>>);

/// The practice engine and a first Backspace it holds, like the hook's.
struct Practice {
    engine: Engine,
    held: bool,
}

/// What the box should do with a key, one step of [`Tour::key`]'s answer. Mirrors `TourAction`
/// in `app/src/types.ts`.
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
    /// A first Backspace is held: delete nothing, and ask "Undo?" about this fix.
    Hold { original: String, corrected: String },
    /// The held Backspace goes on after all: delete one character, then apply what follows.
    Release,
}

/// The practice box always demonstrates the same correction and undo gesture: the shipped
/// defaults (Backspace twice, a word learned after two undos), whatever the user set (#90), so
/// what the tour says is what a new install does.
fn engine_for() -> Engine {
    let config = Config {
        mode: EngineMode::BundledOnly,
        min_word_len: 3,
        undo_mode: UndoMode::Twice,
        learn_after_undos: 2,
        ..Config::default()
    };
    let mut engine = Engine::new(config, None);
    // Start as if the box began at a line start, so the first word counts.
    engine.on_key(Key::Boundary('\n'), &Context::default());
    engine
}

fn practice() -> Practice {
    Practice {
        engine: engine_for(),
        held: false,
    }
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
    fn lock_engine(&self) -> MutexGuard<'_, Option<Practice>> {
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
        *self.lock_engine() = Some(practice());
    }

    /// What the box does with `key`, in order: a held Backspace sent on first
    /// ([`TourAction::Release`]) when this key isn't the second Backspace, then this key's own
    /// action. An unknown key changes nothing.
    pub fn key(&self, key: &str) -> Vec<TourAction> {
        let Some(key) = key_of(key) else {
            return vec![TourAction::Pass];
        };
        let mut guard = self.lock_engine();
        let practice = guard.get_or_insert_with(practice);
        let engine = &mut practice.engine;
        let mut out = Vec::with_capacity(2);
        // As the hook does: the held Backspace goes out before any other key reaches the engine.
        if std::mem::take(&mut practice.held) && key != Key::Backspace {
            if engine.end_undo_window() {
                engine.on_key(Key::Backspace, &Context::default());
            }
            out.push(TourAction::Release);
        }
        let action = engine.on_key(key, &Context::default());
        if let Action::Undo(r) = &action {
            // The box applies every undo it is given.
            engine.undo_landed(r);
        }
        // Learned words stay in this engine; the real dictionary never hears about them.
        engine.drain_events();
        out.push(match action {
            Action::Pass => TourAction::Pass,
            Action::HoldBackspace(r) => {
                practice.held = true;
                TourAction::Hold {
                    original: r.original,
                    corrected: r.corrected,
                }
            }
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
        });
        out
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
            for action in tour.key(&key) {
                apply(&action, c, out);
                actions.push(action);
            }
        }
        actions
    }

    /// What `app/src/Tour.tsx` does with one step for the key `c`.
    fn apply(action: &TourAction, c: char, out: &mut String) {
        match action {
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
            TourAction::Hold { .. } => {}
            TourAction::Release => {
                out.pop();
            }
        }
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
        // Backspace twice right after the fix puts the word back.
        type_into(&tour, "\u{8}\u{8}", &mut text);
        assert_eq!(text, "teh ");
        // One undo doesn't teach it yet; the second does.
        type_into(&tour, "teh \u{8}\u{8}", &mut text);
        assert_eq!(text, "teh teh ");
        type_into(&tour, "teh ", &mut text);
        assert_eq!(text, "teh teh teh ");
    }

    /// #54: the box works like a new install. The first Backspace asks, the second undoes, and
    /// nothing else is deleted.
    #[test]
    fn the_undo_takes_two_backspaces() {
        let tour = Tour::default();
        tour.reset();
        let mut text = String::new();
        type_into(&tour, "adn ", &mut text);
        assert_eq!(text, "and ");
        let first = type_into(&tour, "\u{8}", &mut text);
        assert_eq!(
            first,
            [TourAction::Hold {
                original: "adn".into(),
                corrected: "and".into()
            }]
        );
        assert_eq!(text, "and ", "nothing deleted yet");
        let second = type_into(&tour, "\u{8}", &mut text);
        assert!(matches!(second.as_slice(), [TourAction::Undo { .. }]));
        assert_eq!(text, "adn ");
    }

    /// #54: every other key sends the held Backspace on first, so one Backspace and a comma
    /// still makes "and,", as in any app.
    #[test]
    fn a_held_backspace_goes_on_before_the_next_key() {
        for (keys, want) in [(",", "and,"), ("x", "andx"), (" ", "and "), ("\n", "and\n")] {
            let tour = Tour::default();
            tour.reset();
            let mut text = String::new();
            type_into(&tour, "adn \u{8}", &mut text);
            let actions = type_into(&tour, keys, &mut text);
            assert_eq!(actions.first(), Some(&TourAction::Release), "{keys:?}");
            assert_eq!(text, want, "{keys:?}");
            // Nothing is held any more: Backspace deletes.
            type_into(&tour, "\u{8}", &mut text);
            assert_eq!(text, &want[..want.len() - 1], "{keys:?}");
        }
        // Reset releases it for shortcuts, navigation and repeats. The browser applies the
        // repeat itself, so it can delete without sending a second Backspace to the engine.
        let tour = Tour::default();
        tour.reset();
        let mut text = String::new();
        type_into(&tour, "adn \u{8}", &mut text);
        assert_eq!(tour.key("Reset"), [TourAction::Release, TourAction::Pass]);
        assert_eq!(tour.key("Backspace"), [TourAction::Pass]);
        // A key the box ignores changes nothing, the hold included.
        let tour = Tour::default();
        tour.reset();
        let mut text = String::new();
        type_into(&tour, "adn \u{8}", &mut text);
        assert_eq!(tour.key("F5"), [TourAction::Pass]);
        type_into(&tour, "\u{8}", &mut text);
        assert_eq!(text, "adn ");
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
        type_into(&tour, "teh \u{8}\u{8}teh \u{8}\u{8}", &mut text);
        assert_eq!(text, "teh teh ");
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
