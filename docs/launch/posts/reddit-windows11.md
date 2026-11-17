# r/Windows11: weekly thread only

**Verdict: weekly Software Showoff thread only. No standalone post.**

Rule 7, read 2026-11-17: "We currently are not accepting new software for review, however you may share it in
our weekly Software Showoff thread." The thread is posted by u/froggypwns as "Software Showoff Sunday". Rule 1
also says being compatible with Windows is not enough to be on topic, and rule 11 bans moving conversations to
DMs or Discord.

## Before posting

- [ ] Repo public, v0.1.0 released, `https://unflub.rmgn.net/` deployed.
- [ ] This week's Software Showoff thread is up. Search the sub for "Software Showoff Sunday".
- [ ] Rewrite anything below that doesn't sound like you.
- [ ] Answer replies in the thread, not in DMs.

## Comment

---

**unflub: phone-style autocorrect for every text box on Windows 11**

I type fast and badly. Windows 11 has an "Autocorrect misspelled words" switch, but it only works in some
apps, and the ones I use most, AI prompt boxes and chat apps, aren't among them. So I built unflub.

- Fixes a typo when you finish the word, in any app.
- Backspace right after a fix puts your word back, and it stops fixing that word.
- Skips terminals, password fields and password managers by default, and leaves code alone.
- About 0.1 ms per word, about 20 MB in the background, nothing while you're not typing.
- Nothing you type leaves the PC. It checks for updates daily, and anonymous usage counts and crash reports
  are on unless you turn them off on first run.

Free, MIT licensed. English only for now. The installer isn't signed yet, so SmartScreen will warn you.

Download: https://unflub.rmgn.net or `winget install RMGN.unflub`
Code: https://github.com/ReimagineMedias/unflub

If it fixes something it shouldn't, I'd like to hear which app and which word.

---
