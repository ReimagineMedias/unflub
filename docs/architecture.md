# Architecture

unflub fixes typos as you type, in any text field on the desktop. This page explains how.

## Layout

```
crates/unflub-core      Platform-free engine. No OS calls, no unsafe. Tested on every OS in CI.
crates/unflub-windows   Windows adapter: keyboard hook, text injection, Windows spell checker.
app/                    Tauri 2 shell: tray, settings window, pill overlay, updater, start at login. (coming)
site/                   unflub.rmgn.ca (coming)
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
4. The adapter swallows the boundary key and injects backspaces plus the fixed word and the
   boundary. Because this happens inside the hook, the injected text lands before any key the
   user presses next, so fast typing can't interleave with a correction.
5. If the very next key is Backspace, the engine returns `Action::Undo`: the original word comes
   back and, by default, that word is never corrected again.

The hook budget is **1 ms per key, worst case**. Anything slower would add input lag, so the
engine never calls slow APIs on this path.

## The Windows adapter

`crates/unflub-windows` runs three threads, all in the COM multithreaded apartment:

| Thread | Does |
|---|---|
| `unflub-hook` | `WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks, focus WinEvents, calls the engine, `SendInput` |
| `unflub-uia` | Asks UI Automation whether the newly focused field is a password field |
| `unflub-spell` | Slow `ComprehensiveCheck` lookups that fill the OS auto-replace cache |

- **Key translation.** `ToUnicodeEx` with flag `0x4` (Windows 10 1607+) reads what a key types
  in the foreground window's layout without changing the kernel's dead-key state, so accents
  keep working in the user's app. We can't see what a dead key composes with, so the key after
  a dead key resets the word instead of guessing.
- **Injection.** One `SendInput` per action: backspaces, then the text as Unicode packets
  (`
` and `	` as real Enter and Tab), all tagged so our hook skips them. Input injected by
  other programs (dictation tools, macro tools) resets the word. If Ctrl, Alt or Win is held
  when a fix is due, we don't inject (Ctrl+Backspace would delete a word); the key goes through
  and the word resets. If `SendInput` is blocked, the key goes through untouched.
- **Context.** On focus changes the hook caches the foreground exe name and checks for classic
  `ES_PASSWORD` edit boxes. UI Automation's `IsPassword` covers everything else; it can block on
  hung apps, so it runs on its own thread with short UIA timeouts, and a field counts as secure
  until its answer arrives.
- **Spell checker.** `Check` runs on the hook thread (it is fast). The OS auto-replace lookup
  only reads a cache; misses are looked up in the background and used next time.
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
