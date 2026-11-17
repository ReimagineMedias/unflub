# unflub

System-wide, phone-style autocorrect. RMGN product, site at unflub.rmgn.net (Cloudflare). All app/CI domains are rmgn.net since 2026-11-17 (bundle id stays `ca.rmgn.unflub`).
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
  `cargo test --release -p unflub-core --test corpus -- --nocapture` (owner corpus 23/25, common typos 104/111 since
  2026-11-17), `cargo run --release -p unflub-core --example bench` and the quality gate
  `cargo run --release -p unflub-core --example quality -- --check perf/limits.toml` (7.9 false fixes per 10k clean
  words, 85.86 % exact, 1.72 % wrong; `--dump <dir>` lists every false fix). Its corpora in `data/quality/` are LF
  with `-text` in .gitattributes: the lab's CRLF copies never split into paragraphs.
- Pill (`overlay.rs`, `caret.rs`): `cargo run -p unflub-windows --example pill_snapshot` renders every style offscreen to
  `.local/pill/*.png` without showing a window; check visuals there, never by running the hook. Fonts resolve to
  Segoe UI Variable Text and Cascadia Mono on Windows 11. Software Direct2D renderer setup is ~2 ms warm (26 ms on
  first DLL load), so it is dropped whenever the pill hides.
- No caret position (pill skipped): terminals, games, canvas editors, and browser/XAML fields whose UIA provider has no
  TextPattern. A Win32 caret gives only the line; the word width is estimated (0.37 x line height per letter).
- Windows `ComprehensiveCheck` measured 2026-11-16 (Windows 11 26300, en-US): ~3 ms per typo, and its auto-replace list
  has `recieve`->`receive`, `becuase`->`because` but nothing for `teh`. `CORRECTIVE_ACTION_REPLACE` is rare; don't rely on it.
- `unflub-windows` tests call the real `ISpellChecker` but never install hooks or inject input. Try the hook live with
  `cargo run --release --bin unflub-cli -- --demo` (rules teh/adn/recieve). Never run it in automation on a machine someone
  is typing on, except with `--dry-run`.
- Windows `ISpellChecker::Check` reports any word of 64+ UTF-16 units as correct.
- Two processes since 2026-11-17: `unflub-engine.exe` (`crates/unflub-engine`: hook, engine, spell checker, pill; no
  network code) and `unflub.exe` (tray, settings, tour, updater, remote config, stats, crash reports). The app spawns
  the engine beside itself (`app/src-tauri/src/engine.rs`, kill-on-close Job Object, restart backoff 1 s doubling to
  5 min), over a per-user named pipe (DACL = user SID, `PIPE_REJECT_REMOTE_CLIENTS`, first instance, client pid
  checked) with 4-byte-length JSON frames and a `Hello`/`Welcome` protocol version. Pause ends the engine process.
  `npm run tauri dev` still installs a real keyboard hook (through the engine): never in automation. Test IPC with
  `unflub-engine --test-ipc` (never hooks; `crates/unflub-engine/tests/ipc.rs`) or the in-process pipe pair tests.
- No-network guard: `cargo test -p unflub-engine --test no_network -- --nocapture` (`cargo tree -p unflub-engine -e
  normal --target all`, fails on reqwest/hyper/h2/ureq/isahc/curl/surf/sentry/tauri/rustls/native-tls/openssl
  families and on `Win32_Networking*` or winapi socket features). Verified it fails when reqwest or
  `Win32_Networking_WinSock` is added. The app's `windows` features don't leak into the engine's tree.
- Engine sidecar packaging: `bundle.externalBin = ["binaries/unflub-engine"]`; `beforeBuildCommand` runs
  `scripts/build-engine.mjs` (honours `TAURI_ENV_TARGET_TRIPLE`, copies to `app/src-tauri/binaries/`, git-ignored).
  `app/src-tauri/build.rs` removes externalBin through `TAURI_CONFIG` unless it is a release build under the Tauri CLI
  (`TAURI_ENV_PLATFORM` set), because tauri-build copies the sidecar into `target/<profile>/unflub-engine.exe`, the
  same path Cargo builds the engine to, and errors if it is missing. Plain cargo builds, clippy and tests need nothing
  in `binaries/`.
