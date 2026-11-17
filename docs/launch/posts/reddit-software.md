# r/software

**Verdict: post on a Wednesday only, with the right flair, and give readers something beyond a link.**

Rules read 2026-11-17:

- Rule 3: "Don't promote your own software unless its open source. Permanent ban. The one exception is if you
  share on Wednesdays, use the appropriate flair and provide something of value to the community rather than
  just the software." unflub is open source; the safe reading is to also follow the Wednesday and flair part.
- Rule 5: releases should be new programs or big updates, not a version log, and fully usable without paying.
  Both hold.
- Rule 6: a hidden karma threshold. If AutoModerator removes the post, modmail them rather than reposting.
- Rule 7: proper grammar and formatting.

## Before posting

- [ ] It is Wednesday.
- [ ] Repo public, v0.1.0 released, site deployed.
- [ ] Pick the release or self-promotion flair the sub offers that day.
- [ ] One post only. Later versions wait for a big update.
- [ ] Open the thread quoted below and check the quote still matches, or drop the first paragraph:
      https://www.reddit.com/r/software/comments/mvvlvv/software_that_autocorrects_your_typing/

## Title

`unflub, a free open-source app that autocorrects typos in every Windows app as you type`

## Body

---

A while back someone here asked for software "that corrects your typing on the fly while you're typing it",
not a spell checker you run afterwards. I wanted the same thing and couldn't find one I trusted with a
keyboard hook, so I built it.

**What it does.** You finish a word, and if it was a typo, it's fixed before your next key lands. Backspace
right after a fix brings your word back, and unflub won't fix that word again.

**Where it works.** Any text field on Windows: browsers, chat apps, AI prompt boxes, email. Terminals,
password fields and password managers are skipped by default. In editors it fixes comments and plain
sentences and leaves camelCase, paths and anything in backticks alone.

**What it costs your PC.** About 0.1 ms to decide on a word, about 20 MB of memory, and no CPU while you're
not typing.

**Privacy.** Nothing you type leaves the computer. It checks for updates once a day with no ID attached.
Anonymous usage counts and crash reports are on by default; the first-run tour asks, and one switch turns
them off.

**Limits.** Windows only and English only for now. The installer isn't signed yet, so SmartScreen will show a
warning.

Free, MIT. Download: https://unflub.rmgn.net, or `winget install RMGN.unflub`. Code:
https://github.com/ReimagineMedias/unflub

---
