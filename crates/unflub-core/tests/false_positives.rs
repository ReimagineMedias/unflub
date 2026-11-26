//! Words that must come through unchanged.

mod common;

use common::Typist;
use unflub_core::{Config, Engine, EngineMode, SpellBackend};

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

/// #55: an elided prefix before an apostrophe is never deleted.
#[test]
fn elisions_keep_their_prefix() {
    for word in [
        "d'angelo",
        "d'arcy",
        "o'reilly",
        "d'accord",
        "n'est",
        "c'est",
        "c\u{2019}est",
        "d'angelo's",
    ] {
        let mut t = Typist::bundled();
        let text = format!("so {word} ok ");
        assert_eq!(t.type_str(&text), text);
    }
    // Contractions with a typo still get fixed.
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("so do'nt "), "so don't ");
}

/// #58: possessives and plurals of the user's words are theirs too.
#[test]
fn personal_word_possessives_and_plurals_stay() {
    let mut t = bundled_with(&["folake", "Gaelle", "wechat", "teh"]);
    let text = "so folake's sister, gaelle\u{2019}s and gaelles and wechat's app, tehs ";
    assert_eq!(t.type_str(text), text);
    // Other words still get fixed.
    assert_eq!(t.type_str("adn "), format!("{text}and "));
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

/// #60: American spellings the frequency list lacked became British (favorite -> favourite),
/// and the other way round. Neither spelling is a typo of the other, in any mode.
#[test]
fn regional_spellings_stay() {
    let words = "favorite favorites colors analyze analyzing analyzed offense defense theater \
traveled canceled jewelry catalog neighbor labor humor harbor skeptical pajamas plow somber \
luster savor mom moms favourite colours analyse analysing offence theatre travelled cancelled \
jewellery catalogue neighbour labour humour harbour sceptical pyjamas plough sombre lustre \
savour behaviour organise recognise mum";
    for mode in [EngineMode::BundledOnly, EngineMode::Aggressive] {
        let mut changed = Vec::new();
        for word in words.split_ascii_whitespace() {
            let mut t = Typist::new(Config {
                mode,
                ..Config::default()
            });
            let text = format!("my {word} is ok ");
            let got = t.type_str(&text).to_string();
            if got != text {
                changed.push(got);
            }
        }
        assert!(changed.is_empty(), "{mode:?}: {}", changed.join(" | "));
    }
}

/// An OS checker whose list has none of the words: what Windows' en-US checker does for
/// shorthand, brands and lowercase names.
struct FlagsEverything;

impl SpellBackend for FlagsEverything {
    fn is_correct(&self, _: &str) -> bool {
        false
    }

    fn auto_replacement(&self, _: &str) -> Option<String> {
        None
    }
}

/// #36: chat shorthand, lowercase names, brands, acronyms and tech words became unrelated
/// common words (`tho` -> `the`, `lyft` -> `left`, `qa` -> `a`, `fn` -> `fun`).
#[test]
fn shorthand_brands_and_names_stay() {
    let words = "tho wth smth yeet okr highkey lowkey sus ofc ty theo noor bjorn bilal matias \
valentina sebastien dragan wechat lyft quora viber pinterest fortnite interac aws qa ints fn ssh \
nvme saml nuxt guid buf strlen printf undervolt gpu cpu theek yaar kuch arre";
    let modes = [
        (EngineMode::BundledOnly, false),
        (EngineMode::Both, true),
        (EngineMode::Balanced, true),
        (EngineMode::Aggressive, true),
        (EngineMode::Aggressive, false),
    ];
    for (mode, checker) in modes {
        let mut changed = Vec::new();
        for word in words.split_ascii_whitespace() {
            for text in [format!("ok {word} ok "), format!("so {word} today ")] {
                let backend = checker.then(|| Box::new(FlagsEverything) as Box<dyn SpellBackend>);
                let config = Config {
                    mode,
                    ..Config::default()
                };
                let mut t = Typist::with_engine(Engine::new(config, backend));
                let got = t.type_str(&text).to_string();
                if got != text {
                    changed.push(got);
                }
            }
        }
        assert!(
            changed.is_empty(),
            "{mode:?} checker {checker}: {}",
            changed.join(" | ")
        );
    }
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("my lyft's driver "), "my lyft's driver ");
    // The guard is for shorthand, not typos: a slip of a common word is still fixed.
    let mut t = Typist::bundled();
    assert_eq!(t.type_str("teh adn thw "), "the and the ");
}
