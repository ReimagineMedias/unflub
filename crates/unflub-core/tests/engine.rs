//! Session behaviour: undo, case, resets, policy and modes.

mod common;

use common::Typist;
use unflub_core::{
    Action, Config, Context, Corrector, Engine, EngineMode, Event, Key, Source, SpellBackend,
    UndoMode,
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

/// Balanced flags what either engine flags, like Aggressive, but keeps Both's confidence margin
/// and its "might be a name" doubt.
#[test]
fn balanced_mode() {
    let mut t = Typist::new(Config {
        mode: EngineMode::Balanced,
        ..Config::default()
    });
    assert_eq!(t.type_str("teh "), "the ");
    let typed = |mode| {
        Typist::new(Config {
            mode,
            ..Config::default()
        })
        .type_str("I met Jhon today ")
        .to_string()
    };
    let (both, balanced) = (typed(EngineMode::Both), typed(EngineMode::Balanced));
    assert_eq!(
        balanced, both,
        "a capitalised word mid-sentence is left alone"
    );
}

#[test]
fn counts_words_checked() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let counter = Arc::new(AtomicU64::new(0));
    let mut engine = Engine::new(config(), None);
    engine.count_words_into(counter.clone());
    let mut t = Typist::with_engine(engine);
    assert_eq!(t.type_str("teh cat sat. "), "the cat sat. ");
    // Three words reached a boundary; the trailing space after "." has no word.
    assert_eq!(counter.swap(0, Ordering::Relaxed), 3);
    t.type_str("ok ");
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

#[test]
fn counts_words_skipped_in_excluded_apps_and_code() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use unflub_core::SkipCounts;
    let skips = Arc::new(SkipCounts::default());
    let mut engine = Engine::new(
        Config {
            excluded_apps: vec!["keepass.exe".into()],
            ..config()
        },
        None,
    );
    engine.count_skips_into(skips.clone());
    let mut t = Typist::with_engine(engine);
    let excluded = Context {
        app: Some("keepass.exe"),
        secure: false,
    };
    // Two words end; the space after the comma ends none.
    t.type_in("teh cat, ", &excluded);
    assert_eq!(skips.excluded_app.swap(0, Ordering::Relaxed), 2);
    // Secure fields are counted by the adapter, not here.
    t.type_in(
        "teh ",
        &Context {
            app: Some("keepass.exe"),
            secure: true,
        },
    );
    assert_eq!(skips.excluded_app.load(Ordering::Relaxed), 0);

    // After the reset above, a boundary first so the first word counts as fully seen.
    t.type_str("\nmy_var useState NASA src2 fine ");
    assert_eq!(skips.code_context.load(Ordering::Relaxed), 4);
    assert_eq!(skips.excluded_app.load(Ordering::Relaxed), 0);
}

#[test]
fn word_ends_need_a_word_before_the_boundary() {
    use unflub_core::WordEnds;
    let mut w = WordEnds::default();
    let ends: Vec<bool> = "it's ok, fine."
        .chars()
        .map(|c| w.ends_word(common::key_for(c)))
        .collect();
    assert_eq!(ends.iter().filter(|e| **e).count(), 3, "{ends:?}");
    let mut w = WordEnds::default();
    w.ends_word(Key::Char('a'));
    w.ends_word(Key::Reset);
    assert!(!w.ends_word(Key::Boundary(' ')), "a reset forgets the word");
    w.ends_word(Key::Char('a'));
    w.ends_word(Key::Backspace);
    assert!(w.ends_word(Key::Boundary(' ')));
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

fn mode(undo_mode: UndoMode, learn_after_undos: u8) -> Typist {
    Typist::new(Config {
        undo_mode,
        learn_after_undos,
        ..config()
    })
}

fn learned(t: &mut Typist) -> Vec<String> {
    t.engine
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Learned(w) => Some(w),
            _ => None,
        })
        .collect()
}

