//! Times engine startup and `on_key` over a realistic typing stream.
//!
//! ```text
//! cargo run --release -p unflub-core --example bench                         human-readable
//! cargo run --release -p unflub-core --example bench -- --json               one JSON object
//! cargo run --release -p unflub-core --example bench -- --check perf/limits.toml [--section core]
//! ```
//!
//! `--check` compares every metric that has a limit in the given section of the limits file
//! (default `core`) and exits with status 1 if any is over. See `docs/performance.md`.

use std::time::{Duration, Instant};

use unflub_core::{Config, Context, Dictionary, Engine, EngineMode, Key};

const TEXT: &str = "I think teh new build is ready, but I wouldnt ship it yet. \
If tou have a minute, could you check the settings page? The colours look off \
and the toggle for autocorrect doesnt save. We should recieve the design notes \
tomorrow; until then lets keep the scope small and focus on the issuses people \
reported. Thanks for the quick turnaround, its definately appreciated!\n";

const KEYS: usize = 100_000;

const USAGE: &str = "usage: bench [--json] [--check <limits.toml>] [--section <name>]

  --json              print one JSON object instead of the human-readable report
  --check <file>      compare against the limits in <file>; exit 1 if any is exceeded
  --section <name>    which [section] of the limits file to use (default: core)";

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

struct Stats {
    n: usize,
    p50: Duration,
    p99: Duration,
    /// The second-slowest sample: one preempted sample on a shared runner can't fail the gate,
    /// while a slow path that shows up more than once still does.
    max2: Duration,
    max: Duration,
}

fn stats(mut times: Vec<Duration>) -> Stats {
    times.sort_unstable();
    Stats {
        n: times.len(),
        p50: percentile(&times, 0.50),
        p99: percentile(&times, 0.99),
        max2: times[times.len().saturating_sub(2)],
        max: times[times.len() - 1],
    }
}

