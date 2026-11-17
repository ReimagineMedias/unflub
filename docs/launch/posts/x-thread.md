# X thread

Five posts. Each is under 280 characters; links count as 23. Post on L after the Show HN is up, but do not
link the HN thread or ask anyone to vote there.

## Before posting

- [ ] Repo public, v0.1.0 released, `https://unflub.rmgn.ca/` deployed, OG image live so the link unfurls.
- [ ] The 30-second video from `../assets.md`, attached to post 1.
- [ ] The download link uses `ref=x`. It counts as `other` until the API allowlist has `x`.
- [ ] The separate-process line in post 4 is true for v0.1, or use the alternative given.

## Posts

**1** (attach the video)

```text
You flubbed. We unflubbed.

unflub is phone-style autocorrect for every text box on Windows. Finish a word, and the typo is fixed before your next key lands.

Free, offline, MIT.

https://unflub.rmgn.ca
```

**2**

```text
I type fast and I make a lot of typos. My phone fixes them. My laptop just underlined them, and AI prompt boxes didn't even do that.

So I built the phone behaviour for every app, in Rust.
```

**3**

```text
The numbers, on my laptop:

0.1 ms to decide on a word
~20 MB in the background
0% CPU while you're not typing

CI fails any change that pushes them past their limits, including on a simulated slow PC.
```

**4**

```text
Backspace right after a fix puts your word back, and it won't touch that word again.

Code, passwords and terminals are left alone.

Nothing you type leaves your computer. The part that reads keys has no network code at all.
```

If the separate-process claim is not true for v0.1, end post 4 with this instead:

```text
Nothing you type leaves your computer, and the code is public so you can check.
```

**5**

```text
Windows now, English only. macOS and Linux are next.

Download: https://api.rmgn.ca/unflub/download/windows?ref=x
Or: winget install RMGN.unflub
Code: https://github.com/ReimagineMedias/unflub

Tell me where it fixed something it shouldn't have.
```
