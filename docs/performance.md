# Performance

unflub runs all the time, so it has to be cheap all the time, on slow laptops as well as fast
ones. CI measures this on every pull request and fails the build when a number goes over its
limit. The limits live in [`perf/limits.toml`](../perf/limits.toml), each with the value we
measured next to it.

## What we measure

**The engine** (`crates/unflub-core/examples/bench.rs`, every OS). It builds the dictionary
index, then feeds the engine 100,000 keys of realistic typing with typos, then the hardest
unknown words we know of.

| Metric | What it means |
|---|---|
| `startup_ms` | Time to build the dictionary index. unflub can't fix anything until it's done. |
| `boundary_p50_us`, `boundary_p99_us`, `boundary_max2_us` | Time to decide about a word when you type a space or punctuation. This runs inside the keyboard hook, which has 1 ms per key. The gate uses the second-slowest sample, so one preempted sample on a shared runner can't fail it; `boundary_max_us` is still reported. |
| `char_p99_us` | Time to handle a letter key. |
| `hard_max2_us` | The second-slowest decision on the hardest words: long, unknown, many near misses (`hard_max_us` is reported, not gated). |
| `heap_bytes` | Memory the dictionary index holds. |
| `peak_rss_bytes` | The most memory the bench process ever had resident (where the OS reports it). |

**Autocorrect quality** (`crates/unflub-core/examples/quality.rs`, every OS). Not speed but
how often the engine is wrong: about 100,000 words of clean text (*Pride and Prejudice*, the
Node.js stream docs, chapters of the Rust book; `data/quality/ATTRIBUTION.md`) typed through the
engine, where every replacement counts as a false fix, and 5,000 realistic single-letter typos in
their original context (adjacent key, transposed, dropped and doubled letters, 1,250 each).
Default config, no OS spell checker, so the numbers are the same on every OS.

| Metric | What it means | Before / after 2026-11-17 | Limit |
|---|---|---:|---:|
| `false_fixes_per_10k` | Replacements per 10,000 clean words (lower is better) | 48.2 / 7.9 | at most 10 |
| `typo_exact_pct` | Typos turned back into the right word | 82.26 / 85.86 | at least 84 |
| `typo_wrong_pct` | Typos turned into a different word | 2.92 / 1.72 | at most 2.5 |

The 2026-11-17 changes came from an independent quality lab: possessives with a known or
capitalised base are left alone (`sister's` was "fixed" to `sisters`), a capitalised word
mid-sentence is a name and gets no fix (`Longbourn` to `Longhorn`), four technical words were
added, and a missing letter costs less when ranking. A gentler length penalty for unknown words
cut false fixes too but lost exact fixes, so it was not adopted. `--dump <dir>` writes every
false fix and typo outcome as TSV.

