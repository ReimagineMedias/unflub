# r/rust: weekly thread only

**Verdict: comment in the weekly "What's everyone working on this week?" thread. No standalone post.**

Rules read 2026-11-17:

- Rule 7: "Applications who just happen to be written in Rust are not on-topic." Progress on Rust projects
  goes in the weekly thread.
- Rule 6: "No slop, whether automatically generated or not." AI-looking content may be removed. Write the
  comment in your own words; the text below is a starting point.
- Rule 2: posts must relate to Rust, with useful context in the title.

A standalone post would only fit as a real write-up about Rust itself, for example how the engine stays inside
a 1 ms keyboard-hook budget. That needs an actual article first.

## Before posting

- [ ] This week's "What's everyone working on" thread is up.
- [ ] Repo public.
- [ ] Rewritten in your own words.

## Comment

---

I've been building unflub, phone-style autocorrect for every text field on Windows. The Rust part I'm happiest
with: `unflub-core` has no OS calls and no `unsafe`, and the Windows adapter calls it synchronously from a
low-level keyboard hook, so every decision has to fit in 1 ms. Detection goes through
`ISpellChecker::Check`, about 0.04 ms, and candidates come from a SymSpell-style index built at startup in
about 100 ms. p99 per word decision is about 90 µs on my laptop, and CI fails a PR that pushes it past the
limit, including in a job object capped at a quarter of one core.

The settings window is Tauri 2 and only exists while it's open. macOS and Linux adapters are next, on the same
core.

https://github.com/ReimagineMedias/unflub

---
