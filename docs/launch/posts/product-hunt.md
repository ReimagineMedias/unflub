# Product Hunt

## Before posting

- [ ] Personal account, created at least a week before you need to post. New accounts wait a week, or
      subscribe to the newsletter for immediate access. Company accounts cannot post or comment.
- [ ] Verified email, and a filled-in profile: name, headline, website, links.
- [ ] Repo public, v0.1.0 released, `https://unflub.rmgn.net/` deployed.
- [ ] Feedback from the Show HN is fixed or answered, so this is not the same launch twice in a week.
- [ ] Gallery images and video from `../assets.md`.
- [ ] The privacy line in the first comment matches the app; the separate-process sentence is true or cut.
- [ ] You can be online from 3:01 AM Ottawa time, when the launch goes live at 12:01 AM Pacific, and most
      of that day.

Rules that matter, from the Product Hunt launch guide: never ask for upvotes, not even in a DM; no contests
tied to upvotes; one account per person. Asking people to try it and leave feedback is fine. Upvotes from new
accounts count for little, so a wave of friends signing up does not help. Save as a draft first. A scheduled
launch is reviewed, and an incomplete one can be removed. You can schedule from tomorrow up to 30 days out.
Weekends have less traffic and less competition, which the guide says can suit a side project.

## Fields

**Product link:** `https://unflub.rmgn.net/`. The guide says to link where people can get it; the site has
the download button and the privacy page.

**Name** (40 max): `unflub`

**Tagline** (60 max), pick one:

1. `Phone-style autocorrect for every app on Windows` (48)
2. `Free, offline autocorrect for every text box on Windows` (55)
3. `You flubbed. We unflubbed. Autocorrect for every Windows app` (60)

I would use 1. The guide asks for the plain version, and the joke goes in the first comment.

**Description** (500 max):

> unflub fixes typos the moment you finish a word, in any app on Windows: chat, browser, AI prompt boxes,
> email, notes. It decides in about 0.1 ms and sits at about 20 MB in the background. Press Backspace right
> after a fix to get your word back, and unflub remembers. Code, passwords and terminals are left alone.
> Nothing you type leaves your computer. Free and open source under MIT. macOS and Linux are next.

(409 characters.)

**Pricing:** Free.

**Launch tags** (up to 3): pick the closest of Productivity, Open Source and Writing, or Windows if it is
offered.

**Links:** GitHub repo `https://github.com/ReimagineMedias/unflub`, privacy page
`https://unflub.rmgn.net/privacy/`.

**Company info:** mark it open source and link the repo.

**Makers:** Matias. Add anyone else who built it, with their own account.

**Shoutouts** (optional): Tauri, if it has a Product Hunt page. Say what it did for us in a sentence, such as
"The settings window is a Tauri window that only exists while it is open, so it costs nothing the rest of
the time."

**Logo:** 240x240 PNG of the unflub mark on the espresso background. If it is animated, the first frame
must work alone, since GIFs only play on hover.

## Gallery shot list

1270x760 each, in this order. Details and sources in `../assets.md`.

1. The fix itself: "teh " becomes "the " in a chat box, with the pill showing. Caption: "Type fast. It
   fixes the word before your next key lands."
2. Undo: the fix, then Backspace putting the original back, with a small "unflub won't fix this again"
   note. Caption: "Backspace undoes it, and it learns."
3. The same fix in three apps side by side: an AI prompt box, a chat app, an email draft.
   Caption: "Every text box, not just the ones with spell check."
4. Code left alone: an editor where a comment gets fixed and `useState` and a file path do not.
   Caption: "Leaves code, passwords and terminals alone."
5. The numbers: 0.1 ms, ~20 MB, 0% CPU idle, on the studio-white card from the site.
   Caption: "Light enough to forget about."
6. Settings and the local Stats page. Caption: "You choose everything it does. Your Stats stay on your
   computer."

**Video:** the 30-second X video from `../assets.md`, uploaded to YouTube as unlisted or public.

## First comment

5,000 characters max. Posted and pinned when the launch goes live.

---

Hi Product Hunt, I'm Matias, and I make small tools under RMGN in Ottawa.

I type fast and I make a lot of typos. On my phone they fix themselves. On my computer they sit there, and
the places I type most, AI prompt boxes and chat apps, often don't even underline them. So I built unflub.
You flubbed. We unflubbed.

What it does:

- Fixes a typo the moment you finish the word, in any app on Windows.
- Backspace right after a fix puts your word back, and unflub stops fixing that word.
- Leaves code, passwords and terminals alone.

It is built for people who type faster than they spell, and for anyone who misses phone autocorrect on a
laptop.

How it is different from what you may already use: Grammarly checks grammar and tone, online. Windows' own
autocorrect only works in some apps. unflub only fixes typos, but it does that everywhere, offline, in about
0.1 ms, and it is free and open source under MIT. Nothing you type leaves your computer. The app does check
for updates daily and sends anonymous usage counts and crash reports unless you say no on first run. The
privacy page lists exactly what goes out.

It is Windows only for now, and English only. macOS and Linux are next on the same engine. The installer
isn't code-signed yet, so Windows will show a SmartScreen warning the first time.

What I would like to hear: which app it misbehaved in, and which word it fixed that it shouldn't have.

---

## During the day

- Reply to every comment, in your own words, as fast as you can.
- Share the launch link on X, LinkedIn and with people who already use unflub, asking them to try it and
  say what they think. Do not post the link in communities you have never taken part in.
- Do not post the launch link in the Show HN thread or ask HN readers to go there.
