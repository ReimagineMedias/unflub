//! Session behaviour: undo, case, resets, policy and modes.

mod common;

use common::Typist;
use unflub_core::{
    Action, Config, Context, Corrector, Engine, EngineMode, Event, Key, Source, SpellBackend,
};

fn config() -> Config {
    Config {
        mode: EngineMode::BundledOnly,
        ..Config::default()
    }
}

#[test]
fn engine_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Engine>();
}

#[test]
fn engine_is_a_corrector() {
    let mut engine = Engine::new(config(), None);
    let corrector: &mut dyn Corrector = &mut engine;
    let ctx = Context::default();
    corrector.on_key(Key::Boundary(' '), &ctx);
    for c in "teh".chars() {
        corrector.on_key(Key::Char(c), &ctx);
    }
    assert!(matches!(
        corrector.on_key(Key::Boundary(' '), &ctx),
        Action::Replace(_)
    ));
}

#[test]
fn replacement_fields() {
    let mut t = Typist::bundled();
    t.type_str("teh");
    let action = t.key(Key::Boundary(' '));
    let Action::Replace(r) = action else {
        panic!("expected a replacement, got {action:?}")
    };
    assert_eq!(r.delete, 3);
    assert_eq!(r.insert, "the ");
    assert_eq!(r.original, "teh");
    assert_eq!(r.corrected, "the");
    assert_eq!(r.source, Source::Bundled);
    assert_eq!(t.engine.drain_events(), vec![Event::Corrected(r)]);
    assert!(t.engine.drain_events().is_empty());
}

#[test]
fn punctuation_boundaries_keep_their_character() {
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("teh. adn, taht!\n"), "the. and, that!\n");
}

#[test]
fn previous_word_context() {
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("if tou can "), "if you can ");
}

#[test]
fn undo_restores_and_learns() {
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("teh "), "the ");
    t.engine.drain_events();
    let action = t.key(Key::Backspace);
    let Action::Undo(r) = action else {
        panic!("expected undo, got {action:?}")
    };
    assert_eq!(r.delete, 4);
    assert_eq!(r.insert, "teh ");
    assert_eq!(t.text, "teh ");
    let events = t.engine.drain_events();
    assert!(events.contains(&Event::Learned("teh".into())));
    assert!(events.iter().any(|e| matches!(e, Event::Undone(_))));
    // Learned: not corrected again, in any case.
    assert_eq!(t.type_str("teh Teh "), "teh teh Teh ");
}

#[test]
fn undo_without_learning() {
    let mut t = Typist::new(Config {
        undo_learns: false,
        ..config()
    });
    t.type_str("teh ");
    assert!(matches!(t.key(Key::Backspace), Action::Undo(_)));
    let events = t.engine.drain_events();
    assert!(!events.iter().any(|e| matches!(e, Event::Learned(_))));
    assert_eq!(t.type_str("teh "), "teh the ");
}

#[test]
fn any_other_key_closes_the_undo_window() {
    let mut t = Typist::bundled();
    t.type_str("teh ");
    t.key(Key::Char('x'));
    assert_eq!(t.key(Key::Backspace), Action::Pass);
    assert_eq!(t.key(Key::Backspace), Action::Pass);
    assert_eq!(t.text, "the");
}

#[test]
fn case_is_preserved() {
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("Teh "), "The ");

    // All caps are acronyms while skipping code-like words...
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("TEH "), "TEH ");
    // ...and are corrected in capitals otherwise. Mixed case is always left alone.
    let mut t = Typist::new(Config {
        skip_code: false,
        ..config()
    });
    assert_eq!(t.type_str("TEH tEh "), "THE tEh ");
}

#[test]
fn reset_forgets_the_word() {
    let mut t = Typist::bundled();
    t.type_str("te");
    t.key(Key::Reset);
    assert_eq!(t.type_str("h "), "teh ");
    // The first word after a reset may continue unseen text, so it is left alone too.
    let mut t = Typist::bundled();
    t.key(Key::Reset);
    assert_eq!(t.type_str("teh teh "), "teh the ");
}

#[test]
fn backspace_edits_the_word() {
    let mut t = Typist::bundled();
    t.type_str("tehh");
    t.key(Key::Backspace);
    assert_eq!(t.type_str(" "), "the ");
    // Deleting past what we saw makes the word unknown.
    let mut t = Typist::bundled();
    t.type_str("a");
    for _ in 0..3 {
        t.key(Key::Backspace);
    }
    assert_eq!(t.type_str("teh "), "teh ");
}

#[test]
fn secure_fields_and_excluded_apps() {
    let mut t = Typist::new(Config {
        excluded_apps: vec!["code.exe".into()],
        ..config()
    });
    let secure = Context {
        app: None,
        secure: true,
    };
    assert_eq!(t.type_in("teh ", &secure), "teh ");
    // Nothing typed in the secure field is remembered.
    t.type_in("te", &secure);
    assert_eq!(t.type_str("h "), "teh teh ");
    t.text.clear();
    let code = Context {
        app: Some("code.exe"),
        secure: false,
    };
    assert_eq!(t.type_in(" teh ", &code), " teh ");
    assert_eq!(t.type_str(" teh "), " teh  the ");
}

