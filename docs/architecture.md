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
   we didn't see.
3. On a boundary key (space, punctuation, Enter) the engine decides whether the word is wrong
   and what it should be. If it is confident, it returns `Action::Replace`.
4. The adapter swallows the boundary key and injects backspaces plus the fixed word and the
   boundary. Because this happens inside the hook, the injected text lands before any key the
   user presses next, so fast typing can't interleave with a correction.
5. If the very next key is Backspace, the engine returns `Action::Undo`: the original word comes
   back and, by default, that word is never corrected again.

The hook budget is **1 ms per key, worst case**. Anything slower would add input lag, so the
engine never calls slow APIs on this path.

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

## Never corrected

- Password and other secure fields, and apps on the user's exclude list.
- Code-looking tokens: camelCase, snake_case, paths, URLs, anything inside backticks, words with
  digits, and all-caps words (treated as acronyms).
- The personal dictionary and words the user has undone before.
- Words shorter than the minimum length.
- The first word after a reset: the caret may be in the middle of text we never saw.
- Hyphenated words, words with non-ASCII letters, and words ending in an apostrophe.

## UI processes

- **Core process** (hook, engine, tray): always running, target under 15 MB RAM idle.
- **Pill**: drawn natively in a click-through, no-activate window, shown briefly after a fix.
  No web view stays resident.
- **Settings**: a Tauri window created on demand and destroyed when closed.

## Later layers

- Optional on-device language model to rerank candidates by sentence context, for machines that
  can afford it.
- Phone parity extras: sentence-start capitals, `i` → `I`, double-space for a period.
