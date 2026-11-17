# unflub

System-wide, phone-style autocorrect. RMGN product, site at unflub.rmgn.net (Vercel), API at api.rmgn.net, files at dl.rmgn.net.
Keep `CLAUDE.md` identical in substance.

Git workflow: PRs. Branch `type/topic`, open a PR on ReimagineMedias/unflub, merge only when the owner says so (squash).
Repo is private until v0.1, then flipped public.

## Findings

- Build from PowerShell, not Git Bash: Git Bash's `/usr/bin/link` shadows MSVC `link.exe` and every link fails.
- Windows `ISpellChecker` measured 2026-11-16: `Check` on a correct word ~0.04 ms, `Suggest` 5-100 ms. Only `Check` (and the
  auto-replace lookup via `ComprehensiveCheck`, when fast) may run on the hook path. Top suggestion right on 20/23 real typos;
  it flags identifiers like `useState`, so code-skip rules matter.
- The hook path budget is 1 ms worst case; corrections are applied synchronously inside the hook so fast typing can't interleave.
- Brand: default theme palette `#2D1916` espresso, `#79453A` brown, `#F2E6E3` blush paper, `#B06A5C` rose. Other themes
  (RMGN paper, quiet ink, System accent) are settings, not replacements. Fonts Manrope + IBM Plex Mono.
- Pill default: brief underline on the fixed word plus a small capsule (~1 s). Capsule, underline-only and off are all settings.
- Core engine (`crates/unflub-core`): SymSpell (MIT) frequency lists in `data/`, embedded with `include_str!`. Index build
  ~100 ms release, ~15 MB heap. Boundary decision p99 ~90 us, hard unknown words ~150 us max. Ranking weights live in
  `src/rank.rs` (`TUNING`) and `src/distance.rs`; after changing them run
  `cargo test --release -p unflub-core --test corpus -- --nocapture` (owner corpus 22/25, common typos 103/111) and
  `cargo run --release -p unflub-core --example bench`.
- Pill (`overlay.rs`, `caret.rs`): `cargo run -p unflub-windows --example pill_snapshot` renders every style offscreen to
  `.local/pill/*.png` without showing a window; check visuals there, never by running the hook. Fonts resolve to
  Segoe UI Variable Text and Cascadia Mono on Windows 11. Software Direct2D renderer setup is ~2 ms warm (26 ms on
  first DLL load), so it is dropped whenever the pill hides.
- No caret position (pill skipped): terminals, games, canvas editors, and browser/XAML fields whose UIA provider has no
  TextPattern. A Win32 caret gives only the line; the word width is estimated (0.37 x line height per letter).
- Windows `ComprehensiveCheck` measured 2026-11-16 (Windows 11 26300, en-US): ~3 ms per typo, and its auto-replace list
  has `recieve`->`receive`, `becuase`->`because` but nothing for `teh`. `CORRECTIVE_ACTION_REPLACE` is rare; don't rely on it.
- `unflub-windows` tests call the real `ISpellChecker` but never install hooks or inject input. Try the hook live with
  `cargo run --release --bin unflub -- --demo` (rules teh/adn/recieve). Never run it in automation on a machine someone
  is typing on, except with `--dry-run`.
- Windows `ISpellChecker::Check` reports any word of 64+ UTF-16 units as correct.
- The tray app runs the hook in-process (`app/src-tauri/src/engine.rs`), so `npm run tauri dev` installs a real keyboard
  hook too: same rule as the CLI, never in automation. Settings apply live through `unflub_windows::LiveEngine` (a
  channel the hook thread drains at the next key); pausing unhooks. The Tauri main thread is an STA, so the spell
  checker is created on the `unflub-engine` thread. The app's excluded apps live in the engine `Config` only.
- UI Automation (and MSAA) queries switch on Chromium's accessibility tree, costing CPU and memory in Chrome and Electron.
  The adapter asks UIA only on focus changes, clicks, Tab and Enter, for every class except the standard edits (Edit, RichEdit*: ES_PASSWORD alone);
  unknown classes fail closed (secure until UIA says otherwise). Tests only create the UIA client and never query an app.
