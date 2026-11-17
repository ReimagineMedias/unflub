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

/// Programming terms, tools, names and chat shorthand typed in prose to coding agents.
const JARGON: &str = "tauri kubernetes zustand vite supabase vercel figma svelte deno nginx \
grafana pydantic webpack tailwind shadcn prisma hono zod ollama qdrant llama gemini anthropic \
lmao tbh idk imo ngl rn brb afaik smh yall gotcha dunno lemme gimme kinda sorta yup nah meh ugh \
hmm haha hahaha lmk wfh ttyl unflub useeffect tokio serde clippy rustc npx pnpm eslint vitest \
jsx tsx ok lol gonna stdin println repl regex sql postgres mongo redis uuid utf ascii dev devs \
prod env src lib cfg impl struct enum fn args init async await todo btw pls thx kthx cya wtf omw \
irl dm dms lgtm wip nit nits ack txt png jpg svg csv yaml toml ttf woff dockerfile kubectl \
nodejs deps dep repo readme changelog bool str usize vec arc mutex refcell ctx cb fmt kwargs \
auth stdout stderr sudo grep sed awk frontend backend middleware refactor linter monorepo \
webhook localhost";

#[test]
fn programming_vocabulary_stays() {
    let mut changed = Vec::new();
    for word in JARGON.split_ascii_whitespace() {
        let mut t = Typist::bundled();
        let text = format!("{word} ");
        let got = t.type_str(&text).to_string();
        if got != text {
            changed.push(format!("{word}->{}", got.trim_end()));
        }
    }
    assert!(changed.is_empty(), "changed: {}", changed.join(" "));
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
