//! A tiny stand-in for the engine: three fixed rules, phone-style undo. Lets the hook be tried
//! live before the core engine lands (`unflub-cli --demo`).

use std::collections::HashSet;

use unflub_core::{Action, Context, Key, Replacement, Source};
use unflub_windows::Corrector;

const RULES: &[(&str, &str)] = &[("teh", "the"), ("adn", "and"), ("recieve", "receive")];

#[derive(Debug, Default)]
pub struct DemoCorrector {
    word: String,
    /// The correction just made, while Backspace can still undo it.
    last: Option<Replacement>,
    /// Lowercased words the user undid; never corrected again this session.
    undone: HashSet<String>,
}

/// Give `fixed` the capitalisation of `typed`: `Teh` -> `The`, `TEH` -> `THE`.
fn match_case(typed: &str, fixed: &str) -> String {
    let letters = || typed.chars().filter(|c| c.is_alphabetic());
    if letters().count() > 1 && letters().all(char::is_uppercase) {
        return fixed.to_uppercase();
    }
    let mut out = String::with_capacity(fixed.len());
    let mut chars = fixed.chars();
    if typed.chars().next().is_some_and(char::is_uppercase) {
        out.extend(chars.next().map(|c| c.to_ascii_uppercase()));
    }
    out.extend(chars);
    out
}

impl Corrector for DemoCorrector {
    fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        if ctx.secure {
            self.word.clear();
            self.last = None;
            return Action::Pass;
        }
        match key {
            Key::Reset => {
                self.word.clear();
                self.last = None;
                Action::Pass
            }
            Key::Char(c) => {
                self.word.push(c);
                self.last = None;
                Action::Pass
            }
            Key::Backspace => {
                let Some(fix) = self.last.take() else {
                    self.word.pop();
                    return Action::Pass;
                };
                self.undone.insert(fix.original.to_lowercase());
                self.word = fix.original.clone();
                Action::Undo(Replacement {
                    delete: fix.insert.chars().count(),
                    insert: fix.original.clone(),
                    original: fix.original,
                    corrected: fix.corrected,
                    source: fix.source,
                })
            }
            Key::Boundary(b) => {
                let typed = std::mem::take(&mut self.word);
                let lower = typed.to_lowercase();
                let rule = RULES.iter().find(|(from, _)| *from == lower);
                let Some((_, to)) = rule.filter(|_| !self.undone.contains(&lower)) else {
                    self.last = None;
                    return Action::Pass;
                };
                let corrected = match_case(&typed, to);
                let fix = Replacement {
                    delete: typed.chars().count(),
                    insert: format!("{corrected}{b}"),
                    original: typed,
                    corrected,
                    source: Source::UserRule,
                };
                self.last = Some(fix.clone());
                Action::Replace(fix)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(c: &mut DemoCorrector, text: &str) -> Vec<Action> {
        let ctx = Context::default();
        text.chars()
            .map(|ch| {
                let key = match ch {
                    '\u{8}' => Key::Backspace,
                    '|' => Key::Reset,
                    ch if unflub_windows::translate::is_word_char(ch) => Key::Char(ch),
                    ch => Key::Boundary(ch),
                };
                c.on_key(key, &ctx)
            })
            .collect()
    }

    fn replace(delete: usize, insert: &str, original: &str, corrected: &str) -> Action {
        Action::Replace(Replacement {
            delete,
            insert: insert.into(),
            original: original.into(),
            corrected: corrected.into(),
            source: Source::UserRule,
        })
    }

    #[test]
    fn fixes_rules_on_boundary_with_case() {
        let mut c = DemoCorrector::default();
        assert_eq!(
            feed(&mut c, "teh ").pop(),
            Some(replace(3, "the ", "teh", "the"))
        );
        assert_eq!(
            feed(&mut c, "Adn,").pop(),
            Some(replace(3, "And,", "Adn", "And"))
        );
        assert_eq!(
            feed(&mut c, "RECIEVE\n").pop(),
            Some(replace(7, "RECEIVE\n", "RECIEVE", "RECEIVE"))
        );
        assert_eq!(feed(&mut c, "then ").pop(), Some(Action::Pass));
    }

    #[test]
    fn backspace_right_after_a_fix_undoes_it_and_learns() {
        let mut c = DemoCorrector::default();
        feed(&mut c, "teh ");
        let undo = feed(&mut c, "\u{8}").pop().unwrap();
        assert_eq!(
            undo,
            Action::Undo(Replacement {
                delete: 4,
                insert: "teh".into(),
                original: "teh".into(),
                corrected: "the".into(),
                source: Source::UserRule,
            })
        );
        // Undone words stay as typed.
        assert_eq!(feed(&mut c, " teh ").pop(), Some(Action::Pass));
    }

    #[test]
    fn backspace_mid_word_edits_and_reset_forgets() {
        let mut c = DemoCorrector::default();
        assert_eq!(
            feed(&mut c, "tex\u{8}h ").pop(),
            Some(replace(3, "the ", "teh", "the"))
        );
        // A typed key between the fix and Backspace means no undo.
        let mut c = DemoCorrector::default();
        assert!(feed(&mut c, "teh x\u{8}")
            .iter()
            .all(|a| !matches!(a, Action::Undo(_))));
        // Caret moved mid-word: "eh" is not "teh".
        assert_eq!(feed(&mut c, "t|eh ").pop(), Some(Action::Pass));
    }

    #[test]
    fn never_touches_secure_fields() {
        let mut c = DemoCorrector::default();
        let ctx = Context {
            app: None,
            secure: true,
        };
        for k in [
            Key::Char('t'),
            Key::Char('e'),
            Key::Char('h'),
            Key::Boundary(' '),
        ] {
            assert_eq!(c.on_key(k, &ctx), Action::Pass);
        }
    }
}
