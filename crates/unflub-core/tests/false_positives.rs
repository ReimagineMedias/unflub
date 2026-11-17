//! Words that must come through unchanged.

mod common;

use common::Typist;
use unflub_core::{Config, EngineMode};

fn bundled_with(personal: &[&str]) -> Typist {
    Typist::new(Config {
        mode: EngineMode::BundledOnly,
        personal_words: personal.iter().map(|w| w.to_string()).collect(),
        ..Config::default()
    })
}

#[test]
fn code_names_and_informal_words_stay() {
    let inputs = [
        "useState",
        "snake_case",
        "src/main.rs",
        "https://x.com",
        "API",
        "Tauri",
        "I'm",
        "don't",
        "e-mail",
        "2nd",
        "ok",
        "lol",
        "gonna",
    ];
    for input in inputs {
        for text in [format!("we use {input} here "), format!("{input} ")] {
            let mut t = Typist::bundled();
            assert_eq!(t.type_str(&text), text, "{input} changed");
        }
    }
}

#[test]
fn personal_words_are_never_corrected() {
    let mut t = bundled_with(&["RMGN", "teh"]);
    assert_eq!(t.type_str("rmgn teh Teh "), "rmgn teh Teh ");
}

#[test]
fn unknown_word_without_a_close_match_stays() {
    // Not in the personal dictionary and no confident candidate.
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("rmgn "), "rmgn ");
}

#[test]
fn capitalised_name_mid_sentence_stays() {
    let mut t = Typist::bundled();
    let text = "we built it with Tokio and Tauri ";
    assert_eq!(t.type_str(text), text);
}

#[test]
fn known_words_stay() {
    let mut t = Typist::bundled();
    let text = "the quick brown fox jumps over the lazy dog, its fine and we're well. ";
    assert_eq!(t.type_str(text), text);
}