/// Twice: the first Backspace is held, the second undoes.
#[test]
fn double_backspace_undoes() {
    let mut t = mode(UndoMode::Twice, 2);
    assert_eq!(t.type_str("teh "), "the ");
    let Action::HoldBackspace(fix) = t.press(Key::Backspace) else {
        panic!("first Backspace is held")
    };
    assert_eq!(
        (fix.original.as_str(), fix.corrected.as_str()),
        ("teh", "the")
    );
    assert_eq!(t.text, "the ", "nothing deleted yet");
    let action = t.press(Key::Backspace);
    let Action::Undo(r) = action else {
        panic!("second Backspace undoes, got {action:?}")
    };
    assert_eq!((r.delete, r.insert.as_str()), (4, "teh "));
    assert_eq!(t.text, "teh ");
    assert!(
        learned(&mut t).is_empty(),
        "one undo doesn't teach (threshold 2)"
    );
    // The window is spent: the next Backspace deletes.
    assert_eq!(t.press(Key::Backspace), Action::Pass);
    assert_eq!(t.text, "teh");
}

/// Twice: one Backspace then a comma deletes the space first (the reason for the redesign).
#[test]
fn single_backspace_then_comma_keeps_the_fix() {
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("ok teh ");
    assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
    assert_eq!(t.press(Key::Boundary(',')), Action::Pass);
    assert_eq!(t.text, "ok the,");
    assert!(learned(&mut t).is_empty());
    assert_eq!(t.engine.undo_count("teh", "the"), 0);
}

/// Twice: a held Backspace waits as long as it takes (no timeout in the engine), and is sent
/// when the adapter lets it go (on pause or quit).
#[test]
fn a_held_backspace_waits_until_answered() {
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("teh ");
    assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
    // Still held, however long nothing happens: the second Backspace still undoes.
    let mut later = mode(UndoMode::Twice, 2);
    later.type_str("teh ");
    later.press(Key::Backspace);
    assert!(matches!(later.press(Key::Backspace), Action::Undo(_)));
    assert!(t.release(), "pause or quit sends it on");
    assert_eq!(t.text, "the");
    // Typing goes on normally.
    assert_eq!(t.type_str("re "), "there ");
}

/// Twice: navigation (Reset) releases the held Backspace before it acts.
#[test]
fn single_backspace_then_arrow_deletes_first() {
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("teh ");
    assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
    assert_eq!(t.press(Key::Reset), Action::Pass);
    assert_eq!(t.text, "the");
    // A click instead drops it: the adapter closes the window and doesn't send it.
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("teh ");
    assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
    assert!(t.engine.end_undo_window());
    assert_eq!(t.key(Key::Reset), Action::Pass);
    assert_eq!(t.text, "the ");
}

/// Twice: no window after it closed (pill gone); a Backspace is plain again.
#[test]
fn backspace_after_the_window_is_plain() {
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("teh ");
    assert!(!t.engine.end_undo_window(), "nothing held");
    assert_eq!(t.press(Key::Backspace), Action::Pass);
    assert_eq!(t.text, "the");
    // Once and Off don't hold anything, so the timer never closes them.
    assert!(!mode(UndoMode::Once, 2).engine.end_undo_window());
}

/// Once: the old single-Backspace undo, without instant learning.
#[test]
fn once_mode_undoes_on_the_first_backspace() {
    let mut t = mode(UndoMode::Once, 2);
    t.type_str("teh ");
    assert!(matches!(t.press(Key::Backspace), Action::Undo(_)));
    assert_eq!(t.text, "teh ");
    assert!(learned(&mut t).is_empty());
    assert_eq!(t.type_str("teh "), "teh the ", "still fixed after one undo");
}

/// Off: Backspace is always Backspace.
#[test]
fn off_mode_never_undoes() {
    let mut t = mode(UndoMode::Off, 2);
    t.type_str("teh ");
    assert_eq!(t.press(Key::Backspace), Action::Pass);
    assert_eq!(t.press(Key::Backspace), Action::Pass);
    assert_eq!(t.text, "th");
}