#[test]
fn disabled_and_live_config() {
    let mut t = Typist::bundled();
    t.engine.set_config(Config {
        enabled: false,
        ..config()
    });
    assert_eq!(t.type_str("teh "), "teh ");
    // Nothing is tracked while disabled, so the first word after enabling is unseen.
    t.engine.set_config(config());
    assert_eq!(t.type_str("teh teh "), "teh teh the ");
}

#[test]
fn min_word_len() {
    let mut t = Typist::new(Config {
        min_word_len: 4,
        ..config()
    });
    assert_eq!(t.type_str("teh liek "), "teh like ");
}

#[test]
fn user_rules() {
    let mut t = Typist::new(Config {
        user_rules: vec![
            ("omw".into(), "on my way".into()),
            ("u".into(), "you".into()),
        ],
        ..config()
    });
    t.type_str("omw");
    let Action::Replace(r) = t.key(Key::Boundary(' ')) else {
        panic!("rule did not fire")
    };
    assert_eq!(r.source, Source::UserRule);
    // Rules keep the typed case and ignore the minimum word length.
    assert_eq!(t.type_str("Omw u "), "on my way On my way you ");
    // Undoing a rule does not disable it.
    t.engine.drain_events();
    assert!(matches!(t.key(Key::Backspace), Action::Undo(_)));
    assert!(!t
        .engine
        .drain_events()
        .iter()
        .any(|e| matches!(e, Event::Learned(_))));
    assert_eq!(t.type_str("omw "), "on my way On my way u on my way ");
}

#[test]
fn user_rules_ignore_mode() {
    let mut t = Typist::new(Config {
        mode: EngineMode::SystemOnly,
        user_rules: vec![("teh".into(), "the".into())],
        ..Config::default()
    });
    assert_eq!(t.type_str("teh "), "the ");
}

#[test]
fn backtick_spans_are_code() {
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("run `teh adn` teh "), "run `teh adn` the ");
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("```\nteh\n```\nteh "), "```\nteh\n```\nthe ");
}

#[test]
fn glue_characters_do_not_trigger() {
    let mut t = Typist::bundled();
    let text = "teh/adn teh@x.com teh: _teh ";
    assert_eq!(t.type_str(text), text);
}

#[test]
fn leading_quote_stays_put() {
    let mut t = Typist::bundled();
    t.type_str("'teh");
    let Action::Replace(r) = t.key(Key::Boundary(' ')) else {
        panic!("no fix")
    };
    assert_eq!(r.delete, 3);
    assert_eq!(t.text, "'the ");
}

/// A system checker that only knows `known` and offers `fixes`.
struct FakeChecker {
    known: Vec<&'static str>,
    fixes: Vec<(&'static str, &'static str)>,
}

impl SpellBackend for FakeChecker {
    fn is_correct(&self, word: &str) -> bool {
        self.known.iter().any(|k| k.eq_ignore_ascii_case(word))
    }

    fn auto_replacement(&self, word: &str) -> Option<String> {
        self.fixes
            .iter()
            .find(|(from, _)| *from == word)
            .map(|(_, to)| to.to_string())
    }
}

fn with_backend(mode: EngineMode, backend: Option<FakeChecker>) -> Typist {
    let config = Config {
        mode,
        ..Config::default()
    };
    let backend = backend.map(|b| Box::new(b) as Box<dyn SpellBackend>);
    Typist::with_engine(Engine::new(config, backend))
}

#[test]
fn both_mode_needs_the_system_checker_to_agree() {
    let checker = FakeChecker {
        known: vec!["teh"],
        fixes: vec![],
    };
    let mut t = with_backend(EngineMode::Both, Some(checker));
    assert_eq!(t.type_str("teh adn "), "teh and ");
    // Without a system checker, Both falls back to the bundled dictionary.
    let mut t = with_backend(EngineMode::Both, None);
    assert_eq!(t.type_str("teh "), "the ");
}

#[test]
fn system_only_mode() {
    let checker = FakeChecker {
        known: vec![],
        fixes: vec![("adn", "and")],
    };
    let mut t = with_backend(EngineMode::SystemOnly, Some(checker));
    t.type_str("adn");
    let Action::Replace(r) = t.key(Key::Boundary(' ')) else {
        panic!("no system fix")
    };
    assert_eq!(r.source, Source::System);
    // Flagged with no system fix: confident bundled ranking.
    assert_eq!(t.type_str("teh "), "and the ");
    // No system checker: nothing is flagged.
    let mut t = with_backend(EngineMode::SystemOnly, None);
    assert_eq!(t.type_str("teh "), "teh ");
}

#[test]
fn aggressive_mode_takes_low_margin_fixes() {
    let mut bundled = Typist::bundled();
    assert_eq!(bundled.type_str("Trevix "), "Trevix ");
    let mut aggressive = with_backend(EngineMode::Aggressive, None);
    assert_ne!(aggressive.type_str("Trevix "), "Trevix ");
}