- Perf gate (`docs/performance.md`, limits in `perf/limits.toml` with measured values in comments): core bench
  `cargo run --release -p unflub-core --example bench -- --check perf/limits.toml [--json]`, Windows idle footprint
  `pwsh perf/idle.ps1`, low-end sim `pwsh perf/lowend.ps1` (Job Object via `examples/jobrun.rs`). The scripts run
  `unflub --dry-run --pill off`: hook and engine live, but every action is vetoed (`policy::DRY_RUN`), so nothing is
  injected or swallowed. Safe on a machine someone is typing on; they stop only processes they started.
- Idle measured 2026-11-16 (dry run, pill off): private 18.8 MB, working set 38-40 MB, ~0.03 s CPU per 20 s, 12
  threads dropping to 9 after ~1 min (Windows thread-pool workers from DLL loading retire), ~210 handles.
- A Job Object hard CPU cap is a duty cycle: at 25 % of one core the job runs ~150 ms then pauses ~430 ms. Work that
  fits in one burst (index build) runs at full speed; it doesn't simulate a slower clock.
- Windows PowerShell 5.1: `$PSScriptRoot` can be empty in `param()` defaults, and a native command's stderr throws
  under `$ErrorActionPreference = 'Stop'`. The perf scripts handle both.
- Site (`site/`): Astro static, hosted on Vercel through its Git integration (Root Directory `site`); headers, CSP,
  trailing slashes and caching live in `site/vercel.json`, which `scripts/serve.mjs` and the tests also read. CI only
  checks, builds and tests. Performance figures live in `site/src/data/performance.ts`; keep them in step with
  docs/performance.md. Scripts must stay external files (the CSP has no 'unsafe-inline' for scripts; a test enforces it).
- Site privacy copy (owner, 2026-11-17): "Nothing you type leaves your computer." The update check sends app version,
  Windows version + architecture and anonymous first-of-day/week/month flags, no ID. Anonymous usage stats and crash
  reports are on by default with a first-run choice. Typo pairs are not collected. The app's Stats page is local only.
  Details live on /privacy; the home section stays one short statement. A test rejects the old ID/typo-pair wording.
  Sponsor/star links come from `SITE.sponsor` and `SITE.repo` in `site/src/data/site.ts`.
- Site design (owner, 2026-11-17): white day page, studio-lit capsule and grey cards, espresso pill nav; the home page
  sinks to near-black for `.story` (Everywhere, Undo, Speed, Privacy) via a fixed `.dusk` layer on a `--story` view
  timeline, and the nav pill inverts with it. `.story` also paints its own night so text never sits on white. Size type
  and scenes by width and height (`min(vw, vh)` in clamps); check 1280x720 and 1280x800, not just 1440x900.
- Launch plan and post drafts live in `docs/launch/` (2026-11-17). Before turning on Cloudflare Web Analytics, the
  CSP in `site/public/_headers` must allow `https://static.cloudflareinsights.com` and the CSP test in
  `site/tests/site.test.mjs` must accept it. The download API only records `ref` values `site`, `github`, `readme`,
  `other` (`D:\Code\rmgn-api\src\products\unflub.ts`); channel refs like `hn` count as `other` until added.
  Reddit: r/windows, r/productivity and r/dyslexia forbid promotion; r/Windows11 and r/rust allow weekly threads only.
  Launch captures go in git-ignored `site/.local/launch/`.
- Downloads (site): `PUBLIC_TURNSTILE_SITE_KEY` (public; invisible widget for unflub.rmgn.net and localhost) turns on
  the Turnstile check in `site/src/scripts/download.ts`. Invisible mode requires the visible notice
  (`TurnstileNotice.astro`) by the download buttons and in the footer, linking Cloudflare's Turnstile Privacy Addendum.
  After the check the page fetches `?format=json` from the API (4 s timeout) and opens the returned URL (only
  dl.rmgn.net or github.com). Any failure falls back to GitHub's latest release after a one-line message. Headless Chrome fails the real key;
  test the flow with Cloudflare's test keys (1x...BB pass, 3x...FF interactive) or by hand in a normal browser.
- In Git Bash, `--flag=/path`-style args get rewritten to Windows paths (MSYS); pass names, not leading-slash paths.