/// A word stops being fixed only after `learn_after_undos` undos of the same typo and fix.
#[test]
fn learning_needs_repeated_undos() {
    let undo_twice = |t: &mut Typist| {
        assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
        assert!(matches!(t.press(Key::Backspace), Action::Undo(_)));
    };
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("teh ");
    undo_twice(&mut t);
    assert!(learned(&mut t).is_empty());
    assert_eq!(t.engine.undo_count("teh", "the"), 1);
    t.type_str("teh ");
    assert!(t.text.ends_with("the "), "fixed again after one undo");
    undo_twice(&mut t);
    assert_eq!(learned(&mut t), ["teh"]);
    assert!(t.type_str("teh ").ends_with("teh teh "), "learned");

    // 1: the first undo teaches. 0: never.
    let mut t = mode(UndoMode::Once, 1);
    t.type_str("teh ");
    t.press(Key::Backspace);
    assert_eq!(learned(&mut t), ["teh"]);
    let mut t = mode(UndoMode::Once, 0);
    for _ in 0..6 {
        t.type_str("teh ");
        t.press(Key::Backspace);
        t.type_str("x ");
    }
    assert!(learned(&mut t).is_empty());
    assert_eq!(
        t.engine.undo_count("teh", "the"),
        0,
        "nothing is counted while learning is off"
    );
}

/// Clearing history (a new epoch from the app) empties the engine's counts.
#[test]
fn a_new_undo_epoch_forgets_the_counts() {
    let mut t = Typist::new(Config {
        undo_mode: UndoMode::Once,
        learn_after_undos: 5,
        ..config()
    });
    t.type_str("teh ");
    t.press(Key::Backspace);
    assert_eq!(t.engine.undo_count("teh", "the"), 1);
    t.engine.set_config(Config {
        undo_mode: UndoMode::Once,
        learn_after_undos: 5,
        ..config()
    });
    assert_eq!(t.engine.undo_count("teh", "the"), 1, "same epoch: kept");
    t.engine.set_config(Config {
        undo_mode: UndoMode::Once,
        learn_after_undos: 5,
        undo_counts_epoch: 1,
        ..config()
    });
    assert_eq!(t.engine.undo_count("teh", "the"), 0);
}

/// Counts come back from the app between runs, and an older snapshot never lowers them.
#[test]
fn undo_counts_are_seeded_and_only_grow() {
    let mut t = Typist::new(Config {
        undo_mode: UndoMode::Once,
        undo_counts: vec![("Teh".into(), "the".into(), 1)],
        ..config()
    });
    assert_eq!(t.engine.undo_count("teh", "the"), 1);
    t.type_str("teh ");
    t.press(Key::Backspace);
    assert_eq!(
        learned(&mut t),
        ["teh"],
        "1 seeded + 1 = the default threshold of 2"
    );
    t.engine.set_config(Config {
        undo_counts: vec![("teh".into(), "the".into(), 1)],
        ..config()
    });
    assert_eq!(t.engine.undo_count("teh", "the"), 2);
}

/// The double-Backspace window never opens after Enter or Tab either (review B2).
#[test]
fn no_held_backspace_after_enter() {
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("ok teh\n");
    assert_eq!(t.press(Key::Backspace), Action::Pass);
    assert_eq!(t.text, "ok the");
}

/// Review blocker B2: Enter may already have sent a chat message or moved to another cell, so
/// a fix ended by Enter or Tab can't be undone, and an undo never types Enter or Tab.
#[test]
fn no_undo_after_enter_or_tab() {
    for boundary in ['\n', '\t'] {
        let mut t = Typist::bundled();
        let typed = t.type_str(&format!("ok teh{boundary}")).to_string();
        assert_eq!(typed, format!("ok the{boundary}"));
        t.engine.drain_events();
        // Backspace is just a Backspace: it deletes the boundary, nothing is retyped.
        assert_eq!(t.key(Key::Backspace), Action::Pass, "{boundary:?}");
        assert_eq!(t.text, "ok the");
        assert!(t.engine.drain_events().is_empty());
        assert!(!unflub_core::undoable_boundary(boundary));
    }
    for boundary in [' ', '.', ',', '!', '?'] {
        assert!(unflub_core::undoable_boundary(boundary), "{boundary:?}");
    }
}