fn line(name: &str, s: &Stats) -> String {
    format!(
        "{name:>9}: n={:>6}  p50={:>8.2?}  p99={:>8.2?}  max2={:>8.2?}  max={:>8.2?}",
        s.n, s.p50, s.p99, s.max2, s.max
    )
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

struct Options {
    json: bool,
    check: Option<String>,
    section: String,
}

fn parse_args() -> Result<Options, String> {
    let mut out = Options {
        json: false,
        check: None,
        section: "core".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => out.json = true,
            "--check" => out.check = Some(args.next().ok_or("--check needs a file")?),
            "--section" => out.section = args.next().ok_or("--section needs a name")?,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(out)
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

    let mut human = Vec::new();
    let start = Instant::now();
    let dict = Dictionary::english();
    let startup = start.elapsed();
    human.push(format!(
        "dictionary: {} words, {} bigrams, ~{:.1} MB heap, built in {startup:.1?}",
        dict.word_count(),
        dict.bigram_count(),
        dict.heap_bytes() as f64 / 1_048_576.0
    ));

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
    human.push(format!("{KEYS} keys, {corrections} corrections"));
    let boundary = stats(boundary);
    let char = stats(other);
    human.push(line("boundary", &boundary));
    human.push(line("char", &char));

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
    let hard = stats(hard);
    human.push(line("hard", &hard));

    let peak_rss = peak_rss_bytes();
    human.push(match peak_rss {
        Some(b) => format!("peak RSS: {:.1} MB", b as f64 / 1_048_576.0),
        None => "peak RSS: not available on this OS".into(),
    });

    let metrics: Vec<(&'static str, Option<f64>)> = vec![
        ("startup_ms", Some(startup.as_secs_f64() * 1e3)),
        ("boundary_p50_us", Some(us(boundary.p50))),
        ("boundary_p99_us", Some(us(boundary.p99))),
        ("boundary_max2_us", Some(us(boundary.max2))),
        ("boundary_max_us", Some(us(boundary.max))),
        ("char_p99_us", Some(us(char.p99))),
        ("hard_max2_us", Some(us(hard.max2))),
        ("hard_max_us", Some(us(hard.max))),
        ("heap_bytes", Some(dict.heap_bytes() as f64)),
        ("peak_rss_bytes", peak_rss.map(|b| b as f64)),
    ];

    let checks = match &limits {
        Some(limits) => limits::compare(limits, &metrics).unwrap_or_else(|e| {
            eprintln!("{}: {e}", opts.check.as_deref().unwrap_or_default());
            std::process::exit(2);
        }),
        None => Vec::new(),
    };
    let pass = checks.iter().all(|c| c.pass != Some(false));

    if opts.json {
        println!("{}", to_json(&opts, &metrics, &checks, pass));
    } else {
        for l in &human {
            println!("{l}");
        }
        for c in &checks {
            let verdict = match c.pass {
                Some(true) => "ok  ",
                Some(false) => "FAIL",
                None => "skip",
            };
            let value = c.value.map_or("n/a".into(), fmt_num);
            println!(
                "{verdict} {:<16} {value} (limit {})",
                c.metric,
                fmt_num(c.limit)
            );
        }
    }
    if !pass {
        std::process::exit(1);
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v:.3}")
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_num(v: Option<f64>) -> String {
    v.map_or("null".into(), fmt_num)
}

/// Same shape as `perf/idle.ps1` prints, so `perf/summary.ps1` reads both.
fn to_json(
    opts: &Options,
    metrics: &[(&str, Option<f64>)],
    checks: &[limits::Check],
    pass: bool,
) -> String {
    let metrics = metrics
        .iter()
        .map(|(k, v)| format!("{}: {}", json_str(k), json_num(*v)))
        .collect::<Vec<_>>()
        .join(", ");
    let checks = checks
        .iter()
        .map(|c| {
            format!(
                "{{\"metric\": {}, \"value\": {}, \"limit\": {}, \"pass\": {}}}",
                json_str(c.metric),
                json_num(c.value),
                fmt_num(c.limit),
                c.pass.map_or("null".into(), |p| p.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let section = match opts.check {
        Some(_) => json_str(&opts.section),
        None => "null".into(),
    };
    format!(
        "{{\"name\": \"core bench\", \"os\": {}, \"section\": {section}, \"metrics\": {{{metrics}}}, \
         \"checks\": [{checks}], \"notes\": [], \"pass\": {pass}}}",
        json_str(std::env::consts::OS)
    )
}

#[path = "shared/limits.rs"]
mod limits;

/// Peak resident set size of this process, where the OS reports it cheaply.
#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kb * 1024)
}

/// Peak working set of this process.
#[cfg(windows)]
fn peak_rss_bytes() -> Option<u64> {
    /// PROCESS_MEMORY_COUNTERS: two u32s, then eight SIZE_Ts, the first being the peak.
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        _page_fault_count: u32,
        peak_working_set_size: usize,
        _rest: [usize; 7],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: the pseudo-handle needs no closing; `c` is a correctly sized
    // PROCESS_MEMORY_COUNTERS that lives for the call.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some(c.peak_working_set_size as u64)
}

/// Peak resident set size of this process (`ru_maxrss`, in bytes on macOS).
#[cfg(target_os = "macos")]
fn peak_rss_bytes() -> Option<u64> {
    // struct rusage on 64-bit macOS: two 16-byte timevals, then 14 longs; ru_maxrss is first.
    extern "C" {
        fn getrusage(who: i32, usage: *mut i64) -> i32;
    }
    let mut usage = [0i64; 18];
    // SAFETY: RUSAGE_SELF (0) and a buffer the size of struct rusage.
    let ok = unsafe { getrusage(0, usage.as_mut_ptr()) };
    (ok == 0).then_some(usage[4] as u64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn peak_rss_bytes() -> Option<u64> {
    None
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
