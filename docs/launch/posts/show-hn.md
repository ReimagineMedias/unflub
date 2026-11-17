# Show HN

## Before posting

- [ ] Repo `github.com/ReimagineMedias/unflub` is public, with the rewritten README.
- [ ] v0.1.0 is released and rolled out to 100%. `https://api.rmgn.ca/unflub/download/windows?ref=hn`
      returns the setup.
- [ ] `https://unflub.rmgn.ca/` is deployed.
- [ ] `winget install RMGN.unflub` works. If it does not yet, delete the winget line from the comment.
- [ ] The sentence starting "The part that reads your keys" is true for v0.1: a separate process with no
      network code, and a CI check that fails if one is added. If not, delete that sentence.
- [ ] The numbers still match `docs/performance.md`.
- [ ] You have the next 6 to 8 hours free to answer comments.

Rules that matter, from https://news.ycombinator.com/showhn.html and the guidelines: it must be something
people can try now, no signup; you must be around to discuss it; never ask anyone to upvote or comment, and
do not share the HN link with a request to vote; no uppercase or exclamation marks in the title; crop
"gratuitous numbers" from titles. If it sinks without comments, HN allows a small number of reposts later,
but do not delete and repost.

## Link

Submit the GitHub repo, `https://github.com/ReimagineMedias/unflub`. Handy's 247-point post linked its repo,
HN readers trust a repo more than a landing page, and the README has the download, winget line and privacy
details.

## Title options, all under 80 characters

1. `Show HN: Unflub, phone-style autocorrect for every text box on Windows` (70)
2. `Show HN: Unflub, offline autocorrect for Windows that leaves your code alone` (76)
3. `Show HN: I wanted phone autocorrect in AI prompt boxes, so I made it for Windows` (80)

I would use 1. It says what it is. Option 3 tells the story better but uses the full 80 characters.

## First comment

Post this right after submitting.

---

Hi HN, I'm Matias, in Ottawa. I type fast and I make a lot of typos. On my phone they fix themselves. On my
laptop, every AI prompt box, chat app and terminal-adjacent text field gave me a red underline at best, and
mostly nothing. I wanted the phone behaviour in all of them, so I built unflub. You flubbed. We unflubbed.

What it does: when you finish a word with a space or punctuation, unflub decides whether it was a typo and,
if it is confident, swaps in the right word before your next key lands. Press Backspace right after a fix
and your word comes back, and unflub stops fixing that word.

How it works:

- A low-level keyboard hook feeds each key to an engine written in Rust. The decision happens inside the
  hook, so a fast typist can't interleave keys with a correction. The budget is 1 ms per key, worst case.
- Detection uses the Windows spell checker's `Check` call, which takes about 0.04 ms. Its `Suggest` call
  takes 5 to 100 ms, far too slow, so candidates come from a bundled SymSpell-style index: words within two
  edits, ranked by word frequency, keyboard distance, swapped letters and the previous word.
- It only fixes when the best candidate clearly beats the runner-up. Ambiguous words are left alone. There
  are three modes, Careful, Balanced and Phone-like.
- On my laptop the word decision is about 0.09 ms at p99. Idle, it uses about 19 MB of private memory
  and close to zero CPU. CI runs the same benchmark in a job object capped at a quarter of one core and
  128 MB, and fails the build if the limits are crossed. Numbers and method: docs/performance.md.
- It leaves code alone: camelCase, snake_case, paths, URLs and anything in backticks. Terminals, password
  fields and password managers are skipped by default.
- The settings window is Tauri and only exists while it is open. The correction pill is drawn natively.

Privacy, since a keyboard hook should make you ask: nothing you type leaves your computer. The part that reads
your keys runs as its own process with no network code, and CI fails if anyone adds some. The app does send
a daily update check with the app and Windows version and no ID. Anonymous usage counts and crash reports are
on by default; the first-run tour asks, and one switch turns them off. The local Stats page with your top
typos never leaves the machine. The code is MIT, so you can check all of this.

Limits, honestly:

- Windows only for now. macOS and Linux are next, on the same engine.
- English only.
- It looks at one word and the word before it. No grammar, no sentence-level context.
- It can't see keys typed into administrator windows, and it skips CJK input methods.
- The installer isn't code-signed yet, so SmartScreen will warn you. I'm applying to SignPath's free
  program now that the repo is public.

Download: https://api.rmgn.ca/unflub/download/windows?ref=hn or `winget install RMGN.unflub`.
Site: https://unflub.rmgn.ca

I'd like to hear where it fixed something it shouldn't have, and which apps it misbehaves in.

---

## Replies to have ready

- **"Why is telemetry on by default?"** Say what it is: daily counts of corrections and undos, p99
  latencies, crash and hook-reinstall counts, and which settings are on, with no ID and no text. It tells me whether the undo rate
  means the defaults are wrong. The tour asks on first run, the switch is in Settings > Privacy, and
  `HKLM\Software\Policies\RMGN\unflub` turns it off for a whole organisation. Do not argue; if people want
  it off by default, take that as feedback.
- **"Isn't this a keylogger?"** It is a keyboard hook, like every tool of this kind. Point to the code,
  the network table in `docs/architecture.md`, and the separate-process check if it is in place.
- **"Why not just use X?"** Name what X does better. Grammarly checks grammar and tone, AutoHotkey's
  AutoCorrect script is a fixed list you can edit, tinySpell checks spelling in apps that lack it.
  unflub's case is fixing as you type, offline, free, with undo.