/// Every undo the engine can produce restores the word and a boundary that isn't Enter or Tab.
#[test]
fn undos_never_type_enter_or_tab() {
    for boundary in [' ', '.', ',', ';', '!', '?', ')', '"'] {
        let mut t = Typist::bundled();
        t.type_str(&format!("ok teh{boundary}"));
        if let Action::Undo(r) = t.key(Key::Backspace) {
            assert!(
                !r.insert.contains(['\n', '\r', '\t']),
                "{boundary:?}: {r:?}"
            );
        }
    }
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

fn brb() -> Typist {
    Typist::new(Config {
        user_rules: vec![("brb".into(), "be right back".into())],
        ..config()
    })
}

/// The live bug: a shortcut typed first into an empty prompt box (after a click) never
/// expanded. Without a hint nothing changes; with one in time, the first word counts.
#[test]
fn first_word_after_a_reset_needs_a_word_start_hint() {
    // No hint: conservative, as before.
    let mut t = brb();
    t.key(Key::Reset);
    assert_eq!(t.type_str("brb "), "brb ");
    assert_eq!(t.type_str("brb "), "brb be right back ");

    // The hint arrives before the first word's boundary (even mid-word): fixed.
    let mut t = brb();
    t.key(Key::Reset);
    let generation = t.engine.reset_generation();
    assert!(t.engine.caret_at_word_start(generation));
    assert_eq!(t.type_str("brb "), "be right back ");
    let mut t = brb();
    t.key(Key::Reset);
    let generation = t.engine.reset_generation();
    t.type_str("br");
    assert!(t.engine.caret_at_word_start(generation));
    assert_eq!(t.type_str("b "), "be right back ");
    // Typos too.
    let mut t = Typist::bundled();
    t.key(Key::Reset);
    let generation = t.engine.reset_generation();
    t.engine.caret_at_word_start(generation);
    assert_eq!(t.type_str("teh "), "the ");
}

#[test]
fn a_late_or_stale_word_start_hint_changes_nothing() {
    // After the first word's boundary: too late.
    let mut t = brb();
    t.key(Key::Reset);
    let generation = t.engine.reset_generation();
    t.type_str("brb ");
    assert!(!t.engine.caret_at_word_start(generation));
    assert_eq!(t.text, "brb ");
    // Another reset (an arrow key, a second click) since the question: stale.
    let mut t = brb();
    t.key(Key::Reset);
    let generation = t.engine.reset_generation();
    t.key(Key::Reset);
    assert!(!t.engine.caret_at_word_start(generation));
    assert_eq!(t.type_str("brb "), "brb ");
    // Deleting past what was seen moves the caret somewhere unknown: stale too.
    let mut t = brb();
    t.key(Key::Reset);
    let generation = t.engine.reset_generation();
    t.type_str("b");
    t.key(Key::Backspace);
    t.key(Key::Backspace);
    assert!(!t.engine.caret_at_word_start(generation));
    assert_eq!(t.type_str("brb "), "brb ");
}

/// Typing after Enter or Tab inside our own observed session already counts the first word;
/// a hint is never needed (or harmful) there.
#[test]
fn words_after_an_observed_boundary_need_no_hint() {
    let mut t = brb();
    assert_eq!(t.type_str("ok\nbrb "), "ok\nbe right back ");
    let generation = t.engine.reset_generation();
    assert!(
        !t.engine.caret_at_word_start(generation),
        "a boundary was seen"
    );
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
    // Undoing a rule does not disable it, however often.
    t.engine.drain_events();
    assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
    assert!(matches!(t.press(Key::Backspace), Action::Undo(_)));
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
    // A fence stays open across lines, also when indented or with a language tag.
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("  ```rust\nteh\n\nadn\n  ```\nteh "),
        "  ```rust\nteh\n\nadn\n  ```\nthe "
    );
}