- Undo (since 2026-11-17): double Backspace by default (`undoMode` twice/once/off, settings schema v2), the first
  Backspace is held by the hook (`Action::HoldBackspace`, pill "Undo?" in the alert red) and released before any
  other key-down through the replay pipeline (no timeout; the pill stays until answered), dropped on click/focus change, sent on pause/quit. Learning after
  `learnAfterUndos` undos (default 2), counts in `undo-counts.json` only with Stats "Keep a list" on, none while learning is off; Clear history clears them (engine too, via `undo_counts_epoch`). Never call `ISpellChecker::Add` (shared Windows
  dictionary); `spell.rs` has a test that scans the workspace for it.
- Engine panics: the engine's hook writes `engine-crash.json` (Sentry event JSON: `symbolic` debug images use the
  field names `name` and `id`, not `code_file`/`debug_id`) and ends the process with `TerminateProcess` (exit 101);
  `abort()` started Windows Error Reporting and took seconds. The default panic hook is not called: it prints the
  message, which may hold typed text.
- UI Automation (and MSAA) queries switch on Chromium's accessibility tree, costing CPU and memory in Chrome and Electron.
  The adapter asks UIA only on focus changes, clicks, Tab and Enter, for every class except the standard edits (Edit, RichEdit*: ES_PASSWORD alone);
  unknown classes fail closed (secure until UIA says otherwise). Tests only create the UIA client and never query an app.
- Perf gate (`docs/performance.md`, limits in `perf/limits.toml` with measured values in comments): core bench
  `cargo run --release -p unflub-core --example bench -- --check perf/limits.toml [--json]`, Windows idle footprint
  `pwsh perf/idle.ps1`, low-end sim `pwsh perf/lowend.ps1` (Job Object via `examples/jobrun.rs`). The scripts run
  `unflub-cli --dry-run --pill off`: hook and engine live, but every action is vetoed (`policy::DRY_RUN`), so nothing is
  injected or swallowed. Safe on a machine someone is typing on; they stop only processes they started.
- Idle of the pair measured 2026-11-17 (`pwsh perf/idle.ps1`, `unflub.exe --perf-idle <dir>`: temp settings, no
  network, no single instance, dry-run engine; a second tray icon shows for 25 s): engine 19.5-19.8 MB private, app
  4.3-4.4 MB, both 23.8-24.0 MB; CPU 0-0.047 s per 20 s; app has 40 threads (async runtime, one per core).
- Idle of `unflub-cli` measured 2026-11-16 (dry run, pill off): private 18.8 MB, working set 38-40 MB, ~0.03 s CPU per 20 s, 12
  threads dropping to 9 after ~1 min (Windows thread-pool workers from DLL loading retire), ~210 handles.
- A Job Object hard CPU cap is a duty cycle: at 25 % of one core the job runs ~150 ms then pauses ~430 ms. Work that
  fits in one burst (index build) runs at full speed; it doesn't simulate a slower clock.
- Windows PowerShell 5.1: `$PSScriptRoot` can be empty in `param()` defaults, and a native command's stderr throws
  under `$ErrorActionPreference = 'Stop'`. The perf scripts handle both.
