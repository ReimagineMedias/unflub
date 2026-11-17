//! The tour's practice box. The settings window is unflub's own, and the keyboard hook leaves
//! unflub's own windows alone (typing your words or app names in Settings shouldn't get them
//! "fixed"), so the tour runs a separate engine on the keys typed in that box instead: the web
//! view sends each key here, applies the answer to the box and draws the pill itself. Nothing is
//! injected, and nothing learned here reaches the real personal dictionary.

use crate::settings::{EngineMode, Settings};
use serde::Serialize;
use std::sync::Mutex;
use unflub_core::{Action, Context, Engine, Key};

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

/// The practice engine: the user's settings, minus what needs the OS. With no OS spell checker
/// here, "system only" uses unflub's own dictionary.
fn engine_for(settings: &Settings) -> Engine {
    let mut config = settings.to_core();
    if settings.mode == EngineMode::SystemOnly {
        config.mode = unflub_core::EngineMode::BundledOnly;
    }
    config.enabled = true;
    config.excluded_apps.clear();
    // The practice box teaches the plain gesture and remembers at once; it has no pill timer
    // to hold a Backspace with.
    config.undo_mode = unflub_core::UndoMode::Once;
    config.learn_after_undos = 1;
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
    /// A fresh practice box.
    pub fn reset(&self, settings: &Settings) {
        *self.0.lock().unwrap() = Some(engine_for(settings));
    }

    pub fn key(&self, settings: &Settings, key: &str) -> TourAction {
        let Some(key) = key_of(key) else {
            return TourAction::Pass;
        };
        let mut guard = self.0.lock().unwrap();
        let engine = guard.get_or_insert_with(|| engine_for(settings));
        let action = engine.on_key(key, &Context::default());
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

    fn type_into(
        tour: &Tour,
        settings: &Settings,
        text: &str,
        out: &mut String,
    ) -> Vec<TourAction> {
        let mut actions = Vec::new();
        for c in text.chars() {
            let key = if c == '\u{8}' {
                "Backspace".to_string()
            } else {
                c.to_string()
            };
            let action = tour.key(settings, &key);
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
        let s = Settings::default();
        tour.reset(&s);
        let mut text = String::new();
        let actions = type_into(&tour, &s, "teh ", &mut text);
        assert_eq!(text, "the ");
        assert!(
            matches!(actions.last(), Some(TourAction::Replace { corrected, .. }) if corrected == "the")
        );
        // Backspace right after the fix puts the word back.
        type_into(&tour, &s, "\u{8}", &mut text);
        assert_eq!(text, "teh ");
        // And it is remembered: the next "teh" stays.
        type_into(&tour, &s, "teh ", &mut text);
        assert_eq!(text, "teh teh ");
    }

    #[test]
    fn works_in_every_mode_without_the_os_checker() {
        for mode in [
            EngineMode::Both,
            EngineMode::SystemOnly,
            EngineMode::BundledOnly,
            EngineMode::Balanced,
            EngineMode::Aggressive,
        ] {
            let s = Settings {
                mode,
                ..Settings::default()
            };
            let tour = Tour::default();
            tour.reset(&s);
            let mut text = String::new();
            type_into(&tour, &s, "teh ", &mut text);
            assert_eq!(text, "the ", "{mode:?}");
        }
    }

    #[test]
    fn learning_stays_in_the_box() {
        let tour = Tour::default();
        let s = Settings::default();
        tour.reset(&s);
        let mut text = String::new();
        type_into(&tour, &s, "teh \u{8}", &mut text);
        tour.reset(&s);
        let mut again = String::new();
        type_into(&tour, &s, "teh ", &mut again);
        assert_eq!(again, "the ", "a fresh box forgets");
    }

    #[test]
    fn odd_keys_pass() {
        assert_eq!(key_of("ArrowLeft"), None);
        assert_eq!(key_of(""), None);
        assert_eq!(key_of("a"), Some(Key::Char('a')));
        assert_eq!(key_of(" "), Some(Key::Boundary(' ')));
    }
}
