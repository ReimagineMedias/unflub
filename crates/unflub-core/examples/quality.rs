//! Autocorrect quality on real text: how often the engine "fixes" clean prose, and how well it
//! recovers realistic single-letter typos. CI fails when either gets worse (`[quality]` in
//! `perf/limits.toml`).
//!
//! ```text
//! cargo run --release -p unflub-core --example quality                         human-readable
//! cargo run --release -p unflub-core --example quality -- --json               one JSON object
//! cargo run --release -p unflub-core --example quality -- --check perf/limits.toml
//! cargo run --release -p unflub-core --example quality -- --dump .local/quality
//! ```
//!
//! Data: `data/quality/` (sources and licences in `ATTRIBUTION.md`).
//!
//! - **False fixes.** About 100,000 words of clean text: *Pride and Prejudice*, the Node.js
//!   stream docs and chapters of the Rust book. Each paragraph is reset and primed with `x `
//!   (so its first word counts as seen), then typed; every replacement counts as a false fix,
//!   including spelling variants and errors in the source. Reported per 10,000 source words.
//! - **Typo recovery.** 5,000 single typos (1,250 each of adjacent key, transposition, dropped
//!   and doubled letter) in their original context. Exact: the clean word comes back; wrong: a
//!   different word; no fix: left alone.
//!
//! The engine runs with the default config and no OS spell checker (`Both` then uses the
//! bundled dictionary), the same on every OS. No hook, nothing sent anywhere.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use unflub_core::{Action, Config, Context, Engine, Key};

#[path = "shared/limits.rs"]
mod limits;

const USAGE: &str =
    "usage: quality [--json] [--check <limits.toml>] [--section <name>] [--dump <dir>]

  --json              print one JSON object instead of the human-readable report
  --check <file>      compare against the limits in <file>; exit 1 if any is missed
  --section <name>    which [section] of the limits file to use (default: quality)
  --dump <dir>        write every false fix and typo outcome as TSV files there";

/// Metrics where higher is better: the limit is a minimum.
const AT_LEAST: &[&str] = &["typo_exact_pct"];

fn key(c: char) -> Key {
    if c.is_alphanumeric() || matches!(c, '\'' | '\u{2019}' | '-' | '_') {
        Key::Char(c)
    } else {
        Key::Boundary(c)
    }
}

/// A fresh paragraph or trial: reset, then `x ` so the next word counts as fully seen.
fn prime(e: &mut Engine, ctx: &Context<'_>) {
    e.on_key(Key::Reset, ctx);
    e.on_key(Key::Char('x'), ctx);
    e.on_key(Key::Boundary(' '), ctx);
}

#[derive(Default)]
struct Options {
    json: bool,
    check: Option<String>,
    section: String,
    dump: Option<PathBuf>,
}

fn parse_args() -> Result<Options, String> {
    let mut out = Options {
        section: "quality".into(),
        ..Options::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => out.json = true,
            "--check" => out.check = Some(args.next().ok_or("--check needs a file")?),
            "--section" => out.section = args.next().ok_or("--section needs a name")?,
            "--dump" => out.dump = Some(args.next().ok_or("--dump needs a directory")?.into()),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(out)
}

fn read(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        // Measured on LF text whatever git did to the checkout.
        .replace("\r\n", "\n")
}

struct Outcome {
    words: u64,
    false_fixes: u64,
    per_source: Vec<(String, u64, u64)>,
    exact: u64,
    wrong: u64,
    none: u64,
    fix_rows: String,
    typo_rows: String,
}

fn run(dir: &Path) -> Outcome {
    let ctx = Context::default();
    let mut engine = Engine::new(Config::default(), None);
    let mut out = Outcome {
        words: 0,
        false_fixes: 0,
        per_source: Vec::new(),
        exact: 0,
        wrong: 0,
        none: 0,
        fix_rows: "source\tparagraph\toriginal\tcorrected\n".into(),
        typo_rows: "id\ttype\tsource\tclean\ttypo\tactual\tresult\n".into(),
    };
    for line in read(dir, "corpora.tsv").lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        let (source, file, words) = (cols[0], cols[1], cols[2].parse::<u64>().unwrap());
        let mut fixes = 0;
        for (p, paragraph) in read(dir, file).split("\n\n").enumerate() {
            prime(&mut engine, &ctx);
            for c in paragraph.chars().chain(std::iter::once(' ')) {
                if let Action::Replace(r) = engine.on_key(key(c), &ctx) {
                    fixes += 1;
                    let _ = writeln!(
                        out.fix_rows,
                        "{source}\t{p}\t{}\t{}",
                        r.original, r.corrected
                    );
                }
            }
        }
        out.words += words;
        out.false_fixes += fixes;
        out.per_source.push((source.to_string(), words, fixes));
    }
    for line in read(dir, "typos.tsv").lines().skip(1) {
        let c: Vec<&str> = line.split('\t').collect();
        let (clean, typo, context) = (c[3], c[4], c[5]);
        prime(&mut engine, &ctx);
        // The context goes through the engine like any typing; only the target is scored.
        for ch in context.chars() {
            engine.on_key(key(ch), &ctx);
        }
        for ch in typo.chars() {
            engine.on_key(Key::Char(ch), &ctx);
        }
        let actual = match engine.on_key(Key::Boundary(' '), &ctx) {
            Action::Replace(r) => r.corrected,
            _ => typo.to_owned(),
        };
        let result = if actual == clean {
            out.exact += 1;
            "exact"
        } else if actual == typo {
            out.none += 1;
            "none"
        } else {
            out.wrong += 1;
            "wrong"
        };
        let _ = writeln!(
            out.typo_rows,
            "{}\t{}\t{}\t{clean}\t{typo}\t{actual}\t{result}",
            c[0], c[1], c[2]
        );
    }
    out
}