/// #102: one stray backtick no longer turns correction off until the next click.
#[test]
fn a_stray_backtick_ends_at_the_line() {
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("press ` then teh\nteh end "),
        "press ` then teh\nthe end "
    );
    // Backspacing the stray backtick closes the span too.
    let mut t = Typist::bundled();
    t.type_str("ok `");
    t.key(Key::Backspace);
    assert_eq!(t.type_str("teh "), "ok the ");
}

#[test]
fn glue_characters_do_not_trigger() {
    let mut t = Typist::bundled();
    let text = "teh/adn teh@x.com teh: _teh ";
    assert_eq!(t.type_str(text), text);
}

/// #57: `name(` is a function call, and so are the words inside it.
#[test]
fn function_calls_are_left_alone() {
    let mut t = Typist::bundled();
    let text = "call strlen(x) or printf(\"%d\") or sprintf(lenght) now ";
    assert_eq!(t.type_str(text), text);
    // A parenthesis after a space opens an aside: its words are still fixed.
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("ok (teh adn) "), "ok (the and) ");
    let mut t = Typist::new(Config {
        skip_code: false,
        ..config()
    });
    assert_eq!(t.type_str("ok teh(x) "), "ok the(x) ");
}

/// #104: hashtags are tags, often run-together words, like @mentions.
#[test]
fn hashtags_are_left_alone() {
    let mut t = Typist::bundled();
    let text = "love #rustlang and #teh today ";
    assert_eq!(t.type_str(text), text);
    // Without code skipping they are words again.
    let mut t = Typist::new(Config {
        skip_code: false,
        ..config()
    });
    assert_eq!(t.type_str("ok #teh "), "ok #the ");
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

/// Review S5: Balanced never rewrites a word the system checker calls correct.
#[test]
fn balanced_trusts_the_system_checker() {
    let checker = FakeChecker {
        known: vec!["bien", "teh"],
        fixes: vec![],
    };
    let mut t = with_backend(EngineMode::Balanced, Some(checker));
    assert_eq!(t.type_str("ok bien teh adn "), "ok bien teh and ");
    // Without a checker the bundled dictionary decides, as before.
    let mut t = with_backend(EngineMode::Balanced, None);
    assert_eq!(t.type_str("ok teh "), "ok the ");
}

/// Review S5: with a non-English language, the English dictionary rewrites nothing unless the
/// user opts in; the OS checker's own fixes still apply.
#[test]
fn other_languages_get_no_english_fixes() {
    let french = |mode, english_for_other_languages, backend: Option<FakeChecker>| {
        let config = Config {
            mode,
            language: "fr-FR".into(),
            english_for_other_languages,
            ..Config::default()
        };
        let backend = backend.map(|b| Box::new(b) as Box<dyn SpellBackend>);
        Typist::with_engine(Engine::new(config, backend))
    };
    let checker = || FakeChecker {
        known: vec!["bien", "merci", "raison", "allons"],
        fixes: vec![("ecole", "école")],
    };
    for mode in [
        EngineMode::Both,
        EngineMode::Balanced,
        EngineMode::Aggressive,
        EngineMode::SystemOnly,
    ] {
        let mut t = french(mode, false, Some(checker()));
        assert_eq!(
            t.type_str("ok bien merci raison allons teh ecole "),
            "ok bien merci raison allons teh école ",
            "{mode:?}"
        );
        // No checker at all: nothing to go on.
        let mut t = french(mode, false, None);
        assert_eq!(t.type_str("ok bien teh "), "ok bien teh ", "{mode:?}");
    }
    // Opting in brings the English fixes back.
    let mut t = french(EngineMode::Both, true, None);
    assert_eq!(t.type_str("ok teh "), "ok the ");
    // BundledOnly is an explicit choice of the English dictionary.
    let mut t = french(EngineMode::BundledOnly, false, None);
    assert_eq!(t.type_str("ok teh "), "ok the ");
}

#[test]
fn aggressive_mode_takes_low_margin_fixes() {
    let mut bundled = Typist::bundled();
    assert_eq!(bundled.type_str("Trevix "), "Trevix ");
    let mut aggressive = with_backend(EngineMode::Aggressive, None);
    assert_ne!(aggressive.type_str("Trevix "), "Trevix ");
}

/// Quality lab: possessives with a known or capitalised base are never "fixed" into plurals or
/// bare names; a typo in a lowercase base is fixed on the base, keeping the apostrophe typed.
#[test]
fn possessives_keep_their_base() {
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("my sister's and brother's friend's book "),
        "my sister's and brother's friend's book "
    );
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("ask Collins's cousin "), "ask Collins's cousin ");
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("my sister\u{2019}s book "),
        "my sister\u{2019}s book "
    );
    // Typoed bases still get fixed, with the possessive kept as typed.
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("my sistr's book "), "my sister's book ");
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("my sistr\u{2019}s book "),
        "my sister\u{2019}s book "
    );
}

