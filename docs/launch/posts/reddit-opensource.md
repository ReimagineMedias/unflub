# r/opensource

**Verdict: a direct post is allowed, with the Promotional flair. Stay and talk.**

Rules read 2026-11-17:

- Rule 2: promoting your own work is fine "to a degree"; Reddit's guide is under 10% of your posts.
- Rule 3: "All AI-generated content is low-effort and ban worthy." Rewrite this draft in your own words
  before posting. Treat it as notes, not copy.
- Rule 4: linked code must have an OSI-approved license. MIT qualifies, and the repo has `LICENSE`.
- Rule 6: drive-by posts from accounts with no intention of joining the discussion may be removed.
- Rule 8: use the **Promotional** flair when sharing a project.

## Before posting

- [ ] Repo public with `LICENSE` at the root, and a README that is accurate about what the app sends.
- [ ] v0.1.0 released.
- [ ] Some comment history in r/opensource from before this post.
- [ ] Rewritten in your own words, see rule 3.
- [ ] The separate-process sentence is true for v0.1, or cut.
- [ ] A few free hours to answer the thread.

## Title

`unflub: MIT-licensed, offline autocorrect for every text box on Windows`

## Body

---

I made a small Windows app that does what phone keyboards do: when you finish a word, it fixes the typo.
It works in any app, it runs offline, and it's MIT.

Why open source matters for this one: it's a keyboard hook, and you shouldn't run one you can't read. Nothing
you type leaves the computer. The part that reads keys runs as its own process with no network code, and CI
fails if someone adds any. The app does check for updates daily, and anonymous usage counts and crash reports
are on by default with a choice on first run. The README has the exact list of what is sent.

How it works, briefly: a Rust engine runs inside the keyboard hook with a 1 ms budget per key. The Windows
spell checker says whether a word is wrong, a bundled SymSpell-style index finds candidates, and it only fixes
when one candidate clearly wins. Backspace right after a fix undoes it and remembers. Code-looking words,
terminals and password fields are left alone.

It's Windows only and English only for now. macOS and Linux are next on the same core crate, which has no OS
code in it, so that's where help would go furthest.

Repo: https://github.com/ReimagineMedias/unflub
Site: https://unflub.rmgn.ca

I'd like feedback on the code, and on the default-on usage counts. Tell me if that's the wrong call.

---
