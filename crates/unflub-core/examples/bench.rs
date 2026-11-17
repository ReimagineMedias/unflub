//! Times engine startup and `on_key` over a realistic typing stream.
//!
//! cargo run --release -p unflub-core --example bench

use std::time::{Duration, Instant};

use unflub_core::{Config, Context, Dictionary, Engine, EngineMode, Key};

const TEXT: &str = "I think teh new build is ready, but I wouldnt ship it yet. \
If tou have a minute, could you check the settings page? The colours look off \
and the toggle for autocorrect doesnt save. We should recieve the design notes \
tomorrow; until then lets keep the scope small and focus on the issuses people \
reported. Thanks for the quick turnaround, its definately appreciated!\n";

const KEYS: usize = 100_000;

fn key_for(c: char) -> Key {
    if c.is_alphanumeric() || c == '\'' || c == '-' {
        Key::Char(c)
    } else {
        Key::Boundary(c)
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

fn report(name: &str, mut times: Vec<Duration>) {
    times.sort_unstable();
    println!(
        "{name:>9}: n={:>6}  p50={:>8.2?}  p99={:>8.2?}  max={:>8.2?}",
        times.len(),
        percentile(&times, 0.50),
        percentile(&times, 0.99),
        times[times.len() - 1],
    );
}

fn main() {
    let start = Instant::now();
    let dict = Dictionary::english();
    let startup = start.elapsed();
    println!(
        "dictionary: {} words, {} bigrams, ~{:.1} MB heap, built in {startup:.1?}",
        dict.word_count(),
        dict.bigram_count(),
        dict.heap_bytes() as f64 / 1_048_576.0
    );

    let config = Config {
        mode: EngineMode::BundledOnly,
        ..Config::default()
    };
    let mut engine = Engine::new(config, None);
    let ctx = Context::default();
    let mut boundary = Vec::new();
    let mut other = Vec::new();
    let mut corrections = 0usize;
    engine.on_key(Key::Boundary('\n'), &ctx);
    for c in TEXT.chars().cycle().take(KEYS) {
        let key = key_for(c);
        let t = Instant::now();
        let action = engine.on_key(key, &ctx);
        let dt = t.elapsed();
        if matches!(action, unflub_core::Action::Replace(_)) {
            corrections += 1;
        }
        match key {
            Key::Boundary(_) => boundary.push(dt),
            _ => other.push(dt),
        }
        if corrections % 64 == 0 {
            engine.drain_events();
        }
    }
    println!("{KEYS} keys, {corrections} corrections");
    report("boundary", boundary);
    report("char", other);

    // Words that make the candidate search work hardest: long, unknown, common prefixes.
    let mut hard = Vec::new();
    for _ in 0..20 {
        for word in HARD {
            for c in word.chars() {
                engine.on_key(key_for(c), &ctx);
            }
            let t = Instant::now();
            engine.on_key(Key::Boundary(' '), &ctx);
            hard.push(t.elapsed());
        }
        engine.drain_events();
    }
    report("hard", hard);
}

const HARD: &[&str] = &[
    "interationalization",
    "responsabilities",
    "disctation",
    "abcdefghijklmnopqrstuvwxyzabcdef",
    "qwertyuiopasdfghjklzxcvbnm",
    "eeeeeeeeeeeeeeee",
    "stationaryness",
    "inconsistantly",
    "tou",
    "fro",
    "ta",
    "thier",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "conditionaly",
    "preformance",
    "comunication",
];