**The idle footprint** (`perf/idle.ps1`, Windows). unflub is two processes (see
[architecture.md](architecture.md#two-processes)), and this measures both. It starts
`unflub.exe --perf-idle <temp dir>`: the tray app with its settings and data in a temporary
folder, every network feature off, no settings window, and the `unflub-engine.exe` it starts
with `--dry-run`. It waits until the engine's hook is in, waits 3 more seconds, then samples
both processes every second for 20 seconds. Each process has its own limits
(`[idle.engine]`, `[idle.app]`) and so does the pair (`[idle.total]`, the peak of the
per-sample sums). With `-Exe target/release/unflub-cli.exe` it measures the one-process
developer runner instead (`[idle]`, `--pill off`).

| Metric | What it means |
|---|---|
| `ready_ms` | From launch until the hook is installed and the engine is ready. |
| `private_bytes` | Memory only unflub uses (peak over the samples). |
| `working_set_bytes` | Memory resident in RAM, including shared Windows DLLs. |
| `cpu_seconds` | CPU time used over the 20 idle seconds. |
| `threads`, `handles` | Peak thread and handle counts. |

`--dry-run` (the engine's and the CLI's) installs the hook and runs the engine on real keys, but
never sends input and never swallows a key: every fix it would make is reported as skipped and the engine is reset, as for
any other vetoed fix. The perf scripts send no input of any kind, and stop only the processes
they started.

**The low-end simulation** (`perf/lowend.ps1`, Windows). The core bench and the idle check of
`unflub-cli.exe` (the same engine and hook in one process) run again inside a Windows Job Object
that allows:

- 25 % of **one** logical core (a hard cap; Windows takes it as a share of the whole machine,
  so on 32 logical cores that is 78/10,000),
- one core of affinity,
- 128 MB of committed memory per process (allocations past it fail).

The helper that sets this up is `crates/unflub-windows/examples/jobrun.rs`.

## Current numbers

Measured on 2026-11-16 on the development laptop (Windows 11, 32 logical cores), release build,
several runs each, while someone was typing (each key went through the dry-run hook).

| Metric | This machine | Low-end sim | Limit (normal / low-end) |
|---|---:|---:|---:|
| Index build (`startup_ms`) | 93–139 ms | 99–183 ms | 600 / 2,000 ms |
| Word decision p99 | 87–90 µs | 88–101 µs | 1,000 µs |
| Word decision max | 0.2–1.0 ms | up to 113 ms (see below) | 10 ms / none |
| Hardest word max | 0.15–0.45 ms | 0.2–0.25 ms | 2 ms |
| Letter key p99 | 0.1 µs | 0.1 µs | 20 µs |
| Index memory | 14.9 MB | 14.9 MB | 24 MB |
| Bench peak resident | 44 MB | 44 MB (34 MB committed) | 80 MB |
| Idle (`unflub-cli`): ready | 0.2–1.0 s | 0.2–1.4 s | 3 / 10 s |
| Idle (`unflub-cli`): private memory | 18.8 MB | 18.9 MB | 40 MB |
| Idle (`unflub-cli`): working set | 38–40 MB | 38–40 MB | 64 MB |
| Idle (`unflub-cli`): CPU over 20 s | 0.016–0.047 s | 0.016–0.125 s | 0.2 s |
| Idle (`unflub-cli`): threads | 11–12 | 11–12 | 16 |
| Idle (`unflub-cli`): handles | 207–215 | 210–215 | 400 |

The shipping pair, measured 2026-11-17 on the same machine (four runs, pill on, settings window
closed). Before the split the tray app ran all of this in one process: about 20 MB private and
0.08 s of CPU per 20 s.

| Metric | `unflub-engine.exe` | `unflub.exe` | Both | Limits (engine / app / both) |
|---|---:|---:|---:|---:|
| Ready (launch to hook installed) | 0.17–1.0 s | | | 3 s |
| Private memory | 19.5–19.8 MB | 4.3–4.4 MB | 23.8–24.0 MB | 40 / 24 / 64 MB |
| Working set | 40.6–41.8 MB | 15.7–16.6 MB | 56–58 MB | 64 / 64 / 128 MB |
| CPU over 20 s | 0–0.047 s | 0 s | 0–0.047 s | 0.2 / 0.1 / 0.3 s |
| Threads | 18–19 | 40 | 58–59 | 24 / 64 / 88 |
| Handles | 264–275 | 239–248 | 503–523 | 400 / 400 / 800 |

The app's 40 threads are mostly the async runtime's workers, one per logical core (32 here).

0.1 µs is the timer's resolution on this machine; letter keys take less. CPU time is counted in
15.6 ms ticks, so small idle numbers move in steps.

**CI numbers:** not recorded yet. Each run of the `perf` job writes a table like the one above
to its job summary; copy the first stable numbers for `windows-latest`, `ubuntu-latest` and
`macos-latest` here.

### What the low-end run shows, and what it doesn't

A hard CPU cap is a duty cycle, not a slower processor. While the job is busy, Windows lets it
run about 150 ms and then pauses it about 430 ms. So:

- Work that fits in one burst runs at full speed. The index build (about 100 ms) does, which is
  why `startup_ms` barely moves.
- A key that arrives during a pause waits for it. The bench types non-stop, so a few of its
  20,000 word decisions land in a pause: that is the 113 ms maximum, and why the low-end
  section has no limit on it. In real use unflub is idle between keys and almost never uses up
  a burst.
- The memory limit is real: unflub must build its index and run inside 128 MB of commit, and
  it does (34 MB peak).

To test a genuinely slower CPU, run the same scripts on old hardware or a small VM.

### Threads

`unflub-cli`: 12 threads at first, 9 after about a minute. The extra three are Windows thread-pool workers
started while loading DLLs, which retire when idle. Ours are the main thread, `unflub-hook`,
`unflub-inject`, `unflub-uia` and `unflub-spell`; the rest belong to Windows components we load
(COM, UI Automation, the spell checker). The pill adds `unflub-overlay` and `unflub-caret` when it is
on.

## Running it yourself

Build from PowerShell (in Git Bash, `/usr/bin/link` hides the MSVC linker):

```powershell
cargo build --release -p unflub-core --example bench
cargo build --release -p unflub-windows --bin unflub-cli --example jobrun
cd app; npm ci; npm run build; cd ..               # the app embeds the settings UI
cargo build --release -p unflub-app -p unflub-engine

./target/release/examples/bench --check perf/limits.toml   # human-readable; --json for JSON
cargo run --release -p unflub-core --example quality -- --check perf/limits.toml
pwsh perf/idle.ps1                                          # ~25 s
pwsh perf/lowend.ps1                                        # ~25 s
pwsh -c "./perf/summary.ps1 -Path out1.json, out2.json"     # markdown table from saved JSON
```

The scripts run in PowerShell 7 and Windows PowerShell 5.1. Each prints one JSON report per
line on stdout and exits 1 when a limit is exceeded, 2 when the run itself failed.

Running `idle.ps1` on a machine where someone is typing is safe: the dry-run hook only watches,
and a normal unflub that is already running is left alone (`--perf-idle` skips the single
instance check and uses its own settings folder). Its tray icon shows for the 25 seconds.

If a CI runner can't install the keyboard hook, the dry run keeps the engine loaded, and the
summary says "hook unavailable on runner": those idle numbers leave out the hook thread.

## Changing a limit

Measure first, then edit `perf/limits.toml` and update the measured value in its comment. Keep
headroom for slow, shared CI machines. A key that names no metric is an error, so a typo can't
switch a check off.

## Not measured yet

- The pill in the CLI: its idle check runs with `--pill off`. The pair runs with the default
  pill, whose overlay thread waits idle; Direct2D is loaded only briefly after each fix.
- The pair under the low-end Job Object: `lowend.ps1` still measures `unflub-cli`.
- Linux and macOS idle footprint: there is no adapter for them yet. The engine bench runs there.
- Per-key latency inside the real hook under load: the bench times the engine alone. `unflub-cli`
  prints the hook's own latency histogram when you stop it.