- Releases: see `docs/releasing.md`. A `v*.*.*` tag runs `.github/workflows/release.yml`: build, sign, VirusTotal,
  attest, attach installers + `.sig` to a draft GitHub Release (installers live there; no R2), then register with
  `POST api.rmgn.net/unflub/v1/admin/releases` (`scripts/register-release.mjs`: file, signature, sha256, size; lands
  PAUSED at 0%; CI's token can only register). rmgn-api's publish step builds dl.rmgn.net (signed manifests,
  `tauri-*.json`, redirects to the GitHub assets). The owner publishes the draft
  (draft assets aren't downloadable), then rolls out (5% > 25% > 100%, pause, block, min_version) in
  `https://api.rmgn.net/dashboard/?product=unflub` (Cloudflare Access), never from CI. API contract:
  `D:\Code\rmgn-api\docs\unflub-contract.md`. `latest.json` is still generated (`scripts/updater-manifest.mjs`) but unused. `node scripts/bump-version.mjs <v>` keeps
  the five version locations in sync (CI runs `--check`). macOS/Linux matrix rows exist with `"enabled": false`.
- Updater signing keypair: `D:\Code\unflub\.local\updater\unflub-updater.key` and `.password` (git-ignored; never print
  or commit). Only the public key is in `tauri.conf.json`. Updater artifacts are on only with
  `--config src-tauri/tauri.release.conf.json`, so a plain `npm run tauri build` needs no key.
- Updates are static (`updates.rs`, `manifest.rs`, `remote.rs`): check ~2 min after launch then daily (min interval
  from the manifest's flags). Fetch `dl.rmgn.net/unflub/manifest/{channel}.json` + `.sig` (Ed25519 over the exact
  bytes, keys in `keys.rs`, ETag cached in `<data>/manifest/`); reject unsigned/invalid/>256 KB/older-`generated_at`
  and keep the last good one. Eligibility (kill switch, blocked/below-min critical, bucket < rollout) runs in the app
  with the local bucket (`net.rs`, never sent); only then the updater gets
  `dl.rmgn.net/unflub/{version}/tauri-{target}-{arch}.json`, accepts exactly that version (rollbacks work), and its
  URL/signature must equal the manifest's; installer bytes must match the manifest's sha256/size. Installer URLs:
  GitHub Release assets or dl.rmgn.net (302 to them); redirects only to that GitHub path and GitHub's asset hosts
  (`links.rs`). Update requests carry only `User-Agent`; no install id. `X-First: week,month` goes only on the daily stats report (contract v2). `keys.rs` still holds a PLACEHOLDER
  key (verifies nothing); release CI fails until the real one from rmgn-api is in. `autoUpdate` downloads + verifies in the background to `%LOCALAPPDATA%\ca.rmgn.unflub\updates`
  (SHA-256 re-checked before install); install only on Restart now or tray Quit. Only `install-source` = installer
  self-updates (`install.rs`); winget, chocolatey, scoop, store show "Update with ..." instead. Release builds only.
- Privacy/network map is in `docs/architecture.md#network-and-privacy`. Usage stats (`usage.rs`) and crash reports
  (`crash.rs`, Sentry, inert without `SENTRY_DSN` at build time) default ON with a first-run choice; tests pin the
  payload keys to the contract allowlist. Manifest flags (`remote.rs`) may only switch things off (tested). Machine
  policy `HKLM\Software\Policies\RMGN\unflub` (`managed.rs`). Typo-fix pair sharing was cut from v0.1.
- Settings have `schemaVersion` (`migrations.rs`, step by step, keeps `settings.json.v<N>`), and unknown fields survive
  in `Settings::extra`. All window strings are in `app/src/i18n/en.ts` (`t()`), tray strings in `strings.rs`. External
  links only via `<ExternalLink>` / tauri-plugin-opener (scoped to github.com and unflub.rmgn.net); a cargo test fails
  on raw `<a>` in `app/src`.
- The hook ignores unflub's own exe (HookOptions.excluded_apps), so Settings is never autocorrected; the first-run
  tour's practice box uses a separate engine over IPC (`tour.rs`). The adapter's hook watchdog (GetLastInputInfo vs
  last callback, plus resume/unlock re-arm) reinstalls dropped LL hooks (`HookEvent::HooksReinstalled`).
- Preview the settings UI in a plain browser (`cd app; npx vite`): query params `?welcome` (first run), `?page=tour`,
  `?update=ready&critical`, `?crash`, `?managed`, `?source=scoop`, `?theme=ink&mode=dark`.
- Uninstall: Tauri's NSIS template already deletes `HKCU\...\Run\unflub` (not on `/UPDATE`) and has a "Delete the
  application data" checkbox; `app/src-tauri/windows/installer-hooks.nsh` removes the StartupApproved value and
  `HKCU\Software\Reimagine Media (RMGN)\unflub` (MANUKEY follows `bundle.publisher`), and `install-source`;
  `uninstall-cleanup.wxs` does the login entry for the MSI. Start at login follows the
  registry (Run entry plus Task Manager's StartupApproved flag) at startup and when the window loads, and is never
  written back on its own. Only installed copies may write it (`install::login_allowed`: `%LOCALAPPDATA%\unflub`,
  Program Files\unflub, Scoop `apps\unflub\current`); a release build run from `target\release` registered itself
  once before this rule, so never run one with startAtLogin on outside that check.
- Binaries: the tray app's executable is `unflub.exe` (`[[bin]] name = "unflub"` in `app/src-tauri/Cargo.toml`; the
  package stays `unflub-app`); the keyboard engine is `unflub-engine.exe` (`crates/unflub-engine`); the headless
  developer runner is `unflub-cli` (`crates/unflub-windows/src/bin/unflub-cli`, whole stack in one process).
