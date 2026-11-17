# Launch assets

What to make, at what size, and which existing captures can stand in. Launch files go in
`site/.local/launch/`, which is git-ignored. Only images the repo or site actually uses get committed; the
README hero goes in the repo's `assets/` folder, under 10 MB.

## The rule for every asset

Show the real app in real apps wherever the asset claims "this is what it does": the README hero, the
Product Hunt gallery, AlternativeTo screenshots and the X video. The site's animated scenes are mock-ups
drawn in HTML. They are fine for the OG image and for brand cards like the numbers, but HN and Product Hunt
readers will notice if the "demo" is a web page.

Record on a clean Windows user profile with the release build, the default theme, Balanced mode, the pill on,
and nothing personal on screen: no real chats, no names, no browser tabs, no notifications. Use the Windows
11 light theme for app windows so the espresso pill stands out.

## Shot list

| Asset | Size and format | Content | Reuse | Owner |
|---|---|---|---|---|
| README hero | 960x540 GIF, 8 to 10 s loop, under 5 MB; also an MP4 for GitHub's video embed | Typing "Running late, teh train is stuk agian" into a real chat box. Each typo flips as the space lands, the pill shows briefly. Then "teh", fix, Backspace, "teh" comes back. Hold the last frame 1 s | `site/.local/recordings/fix-strip.png` shows the exact sentence and pill timing to copy. Its look is the old dark site, so do not use the frames themselves | agent records, Matias checks |
| PH gallery 1, the fix | 1270x760 PNG | Real chat window, the pill over a just-fixed "the", the caret after it | Same take as the README hero, a still frame | agent |
| PH gallery 2, undo | 1270x760 PNG | Two frames side by side: the fix, then after Backspace with the original word back | Same take | agent |
| PH gallery 3, every app | 1270x760 PNG | Three real windows: an AI prompt box in a browser, a chat app, an email draft, each with a fixed word underlined | `site/.local/recordings/everywhere.mp4` has the same five scenes as mock-ups; use it as the storyboard only | agent |
| PH gallery 4, code left alone | 1270x760 PNG | VS Code: a comment where "teh" became "the", next to `useState` and `src/main.rs` untouched | The "Code" scene in `everywhere.mp4` is the storyboard | agent |
| PH gallery 5, numbers | 1270x760 PNG | The three Speed cards on the night background with "Measured on..." under them | `site/.local/shots/final/night-1280x800.png` is this card from the current site. Re-render the `Speed` section alone, without the nav pill over it, at 1270x760 | agent |
| PH gallery 6, settings and stats | 1270x760 PNG | Correction page and the local Stats page, side by side | `site/.local/launch/source/shot-correction.png` and `shot-stats.png`, 880 px wide previews of the real settings UI. The Stats numbers are sample data, so recapture from real use or label it "sample data" | agent |
| PH logo | 240x240 PNG | The U mark on espresso | `site/public/icon-512.png`, scaled down | agent |
| PH video | YouTube link | The X video below | Same file | Matias uploads |
| OG image | 1200x630 PNG | Already exists: "Type fast. Land it right." with the teh to the pill | `site/public/og/unflub.png`, made by `npm run assets`. It uses the espresso background rather than the new studio white; it reads well in dark feeds, so keep it unless the owner wants it to match | none |
| GitHub social preview | 1280x640 PNG | Same layout as the OG image | Re-render with `site/scripts/make-assets.mjs` at 1280x640 | agent, Matias uploads in repo settings |
| X video | 1280x720 MP4, H.264, 25 to 30 s, no audio needed, captions burned in | 0 to 3 s "You flubbed." on espresso. 3 to 13 s real typing in a chat app with three fixes. 13 to 18 s Backspace undo. 18 to 24 s VS Code, comment fixed, code untouched. 24 to 30 s "We unflubbed. unflub.rmgn.ca" | `site/.local/recordings/final-arc-1280x800.mp4` is the current site's day-to-night scroll; a 2 to 3 s slice works as the closing card behind the URL | agent records and edits, Matias checks |
| AlternativeTo screenshots | 1280x800 PNG | PH gallery 1 to 4, re-cropped | From the PH set | agent |
| First-run tour | 880x620 PNG, for the README "what to expect" section | The "Try it" step with "You flubbed. We unflubbed." | `site/.local/launch/source/shot-tour1.png`, a preview of the real tour | agent |

## Existing captures and where they fit

| File | What it is | Use |
|---|---|---|
| `site/public/og/unflub.png` | 1200x630 OG card | Use as is |
| `site/.local/shots/final/night-1280x800.png` | Speed cards on the current site | PH gallery 5 source |
| `site/.local/shots/final/home-1440x900.png` | Current home hero, studio white | Press kit and LinkedIn header if needed |
| `site/.local/recordings/final-arc-1280x800.mp4` | 20 s scroll through the current site | Closing card for the X video |
| `site/.local/recordings/everywhere.mp4` | 19 s, five scenes, old dark site | Storyboard only |
| `site/.local/recordings/hero.mp4`, `fix-strip.png` | Old dark hero with the chat mock-up | Storyboard and timing only |
| `site/.local/recordings/lab-*.mp4` | Design experiments | Not for launch |
| `site/.local/launch/source/shot-*.png` | Real settings UI and tour, rendered in a browser preview with sample data | PH gallery 6, README tour image; recapture from the installed app before L |

## Checks before anything goes out

- [ ] No personal data in any frame: names, emails, chats, tabs, notification toasts, the taskbar clock if
      it gives away the time zone.
- [ ] Every number on an asset matches `site/src/data/performance.ts`.
- [ ] The README GIF is under 5 MB and the repo stays free of files over 10 MB.
- [ ] Frames of every video checked by eye, first and last frame included, before anyone posts.
- [ ] Captions burned into the X video, so it works with the sound off.
