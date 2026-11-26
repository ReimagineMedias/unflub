# Architecture

unflub fixes typos as you type, in any text field on the desktop. This page explains how.

**The part of unflub that sees your keys has no network code. CI enforces it.** unflub runs as two processes: `unflub-engine.exe`
holds the keyboard hook, the engine and the pill and has no network code at all, and
`unflub.exe` (the tray app) does everything that talks to the internet and never sees a key.
See [Two processes](#two-processes).

## Layout

```
crates/unflub-core      Platform-free engine. No OS calls, no unsafe. Tested on every OS in CI.
crates/unflub-windows   Windows adapter: keyboard hook, text injection, Windows spell checker.
crates/unflub-engine    unflub-engine.exe (hook, engine, pill) and the pipe protocol. No network code.
app/                    Tauri 2 app, unflub.exe: tray, settings, tour, updates, stats, crash reports.
                        Starts and supervises unflub-engine.exe.
site/                   unflub.rmgn.net (Astro, static)
```

macOS and Linux get their own adapter crates (`unflub-macos`, `unflub-linux`). The core and the
app stay the same.

## How a correction happens

1. The adapter's low-level keyboard hook sees each key and turns it into a `Key`
   (`Char`, `Boundary`, `Backspace`, `Reset`).
2. It calls `Engine::on_key` synchronously, inside the hook. The engine tracks the word being
   typed. Caret moves, clicks, focus changes and shortcuts reset it, so we never guess at text
   we didn't see. After a reset the first word is not corrected, because the caret may be in
   the middle of a word we never saw; correction resumes after the next boundary key. The
   exception is a caret known to be at a word start: after a click or focus change the UIA
   worker (never the hook thread, with its short timeouts) reads the character before the
   caret through `TextPattern`, and if the field is empty or that character is whitespace or an
   opening bracket or quote, the hook passes `caret_at_word_start(generation)` to the engine
   before the next key. It counts only for the reset it was asked about and only until the
   first word's boundary; a late, stale or unknown answer keeps the default. So the first word
   in an empty prompt box (`brb` in a chat or an AI agent's input) is fixed too. Ctrl+Backspace
   (`Key::DeleteWordLeft`) is the other exception: apps differ in exactly what it deletes, but
   the caret ends at a word start, so the next word is fully seen. Ctrl+Delete stays a reset.
3. On a boundary key (space, punctuation, Enter) the engine decides whether the word is wrong
   and what it should be. If it is confident, it returns `Action::Replace`.
4. The adapter swallows the boundary key and hands backspaces plus the fixed word and the
   boundary to its injector thread, which sends them with `SendInput`. The hook never sends
   input itself: injected events pass through every low-level hook on the system, ours
   included, so sending from inside the hook stalled typing for tens of milliseconds. Keys the
   user presses before the correction has landed are held back and replayed right after it,
   in order, so fast typing can't interleave with a correction. Mouse clicks aren't held back:
   a click in the few milliseconds before a correction lands moves the caret first.
5. Undo is a double Backspace by default (setting "Undo a fix": Backspace twice, once, or off).
   People press Backspace right after a fix all the time to delete the space before a comma,
   so one Backspace alone never undoes:

   | After a fix, while its pill shows (pill duration) | What happens |
   |---|---|
   | Backspace | Held: nothing is deleted, the pill turns red and asks "Undo?" (`Action::HoldBackspace`), and stays up until answered |
   | then Backspace again | The fix is undone: your word and the boundary come back, the pill says "Undone" and fades as usual |
   | then any other key (comma, letter, arrow, Shift, Enter, Ctrl+Backspace) | The held Backspace is sent first, then that key, in order (`,` after `the ` gives `the,`); the pill fades out at once |
   | then a click or a focus change | The held Backspace is dropped; the pill fades out |
   | then pause, quit or the engine stopping | The held Backspace is sent after the hooks are gone, so the keypress isn't lost |
   | Backspace after the pill's time | A plain Backspace |

   The hold has no timeout (owner testing: a red pill that vanished on its own, deleting a
   space a second later, felt wrong), but it never holds other input back: the next key-down,
   whatever it is, releases it first. A click is the exception: it has already moved the
   caret, so sending the Backspace then would delete wherever the click landed (holding the
   click back and replaying it would mean re-injecting mouse input).
   With "Backspace once" the first Backspace right after a fix undoes it, as before; with "Off"
   Backspace is always just Backspace.

   A word stops being fixed only after its fix was undone `learnAfterUndos` times (default 2,
   1 to 5, 0 never), counted per typo and fix (only while learning is on, and not in terminals);
   the counts are written to `undo-counts.json` beside the settings only while the Stats page's
   "Keep a list" is on, and Clear history clears them (in the engine too); then it
   goes to the personal dictionary. unflub never writes to the Windows dictionary
   (`ISpellChecker::Add`): that list is shared by every app, so a typo undone once would stop
   being underlined everywhere (a test fails if any source calls it).

   Never after a fix ended by Enter or Tab: undoing would type Enter or Tab again (a second chat
   message, the next spreadsheet cell), so that Backspace is just a Backspace and the pill shows
   no undo hint.

The hook budget is **1 ms per key, worst case**. Anything slower would add input lag, so the
engine never calls slow APIs on this path.

## The Windows adapter

`crates/unflub-windows` runs these threads. All but the injector and the overlay are in the
COM multithreaded apartment:

| Thread | Does |
|---|---|
| `unflub-hook` | `WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks, focus WinEvents, calls the engine, holds and replays keys |
| `unflub-inject` | `SendInput` for corrections and replayed keys, in hand-over order |
| `unflub-uia` | Asks UI Automation whether the focused field is a password field |
| `unflub-spell` | Slow `ComprehensiveCheck` lookups for the OS auto-replace cache |
| `unflub-overlay` | The correction pill window: finds the caret, draws the pill, runs its fades |
| `unflub-caret` | Asks UI Automation where the caret is in browser and XAML fields, for the pill |

- **Key translation.** `ToUnicodeEx` with flag `0x4` (Windows 10 1607+) reads what a key types
  in the foreground window's layout without changing the kernel's dead-key state, so accents
  keep working in the user's app. We can't see what a dead key composes with, so the key after
  a dead key resets the word instead of guessing.
- **Injection.** One `SendInput` per action, on the injector thread: for a fix, the boundary
  key first (it replaces a suggestion that inline autocomplete selected after the word, as in
  Excel or the address bar), then backspaces over the word and that boundary, then the text;
  after Enter or Tab, which would send or commit, just the backspaces and the text. Text goes
  as Unicode packets (`\n` and `\t` as real Enter and Tab), tagged with a batch number so our
  hook can tell when the whole batch has landed. Until then the user's keys (downs and ups,
  modifiers too) are held in a queue of up to 64 events, then replayed with a replay tag and
  handled as normal keys. A full queue or a failed send replays what is held; a send that
  typed nothing gives the triggering key back. If our own events haven't all come back after
  500 ms (Windows dropped our hook mid-batch, or another program's low-level hook eats
  injected input), we stop waiting: what is held is replayed once and unflub **pauses
  itself**. Keys then pass straight through, nothing is held or corrected, and the tray and
  settings window say "unflub paused itself: another app is interfering with keyboard input".
  It probes with one tagged key-up of an unassigned key (which our hook swallows) after 30 s,
  doubling to 30 minutes while it keeps happening, and right after the hooks are reinstalled;
  when the probe comes back it resumes. No key is ever held longer than one timeout. Input injected by
  other programs (dictation tools, macro tools) resets the word. We don't inject, and let the
  key through instead, when Ctrl, Alt or Win is held (Ctrl+Backspace would delete a word), when
  the key is an auto-repeat (swallowing it would leave a key stuck down), or on Shift+Enter (a
  soft line break we'd re-send as a plain Enter). If `SendInput` takes fewer events than we
  sent, the key goes through, the word resets, and any key left down is released.
- **Secure fields.** While a field is secure, the engine never sees its keys: it gets one
  `Reset` instead, and the rest of a word that was half typed when the field became safe is
  skipped too. The focused window's class decides how we check:
  - Standard edit controls (exactly `Edit`, `RichEdit20A/W`, `RICHEDIT50W`, `RichEditD2D`,
    `RichEditD2DPT`): the `ES_PASSWORD` style alone, read on the hook thread. Other classes
    containing "edit" (WinForms, Delphi `TEdit`) are secure at once with `ES_PASSWORD`, and
    otherwise checked like every other class below.
  - Every other class, known or not (browsers, Electron, WPF, UWP, Qt, Java): UI Automation's
    `IsPassword`, asked on focus changes and 50 ms after a click, Tab or Enter. UIA can block
    on hung apps, so it runs on its own thread with 250 ms / 500 ms connection and transaction
    timeouts. The field counts as secure until UIA says it isn't: no answer yet, a UIA error,
    a timeout, or no UIA at all all mean no corrections there.

  Any accessibility query, UIA or MSAA, makes Chromium build its accessibility tree, which
  costs CPU and memory in Chrome and Electron apps. That is why we only ask when focus may
  have moved; a burst of focus events costs one query.
- **Other skips.** Keyboard layouts for Chinese, Japanese and Korean (IME input) and apps on
  the exclude list are treated like secure fields. So are, whatever the exclude list says
  (`policy::never_correct`): virtual machine consoles and VNC viewers (VMware, VirtualBox,
  Hyper-V's VMConnect, QEMU), which forward scan codes and drop our Unicode text, so a fix
  would delete the word and type nothing; terminals and SSH clients beyond the default exclude
  list (PuTTY, KiTTY, MobaXterm, Tabby, Hyper, ConEmu, Termius, Warp and others), where a
  password prompt can't be detected; and xterm.js terminals inside other apps (VS Code's
  terminal), found by their UIA class `xterm-helper-textarea`. A fix made in any terminal,
  including the default-excluded consoles a user takes off the list, never records its words
  on the local Stats page (`policy::keep_words`).
- **Spell checker.** `Check` runs on the hook thread (it is fast). The OS auto-replace lookup
  only reads a cache; misses are looked up in the background and used next time. Learned words
  never go to the Windows dictionary (no `ISpellChecker::Add`, a test enforces it): they are
  unflub's own personal words in its settings (see "Undo teaches" below).
- **Elevated windows.** A normal process can't see keys typed into administrator windows or
  inject into them, so unflub does nothing there.
- **Hook watchdog.** Windows can drop a low-level hook without a word (a callback that took too
  long, or sometimes across sleep or a locked session). Every 5 s the hook thread compares
  `GetLastInputInfo` with the time of its last keyboard or mouse callback; input more than 2 s
  newer than any callback means the hooks are gone, and they are installed again (then at most
  every 30 s, doubling to 10 min while it keeps happening, since touch input or a secure
  desktop can look the same). The pause resets only after a real callback arrives following a
  reinstall; a quiet spell proves nothing. A message-only window also re-arms them once per
  resume from sleep (Windows sends two resume notifications; `ResumeGate` keeps one) and on
  session unlock. Each reinstall is a `HookEvent::HooksReinstalled` with its reason, counted in
  the usage stats as `health.hook_reinstalls` and `hook_reinstalls_{watchdog,resume,unlock}`;
  an engine start that can't hook at all is `health.hook_unavailable`.

### The pill

After a fix or an undo the adapter shows a small overlay at the caret for about a second
(`overlay.rs`, `caret.rs`). Styles: underline plus capsule (default), capsule, underline, off.

- **No cost to typing.** The hook thread does nothing extra: whoever reads `HookEvent`s (or
  `Engine::drain_events`) calls `OverlayHandle::show`, which pushes onto a channel and posts a
  window message.
- **Finding the caret.** 16 ms after the fix (so the app has drawn the new text), the overlay
  reads the foreground thread's system caret (`GetGUIThreadInfo`), mapped to physical pixels
  through the caret window's DPI context. If there is none and the focused window is a
  browser, Electron, WPF or XAML host, it asks UI Automation on `unflub-caret` for the caret
  range moved back over the word, which gives the word's exact bounds. With no answer within
  64 ms the pill is skipped; we never guess. A system caret gives only the line, so the word's
  width is estimated from the line height.
- **The window.** A layered, click-through, no-activate, topmost tool window: never focused,
  never in Alt+Tab or the taskbar. Each pill is drawn once with software Direct2D and
  DirectWrite into a premultiplied DIB for `UpdateLayeredWindow`; fade frames only move the
  window and change its alpha. Hidden, it holds no timer, bitmap or Direct2D object.
- **Placement.** Above the line by default, flipped below when there is no room, clamped to the
  monitor's work area, and scaled by that monitor's DPI.

### Two processes

The part of unflub that sees your keys has no network code. CI enforces it.

| Process | Holds | Network |
|---|---|---|
| `unflub-engine.exe` (`crates/unflub-engine`) | keyboard and mouse hooks, the engine, the Windows spell checker, the pill | none: no HTTP, TLS, Sentry or Tauri crate, no `Win32_Networking` feature |
| `unflub.exe` (`app/src-tauri`) | tray, settings window, tour, updater, remote config, usage stats, crash reports, the local Stats page | dl.rmgn.net (signed manifest, release files), GitHub Releases (installers), api.rmgn.net (usage stats), Sentry (crash reports, builds with a DSN), see [Network and privacy](#network-and-privacy) |

**The guard.** `crates/unflub-engine/tests/no_network.rs` runs `cargo tree -p unflub-engine
-e normal --target all` and fails on any crate in the reqwest, hyper, h2, ureq, isahc, curl,
surf, sentry, tauri, rustls, tokio-rustls, native-tls or openssl families (and a few more), and
on any `Win32_Networking*` feature of `windows` (or a socket or HTTP module of `winapi`). CI
runs it on every OS as its own step, "No network code in the engine".

**The pipe.** The app creates `\\.\pipe\unflub-engine-<pid>-<64 random bits>` as the only
instance of that name (`FILE_FLAG_FIRST_PIPE_INSTANCE`), with a protected DACL that grants
access to the current user's SID alone, and `PIPE_REJECT_REMOTE_CLIENTS`. It starts the engine
with that name and its own pid, then accepts one client and drops it unless
`GetNamedPipeClientProcessId` is the pid it spawned; the engine checks
`GetNamedPipeServerProcessId` the same way. I/O is overlapped, so a pending read never blocks a
write on the same handle.

**Messages** (`crates/unflub-engine/src/protocol.rs`) are a 4-byte little-endian length and a
JSON body, at most 4 MB. JSON rather than bincode: messages are rare, so speed doesn't matter,
while a self-describing format lets either side skip a message it doesn't know and keeps
defaulted fields compatible across versions (bincode isn't self-describing, and the project
stopped in 2025). The engine opens with `Hello { protocol, version }`; the app answers
`Welcome` only if the protocol matches, else ends it. Then:

| Direction | Messages |
|---|---|
| app to engine | `Configure` (core `Config` plus pill settings, remote flags already applied), `Shutdown` |
| engine to app | `Ready { hooked }`, `Corrected`, `Undone` (with the words, for the local Stats page), `WordsChecked` every 10 s when it moved, `Learned`, `HookReinstalled`, `CorrectorPanicked` |

**Supervision** (`app/src-tauri/src/engine.rs`). The app starts the engine beside `unflub.exe`
in a Job Object with kill-on-close, so the engine never outlives the app (crash, Task Manager,
the installer closing `unflub.exe`); the engine also exits when its pipe closes. If it exits
unasked, the app counts a crash and starts it again after 1 s, doubling to at most 5 minutes,
and back to 1 s once a run lasted a minute.

- **Pause ends the engine process**; resume starts a new one (~0.3 s, mostly the dictionary
  index, which an in-process resume rebuilt too). Unhooking inside a live engine would keep
  ~20 MB and a process that could still see keys. While paused nothing holds a hook or a typed
  character.
- **Quit and updates** send `Shutdown`, wait up to 3 s, then end it. The updater does this
  before the installer runs, so `unflub-engine.exe` is never locked.
- **Settings** go over the pipe as one `Configure`; the engine applies them live. Engine
  settings reach the hook thread through `LiveEngine` (`live.rs`): the hook thread applies the
  newest queued `Config` at the start of the next key (one atomic read when nothing is queued).
  Excluded apps travel in that `Config`. The hook's own exclude list holds only `unflub.exe`, so
  nothing typed in unflub's own windows is ever "fixed". A language change restarts the hook
  with a spell checker for the new language (or the user's UI language if Windows has none).
- **The pill** is drawn by the engine. The OS dark mode and accent are read again before each
  pill, so "match system" follows the OS without a message.
- **Undo teaches**: the engine sends `Learned`; the app adds the word to the personal words in
  settings, saves them, and sends the new settings back.
- **Crashes**: the engine's panic hook writes `engine-crash.json` in the app data folder (the
  panic location, frame addresses, loaded modules and thread name; never the message, and the
  home folder cut out) and ends the process at once, without writing to stderr. The app turns it
  into the same opt-in crash report as its own (`crash.rs`) and restarts the engine. Release
  builds use `panic = "abort"`, so a panic in the corrector inside the hook ends the engine the
  same way; the hook's `catch_unwind` (keys pass through, `CorrectorPanicked`) only applies
  where panics unwind, in debug builds and tests.
- **The tour** runs its own engine inside the app (`tour.rs`), with no hook: the web view sends
  each key typed in the practice box over Tauri IPC and draws the result itself.

`unflub.exe --perf-idle <dir>` runs the pair for `perf/idle.ps1`: settings and data in `<dir>`,
every network feature off, no settings window, and the engine with `--dry-run`.
`unflub-engine.exe --test-ipc` does the handshake and messages without ever installing a hook
(`tests/ipc.rs`).

The `unflub-cli` runner stays as a developer tool with the whole stack in one process.

Try the hook before the engine lands: `cargo run --release --bin unflub-cli -- --demo` fixes
`teh`, `adn` and `recieve`, shows the pill (`--pill <style>` to change it), and prints each fix
and the hook latency on exit. `--dry-run` installs the hook and runs the engine but never injects
input or swallows a key; the perf checks use it ([performance.md](performance.md)).
`cargo run -p unflub-windows --example pill_snapshot` renders each pill style offscreen to
`.local/pill/*.png`.

## Deciding what's wrong and what's right

We measured the Windows spell checker (`ISpellChecker`, the engine that draws red underlines in
Chromium and Electron apps) on real typos:

| Call | Time | Use |
|---|---|---|
| Check a correct word | ~0.04 ms | On the hook path: "is this word wrong?" |
| Suggest fixes for a typo | 5–100 ms | Too slow for the hook. Not used there. |

Its top suggestion was right for 20 of 23 real typos, but it ranks without context
(`Couls` → "Cools" when the sentence says "I Couls"). So the engine combines two sources:

- **Detection:** the OS checker (`SpellBackend::is_correct`) and/or the bundled dictionary.
- **Candidates:** a bundled SymSpell-style index over a word frequency list, which finds
  candidates within edit distance 2 in microseconds. Ranking weighs word frequency, keyboard
  adjacency (a slip to a neighbouring key is far more likely than a random letter), transposed
  letters, doubled or dropped letters, and the previous word (bigram frequency).
- **Confidence:** a correction only fires when the best candidate clearly beats the runner-up,
  scaled by the user's confidence setting. Ambiguous words are left alone. The runner-up can
  also be "leave it": an unknown word may be a name or jargon. That hypothesis is stronger for
  short words and for capitalised words mid-sentence, and weaker for long words.

Engine modes (user setting): `Both` (default: OS flags it and the bundled ranking is confident),
`SystemOnly`, `BundledOnly` (identical on every OS), `Balanced` (like `Both`, plus the OS
checker's own auto-replace fixes; it never fixes a word the OS checker calls correct, and
without an OS checker the bundled dictionary decides), `Aggressive` (phone-like: flagged as in
`Balanced`, low margin).

`Config::language` (default `en-US`) tells the adapter which language to open the OS checker
in. The bundled dictionary is English only, so for any other language its candidates are off
and only the OS checker's own fixes apply (French `bien` must never become `been`), unless
`english_for_other_languages` is set (settings.json only for now) or the mode is `BundledOnly`.

The bundled dictionary includes about 1,000 programming terms (`data/tech_words.txt`):
keywords, standard types, tools and jargon. People write prose about code all day, so these
count as known words and are never corrected, even in modes where the OS checker flags them
(as for the other lists unflub adds: everyday words and regional spellings).

## Never corrected

- Password and other secure fields, and apps on the user's exclude list.
- Code-looking tokens: camelCase, snake_case, paths, URLs, anything inside backticks, words with
  digits, and all-caps words (treated as acronyms).
- The personal dictionary and words the user has undone before.
- The protected list (`data/protected_words.txt`): chat shorthand (`tho`, `smth`), brands
  (`lyft`), acronyms typed in lowercase (`aws`) and common first names (`theo`).
- Words shorter than the minimum length.
- The first word after a reset: the caret may be in the middle of text we never saw.
- Hyphenated words, words with non-ASCII letters, and words ending in an apostrophe.

## Processes and threads

- **Engine** (`unflub-engine.exe`: hook, engine, pill): runs while autocorrect is on. The
  bundled dictionary index alone takes about 15 MB. Idle numbers are in
  [performance.md](performance.md).
- **Pill**: drawn natively in a click-through, no-activate window on the engine's
  `unflub-overlay` thread, shown briefly after a fix. No web view.
- **Tray app** (`unflub.exe`): always running, also while paused.
- **Settings**: a Tauri window created on demand and destroyed when closed.
- **Updates**: an `unflub-update` thread that reads the signed manifest on dl.rmgn.net about two
  minutes after launch and then daily. With automatic updates on it downloads and verifies a new version in the
  background; it installs only on Restart now or quit. See
  [releasing.md](releasing.md#how-the-app-updates).
- **Stats**: an `unflub-stats` thread that saves the local Stats page and usage counts every ten
  minutes and sends the daily usage stats. Each report covers one day: the day's payload is
  frozen when it ends and retried on its own while the next day is counted.

## Network and privacy

The part of unflub that sees your keys has no network code. CI enforces it. Keys and words are handled in `unflub-engine.exe`, which
has no network code (see [Two processes](#two-processes)). Nothing typed ever leaves the
computer: words reach only the engine, the pill and, over the local pipe, the app's Stats page,
`stats.json` in the local app data folder (`%LOCALAPPDATA%`, which never roams). Updates are static downloads from dl.rmgn.net and GitHub
Releases; the only thing unflub posts is usage stats to api.rmgn.net (contract:
`D:\Code\rmgn-api\docs\unflub-contract.md`). Nothing carries an identifier.

| What | When | Carries | Off switch |
|---|---|---|---|
| Signed manifest (`dl.rmgn.net/unflub/manifest/{channel}.json` + `.sig`) | at launch, then with each update check (~2 min after launch, then daily) | `User-Agent: unflub/<version>`, `If-None-Match`. The rollout bucket (random 0 to 99, redrawn per version) stays on the computer | Policy `DisableUpdates` and `DisableRemoteConfig` together; `DisableRemoteConfig` alone ignores its flags and message |
| Release file (`dl.rmgn.net/unflub/{version}/tauri-{target}-{arch}.json`) | only when the manifest offers this copy a release | `User-Agent` | Policy `DisableUpdates` |
| Update download (GitHub Release asset, possibly through a dl.rmgn.net redirect) | when an update is found (auto) or on Restart now | nothing extra | Automatic updates, policy |
| Usage stats | once a day | schema 3: counts (fixes by trigger, engine source and app category, undos, skips, active and paused minutes), settings snapshot, decide/spell-check/inject latencies with sample counts, health counters, coarse environment buckets (install source, days since install, Windows build, keyboard language, cores, RAM, tour), feature use and the update funnel. App categories are decided in the engine; app names never leave it. No identity headers, only `X-First: week,month` on the first report of an ISO week or month (`usage.rs`, `net.rs`, allowlist test). These reports are the only active-install count | Privacy > Anonymous usage stats (on by default), policy `DisableUsageStats` |
| Crash report | next start after a crash, or after an engine crash | scrubbed Sentry event: location, stack, modules, OS and app version (`crash.rs`); for the engine, from the marker it wrote | Privacy > Crash reports (on by default), policy `DisableCrashReports`; builds without `SENTRY_DSN` |

- **The manifest is signed.** Ed25519 over the exact bytes, keys in `keys.rs`; unsigned,
  badly signed, oversized (> 256 KB) or older (`generated_at`) manifests are rejected and the
  last good one stays (`manifest.rs`, `remote.rs`). Eligibility (kill switch, blocked,
  `min_version`, rollout) runs in the app; the release's Tauri file must name the installer URL
  and signature in the manifest, and the installer must match its SHA-256 and size.
- **Remote flags only take away.** The manifest's flags (`pill_enabled`, `uia_caret`,
  `stats_enabled`, `min_check_interval_h`) can switch a feature off or make a limit stricter,
  never turn on what the user or a policy turned off. Each consumer ANDs the flag with its own
  setting (`remote.rs` tests every combination). Its optional message shows as a row in About
  and in the tray.
- **Machine policy** in `HKLM\Software\Policies\RMGN\unflub` (`managed.rs`) overrides the user's
  switch; the window shows "Managed by your organization".
- **Who updates** depends on `install-source` beside `unflub.exe` (`install.rs`): only the setup's
  copies update themselves; winget, Chocolatey, Scoop and Store copies are told to use their
  tool, and portable copies to download the new version.
- **Settings file** `settings.json` has a `schemaVersion`; `migrations.rs` upgrades older files
  step by step and keeps a `.v<N>` copy. Fields from a newer unflub are kept and written back.

## Later layers

- Optional on-device language model to rerank candidates by sentence context, for machines that
  can afford it.
- Phone parity extras: sentence-start capitals, `i` → `I`, double-space for a period.
