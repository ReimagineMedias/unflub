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
| `boundary_p50_us`, `boundary_p99_us`, `boundary_max_us` | Time to decide about a word when you type a space or punctuation. This runs inside the keyboard hook, which has 1 ms per key. |
| `char_p99_us` | Time to handle a letter key. |
| `hard_max_us` | The slowest decision on the hardest words: long, unknown, many near misses. |
| `heap_bytes` | Memory the dictionary index holds. |
| `peak_rss_bytes` | The most memory the bench process ever had resident (where the OS reports it). |

**The idle footprint** (`perf/idle.ps1`, Windows). It starts `unflub-cli.exe --dry-run --pill off`
with the keyboard hook installed, waits until the hook is in, waits 3 more seconds, then samples
the process every second for 20 seconds.

| Metric | What it means |
|---|---|
| `ready_ms` | From launch until the hook is installed and the engine is ready. |
| `private_bytes` | Memory only unflub uses (peak over the samples). |
| `working_set_bytes` | Memory resident in RAM, including shared Windows DLLs. |
| `cpu_seconds` | CPU time used over the 20 idle seconds. |
| `threads`, `handles` | Peak thread and handle counts. |

`--dry-run` installs the hook and runs the engine on real keys, but never sends input and never
swallows a key: every fix it would make is reported as skipped and the engine is reset, as for
any other vetoed fix. The perf scripts send no input of any kind, and stop only the processes
they started.

**The low-end simulation** (`perf/lowend.ps1`, Windows). Both checks above run again inside a
Windows Job Object that allows:

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
| Idle: ready | 0.2–1.0 s | 0.2–1.4 s | 3 / 10 s |
| Idle: private memory | 18.8 MB | 18.9 MB | 40 MB |
| Idle: working set | 38–40 MB | 38–40 MB | 64 MB |
| Idle: CPU over 20 s | 0.016–0.047 s | 0.016–0.125 s | 0.2 s |
| Idle: threads | 11–12 | 11–12 | 16 |
| Idle: handles | 207–215 | 210–215 | 400 |

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

12 threads at first, 9 after about a minute. The extra three are Windows thread-pool workers
started while loading DLLs, which retire when idle. Ours are the main thread, `unflub-hook`,
`unflub-inject`, `unflub-uia` and `unflub-spell`; the rest belong to Windows components we load
(COM, UI Automation, the spell checker). The pill adds `unflub-overlay` and `unflub-caret` when it is
on.

## Running it yourself

Build from PowerShell (in Git Bash, `/usr/bin/link` hides the MSVC linker):

```powershell
cargo build --release -p unflub-core --example bench
cargo build --release -p unflub-windows --bin unflub-cli --example jobrun

./target/release/examples/bench --check perf/limits.toml   # human-readable; --json for JSON
pwsh perf/idle.ps1                                          # ~25 s
pwsh perf/lowend.ps1                                        # ~25 s
pwsh -c "./perf/summary.ps1 -Path out1.json, out2.json"     # markdown table from saved JSON
```

The scripts run in PowerShell 7 and Windows PowerShell 5.1. Each prints one JSON report per
line on stdout and exits 1 when a limit is exceeded, 2 when the run itself failed.

Running `idle.ps1` on a machine where someone is typing is safe: the dry-run hook only watches,
and a normal unflub that is already running is left alone.

If a CI runner can't install the keyboard hook, the dry run keeps the engine loaded, and the
summary says "hook unavailable on runner": those idle numbers leave out the hook thread.

## Changing a limit

Measure first, then edit `perf/limits.toml` and update the measured value in its comment. Keep
headroom for slow, shared CI machines. A key that names no metric is an error, so a typo can't
switch a check off.

## Not measured yet

- The pill: the idle check runs with `--pill off`. With it on, the overlay and caret threads
  are added, and Direct2D is loaded briefly after each fix.
- Linux and macOS idle footprint: there is no adapter for them yet. The engine bench runs there.
- Per-key latency inside the real hook under load: the bench times the engine alone. `unflub-cli`
  prints the hook's own latency histogram when you stop it.