/// Quality lab: a capitalised word mid-sentence is a name; at a sentence start it is fixed.
#[test]
fn title_case_mid_sentence_is_a_name() {
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("we went to Longbourn and Meryton "),
        "we went to Longbourn and Meryton "
    );
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("ok. Teh end "), "ok. The end ");
    let mut t = Typist::bundled();
    assert_eq!(
        t.type_str("Teh cat "),
        "The cat ",
        "first word of the field"
    );
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("I said Teh "), "I said Teh ", "mid-sentence");
}

/// Quality lab: technical words the engine used to "fix".
#[test]
fn technical_words_found_by_the_quality_corpus() {
    let mut t = Typist::bundled();
    let text = "a truthy or falsy value, crypto keys, implementors ";
    assert_eq!(t.type_str(text), text);
}

/// Owner live test: Ctrl+Backspace leaves the caret at a word start, so the next word counts.
#[test]
fn ctrl_backspace_leaves_a_word_start() {
    // After a whole word and its space.
    let mut t = brb();
    t.type_str("ok wrong ");
    t.key(Key::DeleteWordLeft);
    assert_eq!(t.text, "ok ");
    assert_eq!(t.type_str("brb "), "ok be right back ");
    // Typos too.
    let mut t = Typist::bundled();
    t.type_str("ok wrong ");
    t.key(Key::DeleteWordLeft);
    assert_eq!(t.type_str("teh "), "ok the ");
    // Mid-word: the half-typed word goes, the next one is fixed.
    let mut t = Typist::bundled();
    t.type_str("ok wro");
    t.key(Key::DeleteWordLeft);
    assert_eq!(t.text, "ok ");
    assert_eq!(t.type_str("teh "), "ok the ");
    // Right after a click (a reset), Ctrl+Backspace still means a word start.
    let mut t = brb();
    t.key(Key::Reset);
    t.key(Key::DeleteWordLeft);
    assert_eq!(t.type_str("brb "), "be right back ");
}

/// Ctrl+Backspace isn't an undo, and with a Backspace held the adapter sends that first.
#[test]
fn ctrl_backspace_never_undoes() {
    let mut t = mode(UndoMode::Once, 2);
    t.type_str("ok teh ");
    assert_eq!(t.press(Key::DeleteWordLeft), Action::Pass);
    let mut t = mode(UndoMode::Twice, 2);
    t.type_str("ok teh ");
    assert!(matches!(t.press(Key::Backspace), Action::HoldBackspace(_)));
    assert_eq!(t.press(Key::DeleteWordLeft), Action::Pass);
    assert_eq!(
        t.text, "ok ",
        "the held Backspace went first, then the word"
    );
}