fn round(v: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (v * f).round() / f
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    // Read the limits first, so a typo fails before the run.
    let limits = opts.check.as_deref().map(|path| {
        limits::load(path, &opts.section).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(2);
        })
    });
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/quality");
    let o = run(&dir);
    let trials = (o.exact + o.wrong + o.none) as f64;
    let pct = |n: u64| round(100.0 * n as f64 / trials, 2);
    let metrics: Vec<(&'static str, Option<f64>)> = vec![
        ("clean_words", Some(o.words as f64)),
        ("false_fixes", Some(o.false_fixes as f64)),
        (
            "false_fixes_per_10k",
            Some(round(o.false_fixes as f64 * 10_000.0 / o.words as f64, 2)),
        ),
        ("typo_trials", Some(trials)),
        ("typo_exact_pct", Some(pct(o.exact))),
        ("typo_wrong_pct", Some(pct(o.wrong))),
        ("typo_none_pct", Some(pct(o.none))),
    ];
    if let Some(dump) = &opts.dump {
        std::fs::create_dir_all(dump).expect("create the dump directory");
        std::fs::write(dump.join("false-fixes.tsv"), &o.fix_rows).expect("write false-fixes.tsv");
        std::fs::write(dump.join("typo-results.tsv"), &o.typo_rows)
            .expect("write typo-results.tsv");
    }
    let checks = match &limits {
        Some(l) => limits::compare_with(l, &metrics, AT_LEAST).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2);
        }),
        None => Vec::new(),
    };
    let pass = checks.iter().all(|c| c.pass != Some(false));
    let notes: Vec<String> = o
        .per_source
        .iter()
        .map(|(s, w, f)| {
            format!(
                "{s}: {f} false fixes in {w} words ({:.2} per 10k)",
                *f as f64 * 10_000.0 / *w as f64
            )
        })
        .collect();
    if opts.json {
        println!("{}", to_json(&opts, &metrics, &checks, &notes, pass));
    } else {
        for (k, v) in &metrics {
            println!("{k:>22}: {}", v.map_or("n/a".into(), |v| v.to_string()));
        }
        for n in &notes {
            println!("  {n}");
        }
        for c in &checks {
            let verdict = match c.pass {
                Some(true) => "ok",
                Some(false) => "FAIL",
                None => "n/a",
            };
            let op = if AT_LEAST.contains(&c.metric) {
                ">="
            } else {
                "<="
            };
            println!("{verdict:>4}  {} {op} {}", c.metric, c.limit);
        }
    }
    if !pass {
        std::process::exit(1);
    }
}

fn json_str(s: &str) -> String {
    serde_like_escape(s)
}

fn serde_like_escape(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Same shape as the bench and `perf/idle.ps1`, so `perf/summary.ps1` reads it.
fn to_json(
    opts: &Options,
    metrics: &[(&str, Option<f64>)],
    checks: &[limits::Check],
    notes: &[String],
    pass: bool,
) -> String {
    let metrics = metrics
        .iter()
        .map(|(k, v)| format!("{}: {}", json_str(k), v.map_or("null".into(), num)))
        .collect::<Vec<_>>()
        .join(", ");
    let checks = checks
        .iter()
        .map(|c| {
            format!(
                "{{\"metric\": {}, \"value\": {}, \"limit\": {}, \"pass\": {}}}",
                json_str(c.metric),
                c.value.map_or("null".into(), num),
                num(c.limit),
                c.pass.map_or("null".into(), |p| p.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let notes = notes
        .iter()
        .map(|n| json_str(n))
        .collect::<Vec<_>>()
        .join(", ");
    let section = match opts.check {
        Some(_) => json_str(&opts.section),
        None => "null".into(),
    };
    format!(
        "{{\"name\": \"autocorrect quality\", \"os\": {}, \"section\": {section}, \"metrics\": {{{metrics}}}, \
         \"checks\": [{checks}], \"notes\": [{notes}], \"pass\": {pass}}}",
        json_str(std::env::consts::OS)
    )
}
