# Architecture

unflub fixes typos as you type, in any text field on the desktop. This page explains how.

## Layout

```
crates/unflub-core      Platform-free engine. No OS calls, no unsafe. Tested on every OS in CI.
crates/unflub-windows   Windows adapter: keyboard hook, text injection, Windows spell checker.
app/                    Tauri 2 shell: tray, settings window, pill overlay, updater, start at login. (coming)
site/                   unflub.rmgn.net (Astro, static)
```

macOS and Linux get their own adapter crates (`unflub-macos`, `unflub-linux`). The core and the
app stay the same.

## How a correction happens

1. The adapter's low-level keyboard hook sees each key and turns it into a `Key`
   (`Char`, `Boundary`, `Backspace`, `Reset`).
2. It calls `Engine::on_key` synchronously, inside the hook. The engine tracks the word being
   typed. Caret moves, clicks, focus changes and shortcuts reset it, so we never guess at text
   we didn't see. After a reset the first word is never corrected, because the caret may be in
   the middle of a word we never saw; correction resumes after the next boundary key.
3. On a boundary key (space, punctuation, Enter) the engine decides whether the word is wrong
   and what it should be. If it is confident, it returns `Action::Replace`.
4. The adapter swallows the boundary key and hands backspaces plus the fixed word and the
   boundary to its injector thread, which sends them with `SendInput`. The hook never sends
   input itself: injected events pass through every low-level hook on the system, ours
   included, so sending from inside the hook stalled typing for tens of milliseconds. Keys the
   user presses before the correction has landed are held back and replayed right after it,
   in order, so fast typing can't interleave with a correction. Mouse clicks aren't held back:
   a click in the few milliseconds before a correction lands moves the caret first.
5. If the very next key is Backspace, the engine returns `Action::Undo`: the original word comes
   back and, by default, that word is never corrected again.

The hook budget is **1 ms per key, worst case**. Anything slower would add input lag, so the
engine never calls slow APIs on this path.

## The Windows adapter

`crates/unflub-windows` runs these threads (all but the injector in the COM multithreaded
apartment):

| Thread | Does |
|---|---|
| `unflub-hook` | `WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks, focus WinEvents, calls the engine, holds and replays keys |
| `unflub-inject` | `SendInput` for corrections and replayed keys, in hand-over order |
| `unflub-uia` | Asks UI Automation whether the focused field is a password field |
| `unflub-spell` | Slow `ComprehensiveCheck` lookups for the OS auto-replace cache, and adding learned words |

- **Key translation.** `ToUnicodeEx` with flag `0x4` (Windows 10 1607+) reads what a key types
  in the foreground window's layout without changing the kernel's dead-key state, so accents
  keep working in the user's app. We can't see what a dead key composes with, so the key after
  a dead key resets the word instead of guessing.
- **Injection.** One `SendInput` per action, on the injector thread: backspaces, then the text
  as Unicode packets (`\n` and `\t` as real Enter and Tab), tagged with a batch number so our
  hook can tell when the whole batch has landed. Until then the user's keys (downs and ups,
  modifiers too) are held in a queue of up to 64 events, then replayed with a replay tag and
  handled as normal keys. A full queue, a 500 ms timeout or a failed send replays what is
  held; a send that typed nothing gives the triggering key back. Input injected by
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
  the exclude list are treated like secure fields.
- **Spell checker.** `Check` runs on the hook thread (it is fast). The OS auto-replace lookup
  only reads a cache; misses are looked up in the background and used next time. Learning a
  word writes the user's dictionary, so it also happens in the background.
- **Elevated windows.** A normal process can't see keys typed into administrator windows or
  inject into them, so unflub does nothing there.

Try the hook before the engine lands: `cargo run --release --bin unflub -- --demo` fixes
`teh`, `adn` and `recieve`, and prints each fix and the hook latency on exit.

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
`SystemOnly`, `BundledOnly` (identical on every OS), `Aggressive` (phone-like).

`Config::language` (default `en-US`) tells the adapter which language to open the OS checker
in. The bundled dictionary is English only for now and ignores it.

The bundled dictionary includes about 1,000 programming terms (`data/tech_words.txt`):
keywords, standard types, tools and jargon. People write prose about code all day, so these
count as known words and are never corrected.

## Never corrected

- Password and other secure fields, and apps on the user's exclude list.
- Code-looking tokens: camelCase, snake_case, paths, URLs, anything inside backticks, words with
  digits, and all-caps words (treated as acronyms).
- The personal dictionary and words the user has undone before.
- Words shorter than the minimum length.
- The first word after a reset: the caret may be in the middle of text we never saw.
- Hyphenated words, words with non-ASCII letters, and words ending in an apostrophe.

## UI processes

- **Core process** (hook, engine, tray): always running, target about 30 MB RAM idle. The
  bundled dictionary index alone takes about 15 MB.
- **Pill**: drawn natively in a click-through, no-activate window, shown briefly after a fix.
  No web view stays resident.
- **Settings**: a Tauri window created on demand and destroyed when closed.

## Later layers

- Optional on-device language model to rerank candidates by sentence context, for machines that
  can afford it.
- Phone parity extras: sentence-start capitals, `i` → `I`, double-space for a period.
