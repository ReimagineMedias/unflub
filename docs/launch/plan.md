# unflub launch and discoverability plan

Written 2026-11-17. Nothing is public yet: `unflub.rmgn.ca` and `api.rmgn.ca` do not resolve, the repo is
private, and v0.1 is not released. Everything below is ordered so the owner's part is short clicks at the end.

"L" is launch day, the day the Show HN goes up. Owner is Matias unless a row says agent. Agents never post,
create accounts or change DNS; they prepare the change and Matias applies it.

## What the research changed

These findings shaped the plan more than the general advice did. Sources are at the bottom.

1. **Handy did not take off through its own Show HN.** The maker's "Show HN: Handy" on 2025-06-17 got 3
   points and 1 comment. The posts that reached the front page were regular submissions by other people:
   237 points on 2025-09-27 and 247 points on 2026-01-15, the second one linking straight to the GitHub repo.
   Handy's winget package is community-maintained, and its README says so. The lesson for us is that a Show HN
   is one try with a low hit rate. What carried Handy was a plain repo that users wanted to share. HN allows
   "a small number of reposts" when a story got no real attention, so one quiet Show HN is not the end.
2. **unflub is not the only system-wide autocorrect for Windows.** Around it are AutoCorrect PC in the
   Microsoft Store, which had a Show HN on 2026-08-12 with 2 points, Typen, which is open source and offline
   and was created on 2026-11-04, EkaKey with 114 stars, Texthelp's Global AutoCorrect for dyslexia, tinySpell,
   and the old AutoHotkey AutoCorrect script. "Autocorrect everywhere" alone will not stand out. The specifics
   do: free and MIT, offline, about 0.1 ms per word, leaves code and terminals alone, Backspace undoes and
   learns, and no AI. Every draft leads with those.
3. **Most of the subreddits we listed say no.** r/windows wants mod permission and is not taking applicants.
   r/productivity bans self-promotion in any form. r/dyslexia bans apps and self-promotion and says not to
   ask for an exception. r/Windows11 allows software only in its weekly Software Showoff thread. r/rust says
   applications that happen to be written in Rust are off-topic outside the weekly "What's everyone working
   on" thread. Only r/opensource, with the Promotional flair, and r/software, on Wednesdays with flair, take
   a direct post. r/opensource, r/productivity and r/rust also remove AI-generated posts, so Matias should
   rewrite the drafts in his own words before posting.
4. **The site's CSP would block Cloudflare Web Analytics.** `site/public/_headers` sets
   `script-src 'self'`, and the beacon loads from `static.cloudflareinsights.com`. A test in
   `site/tests/site.test.mjs` also asserts the exact `script-src 'self';` string. Turning analytics on needs a
   small code change first.
5. **Channel refs on the download link collapse to `other`.** The API accepts only `site`, `github`,
   `readme` and `other` as `ref` values (`D:\Code\rmgn-api\src\products\unflub.ts`). Without new values we
   cannot tell HN downloads from Product Hunt downloads.
6. **The site's download button still points at GitHub Releases** (`SITE.download` in
   `site/src/data/site.ts`). `docs/releasing.md` on the release branches says it should be
   `https://api.rmgn.ca/unflub/download/windows?ref=site`.
7. **Three claims need checking before they go in a post.** "The part that sees keys runs as a
   separate process with no network code, and CI enforces it" does not match the newest architecture doc I
   found (`feat/engine-process`), which says the app runs the adapter in-process and the update check runs on
   a thread of the core process. And the `README.md` on `main` still says "No accounts, no telemetry, no
   cloud", which is wrong now that usage counts and crash reports are on by default. And the site says the
   numbers were "measured on a typical laptop", while `docs/performance.md` says the development laptop has
   32 logical cores. "Measured on our development laptop and in a simulated slow PC" is accurate and still
   reads well. Each draft lists the claims it depends on under its prerequisites.

## Phase 1: pre-launch, L-21 to L-1

| Channel | Action | Owner | Prerequisite | Success signal |
|---|---|---|---|---|
| Product | Merge the release branches, tag v0.1.0, publish the release, roll out to 100% | Matias | CI green on `ci/release` | `https://api.rmgn.ca/unflub/download/windows?ref=site` returns 302 to the setup |
| Product | Submit the v0.1.0 setup to Microsoft for malware analysis and attach the VirusTotal link, as `docs/releasing.md` step 7 says | Matias | Release live | Clean verdict from Microsoft; SmartScreen warning text documented on /download/ |
| Product | Apply to SignPath Foundation for free code signing | Matias | Repo public | Application submitted; signed builds can come in a later release |
| Site | Merge `feat/site`, let `site.yml` deploy, confirm `https://unflub.rmgn.ca/` loads | Matias | `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` secrets | All five pages return 200; `sitemap-index.xml` lists them |
| Site | Point `SITE.download` at `https://api.rmgn.ca/unflub/download/windows?ref=site` | agent | API deployed | Download button redirects to the setup |
| API | Add `hn`, `ph`, `reddit`, `x`, `linkedin`, `alternativeto`, `winget` to the unflub `refs` allowlist in `rmgn-api` | agent, Matias merges | Owner agrees, see decisions | Dashboard shows downloads per channel |
| Site | Change "Measured on a typical laptop" in `site/src/data/performance.ts` to name the real setup | agent | Owner agrees to the wording | Site and `docs/performance.md` say the same thing |
| Site | Allow the Web Analytics beacon in the CSP, update the CSP test, add one line about site analytics to /privacy | agent | Owner agrees to site analytics | `npm test` passes; beacon loads on the live site |
| Site | Cloudflare Web Analytics, steps below | Matias | Site deployed, CSP change merged | Page views show in the dashboard within a day |
| Search | Google Search Console, steps below | Matias | Site deployed | Domain property verified; sitemap status "Success" with 5 URLs |
| Search | Bing Webmaster Tools, import from Search Console | Matias | Search Console verified | Site verified in Bing; sitemap accepted |
| Search | IndexNow key file and a post-deploy ping in `site.yml` | agent | Site deployed | Ping returns 200 or 202; Bing Webmaster Tools lists the URLs under IndexNow |
| Search | On `/autocorrect-windows/`, add short answers to "Does Windows have autocorrect?" and "How do I get autocorrect on my PC?" | agent | none | Page shows for those queries in Search Console within weeks |
| GitHub | Make the repo public | Matias | README accurate, no secrets in history | Repo visible logged out |
| GitHub | Rewrite the README in Handy's shape: one-line pitch, hero GIF, install lines for the setup, winget and Scoop, how it works, privacy, limits | agent | v0.1 facts final; privacy section from `feat/engine-process` | README states what is sent and what is not, matching /privacy |
| GitHub | Description "Phone-style autocorrect for every text field on Windows. Free, offline, MIT."; website `https://unflub.rmgn.ca`; topics `autocorrect`, `spell-checker`, `typing`, `windows`, `rust`, `tauri`, `productivity`, `offline`, `privacy`, `accessibility` | Matias | Repo public | Repo shows in GitHub topic pages |
| GitHub | Upload the social preview image, 1280x640 | Matias | Asset made, see `assets.md` | Links to the repo unfurl with the image |
| winget | Submit `RMGN.unflub` 0.1.0 by hand, per `docs/releasing.md`; turn on `publish-packages.yml` after the merge | Matias | Release live, hashes filled in | `winget install RMGN.unflub` works in Windows Sandbox before L |
| Scoop | Create `ReimagineMedias/scoop-bucket` from `packaging/scoop/unflub.json` | agent prepares, Matias creates repo | Release live | `scoop install rmgn/unflub` works |
| Chocolatey | Push the package; moderation is by hand and can take a while | Matias | Release live, Chocolatey account | Package approved; not needed for L |
| Product Hunt | Create a personal account now. New accounts wait a week before they can post | Matias | none | Account can open Submit |
| Product Hunt | Build the launch page as a draft, not scheduled | Matias | Account active, assets ready | Draft passes the "Before you launch" checklist |
| Reddit | Take part in r/opensource and r/software as yourself, so a later post is not a drive-by | Matias | none | Some comment history in both; r/software has a hidden karma threshold |
| AlternativeTo | Submit unflub as a new app; moderation takes time | Matias | Repo public, site live | Listing approved and linked as an alternative to tinySpell, Grammarly, LanguageTool |
| Assets | Record the hero GIF, PH gallery, OG image and X video per `assets.md` | agent | Release build on a clean Windows profile | Files in `site/.local/launch/`, checked by Matias |

## Phase 2: launch week

Show HN and Product Hunt go on different days, so Matias can stay in each thread. Product Hunt goes live at
12:01 AM Pacific, 3:01 AM in Ottawa.

| Day | Channel | Action | Owner | Prerequisite | Success signal |
|---|---|---|---|---|---|
| L, a weekday morning ET | Hacker News | Post `posts/show-hn.md`, linking the GitHub repo; post the first comment right away; reply to every comment for the first 6 to 8 hours | Matias | Repo public, release live, site deployed, winget merged | Reaches the /show page; real questions answered within the hour; `ref=hn` downloads |
| L | AlternativeTo | If the listing is approved, add the "alternative to" links and screenshots | Matias | Listing approved | Listing appears on the tinySpell page |
| L, after HN is up | X | Post `posts/x-thread.md` with the video | Matias | Video ready | Replies and reposts from people who type a lot |
| L+1 | LinkedIn | Post `posts/linkedin.md` | Matias | Release live | Comments from people outside the dev crowd |
| L+1 | r/opensource | Post `posts/reddit-opensource.md` with the Promotional flair; stay for the thread | Matias | Repo public with LICENSE | Post stays up; questions answered |
| First Wednesday | r/software | Post `posts/reddit-software.md` with the release flair | Matias | Karma above their threshold | Post stays up |
| First Sunday | r/Windows11 | Comment in the Software Showoff thread with `posts/reddit-windows11.md` | Matias | Thread is up | Comment gets replies |
| That week | r/rust | Comment in "What's everyone working on this week" with `posts/reddit-rust.md` | Matias | Thread is up | Comment gets replies |
| L+7 or later | Product Hunt | Schedule the draft, write the first comment from `posts/product-hunt.md`, be online from 3 AM ET | Matias | Account older than a week; fixes from HN feedback shipped | Featured on the homepage; feedback comments; `ref=ph` downloads |
| All week | GitHub | Triage issues within a day; label the good first ones | Matias, agent drafts replies | Repo public | Issues answered; first outside PR |

Do not post to r/windows, r/productivity or r/dyslexia. Their drafts say why.

## Phase 3: ongoing

| Channel | Action | Owner | Prerequisite | Success signal |
|---|---|---|---|---|
| Search Console | Weekly look at queries and pages; fix titles where impressions are high and clicks are low | agent reports, Matias reads | Two weeks of data | Clicks on the keyword map's queries grow |
| Bing | Same check monthly; IndexNow tab shows each deploy's ping | agent | Bing verified | URLs indexed |
| Pages | Build the new pages below, one at a time, only after the owner picks them | agent | Owner decision | Each one gets impressions for its query family within a month |
| awesome-rust | PR to Applications > Productivity, using `posts/awesome-list.md` | Matias | 50 stars | PR merged |
| awesome-windows | Entry in Productivity, written and submitted by Matias himself; see the draft for why | Matias | 50 stars and either 30 days old or a commit in the last 30 days | PR merged |
| Stack Exchange | Answer new questions about system-wide autocorrect on Super User, saying plainly that you made unflub, and solve the problem first | Matias | none | Answers kept, not flagged |
| Reddit | Answer new "is there autocorrect on PC" questions only where the sub allows it; never revive old threads to drop a link | Matias | none | No removals |
| Press | Short email to Windows news sites that cover small free tools, with the GIF and the numbers | Matias | 100+ stars or a front page | One write-up |
| Product Hunt | Relaunch with the macOS version, two or more months after the first launch | Matias | macOS release | Second launch on the same product page |
| Hacker News | Show HN again for macOS and Linux only if it is a real overhaul; Show HN rules call plain upgrades off-topic | Matias | macOS release | n/a |
| winget, Scoop | Check each release opens the winget PR and Scoop picks up the version | agent | `PUBLISH_PACKAGES=true` | Package versions match the release within a day |

## Google Search Console, Bing and IndexNow

### Google Search Console: verify once with DNS

I recommend a **Domain property for `rmgn.ca` verified with a DNS TXT record in Cloudflare**. A Domain
property covers every subdomain and both protocols, so `unflub.rmgn.ca`, `elvyn.rmgn.ca` and future products
are all covered by one record. Nothing goes into the repo, and redeploys cannot break it. The HTML file method
only covers one URL-prefix property and lives in `site/public/`, where a cleanup could delete it. `rmgn.ca`
already uses Cloudflare nameservers and has one TXT record for Zoho SPF, which stays as it is.

1. Open https://search.google.com/search-console, choose **Add property**, then **Domain**, and enter
   `rmgn.ca`.
2. Google shows a TXT value starting with `google-site-verification=`. If it offers to sign in to Cloudflare
   and add the record for you, use that. Otherwise copy the value.
3. In Cloudflare: **rmgn.ca > DNS > Records > Add record**. Type `TXT`, Name `@`, Content the full
   `google-site-verification=...` value, TTL Auto. Save. Leave the SPF record alone; a name can hold
   several TXT records.
4. Back in Search Console, press **Verify**. If it fails, wait a few minutes and try again.
5. Add a second property, **URL prefix** `https://unflub.rmgn.ca/`. Google verifies child properties of a
   verified property automatically. Use this one for unflub's own reports.
6. In the `https://unflub.rmgn.ca/` property: **Sitemaps**, enter `sitemap-index.xml`, **Submit**. Status
   should read "Success" and list 5 discovered URLs: `/`, `/download/`, `/privacy/`,
   `/autocorrect-windows/` and `/spell-check-everywhere/`.
7. **URL inspection** for each of those five URLs, then **Request indexing**. This is a one-time nudge.
8. Settings > Users and permissions: add a second owner if anyone else should keep access.

If you would rather verify only the subdomain, add a Domain property for `unflub.rmgn.ca` and put the TXT
record on the name `unflub` instead of `@`. I have not checked that Cloudflare accepts a TXT record next to
the Worker's custom domain record, which is one more reason to use the apex.

### Bing Webmaster Tools: import from Google

1. Open https://www.bing.com/webmasters and sign in.
2. Choose **Import from Google Search Console**, allow access, tick `https://unflub.rmgn.ca/`, **Import**.
   Bing verifies it and copies the sitemap. Bing then checks ownership through the Search Console link
   from time to time and imports new sitemaps itself.
3. If you prefer not to link the accounts, add `https://unflub.rmgn.ca/` by hand and use **Add CNAME record
   to DNS**. In Cloudflare, add the CNAME Bing gives you with the proxy turned off, so the cloud icon is
   grey. Then submit `https://unflub.rmgn.ca/sitemap-index.xml` under **Sitemaps**.
4. Bing says data takes about 48 hours to appear.

### IndexNow: a key file and a ping after each deploy

IndexNow tells Bing, Naver, Seznam, Yandex and Yep about changed URLs, and engines that join share the
submissions. Google does not take part, so the sitemap is still what Google reads.

Agent steps, in one small PR:

1. Generate a key of 32 hex characters and commit `site/public/<key>.txt` containing only the key.
   The key is public by design; it proves the submitter controls the host.
2. Add a step to the `deploy` job in `.github/workflows/site.yml`, after `wrangler deploy` succeeds. It reads
   the `<loc>` URLs from `dist/sitemap-0.xml` and posts them:

   ```sh
   curl -sS -o /dev/null -w "%{http_code}" -X POST https://api.indexnow.org/IndexNow \
     -H "Content-Type: application/json; charset=utf-8" \
     -d '{"host":"unflub.rmgn.ca","key":"<key>","keyLocation":"https://unflub.rmgn.ca/<key>.txt","urlList":[...]}'
   ```

   200 means received. 202 means received with the key still being checked. Write the code to the job
   summary next to the deploy row. A failed ping should not fail the deploy.
3. In Bing Webmaster Tools, the **IndexNow** page shows what arrived.

Also turn on **Crawler Hints**: Cloudflare dashboard > rmgn.ca > **Caching > Configuration > Crawler
Hints**. It is free on every plan and sends IndexNow pings for the whole zone when Cloudflare's cache sees
changed content. I could not confirm that it sees Workers static-asset responses, which is why the
workflow ping above is the one to rely on.

## Cloudflare Web Analytics

Cookie-free page views and Core Web Vitals, with no code in the page beyond Cloudflare's beacon. Cloudflare
says it "does not collect or use your visitors' personal data."

The code change comes first, or the CSP blocks the beacon:

- `site/public/_headers`: `script-src 'self' https://static.cloudflareinsights.com`. With automatic setup the
  beacon reports to `https://unflub.rmgn.ca/cdn-cgi/rum`, which `connect-src 'self'` already allows. With the
  manual snippet, also add `https://cloudflareinsights.com` to `connect-src`.
- `site/tests/site.test.mjs` line 141 asserts the exact `script-src 'self';`. Change it to accept that one
  extra origin and nothing else.
- `/privacy/`: one sentence saying the website counts page views with Cloudflare Web Analytics, without
  cookies, and that this is separate from the app.

Then the toggle, done by Matias:

1. Cloudflare dashboard > **Analytics & Logs > Web Analytics > Add a site**. Pick the `rmgn.ca` zone and the
   hostname `unflub.rmgn.ca`.
2. Automatic setup is on by default for proxied sites and injects the beacon on every hostname in the zone.
   Under **Manage site > Rules**, limit it to `unflub.rmgn.ca` unless you also want it on `rmgn.ca` and
   `elvyn.rmgn.ca`. **Enable, excluding visitor data in the EU** is an option if you want to skip EU visitors.
3. Load `https://unflub.rmgn.ca/`, open DevTools > Network, and look for `beacon.min.js` and a request to
   `/cdn-cgi/rum`. If the beacon never appears, the Workers static-asset response is not being rewritten.
   Switch to **Enable with JS Snippet installation** and have the agent add the snippet to
   `site/src/layouts/Base.astro`, with the CSP changes for the manual path above.
4. Ad blockers block the beacon, so real traffic is higher than the dashboard shows, more so for HN and
   r/opensource readers.

## Keyword to page map

Signals come from Google, Bing and DuckDuckGo autocomplete, pulled on 2026-11-17 with US English settings,
and from threads found by search. Autocomplete shows that people type a phrase. It does not say how many.
None of the free tools we used give volumes, so this table has none. Treat the order as a guess to check
against Search Console after a month.

Legend: G Google, B Bing, D DuckDuckGo autocomplete. T a thread or Q&A asking for this.

| Query family | Signals | What the searcher wants | Page | Change needed |
|---|---|---|---|---|
| autocorrect for windows, autocorrect app for pc, autocorrect software for pc, best autocorrect for windows, global autocorrect software | G B D | An app to install | `/` and `/download/` | Home title already leads with "phone-style autocorrect for every app on Windows" |
| does windows have autocorrect, is there autocorrect on pc, how to get autocorrect on pc, autocorrect on laptop | G B D, T: r/Windows11 "Is there still no native Autocorrection for Windows?" | To learn whether Windows can do it | `/autocorrect-windows/` | Add the two short answers from phase 1 near the top |
| autocorrect windows 11, autocorrect misspelled words windows 11, custom autocorrect windows 11, windows 11 autocorrect dictionary | G B D | Windows' own setting | `/autocorrect-windows/` | Already covered; check the setting names match Windows 11 25H2 |
| windows 11 autocorrect not working, windows spell check not working, autocorrect not working on laptop | G B D, T: r/WindowsHelp "Windows 11 Autocorrect and Dictionary are driving me crazy" | A fix | Proposed page 1 | New page |
| system wide autocorrect, windows system wide autocorrect, system wide spell check, spell check anywhere, windows spell check across all apps | G, B D for the last, T: Super User "replace-as-you-type system-wide", r/software "Software that autocorrects your typing?" | The exact thing unflub is | `/spell-check-everywhere/` and `/` | Use "system-wide" once in the home description and the spell-check page intro |
| autocorrect for discord, is there autocorrect on discord, is there autocorrect on teams, is there autocorrect on notion | G B D | Autocorrect in one app that lacks it | `/autocorrect-windows/` | One line naming Discord, Teams, Slack and AI chat boxes as places it works |
| grammarly alternative open source, grammarly alternative no ai, grammarly alternative free, offline grammar checker for windows | G B D | A free, private replacement | Proposed page 2 | New page; honest that unflub fixes typos, not grammar |
| tinyspell, tinyspell windows 11, autohotkey autocorrect script, ahk autocorrect v2 | B D for tinySpell, G for AutoHotkey | A specific older tool | Proposed page 2 | Same page, one section each |
| spell checker for dyslexia, autocorrect for dyslexia, spell check apps for dyslexia | G B D | Help with spelling | Proposed page 3, only if tested | Owner decision |
| windows text suggestions, windows text suggestions disable | G B D | Mostly people turning it off | none | Not a fit; skip |
| typo fixer | G B D | Mixed: crossword clues, online tools | none | Skip |

## New pages worth building, at most three

1. **`/autocorrect-not-working/`**, "Windows autocorrect not working? Here is why it skips most apps."
   The "not working" family shows up on all three engines for both autocorrect and spell check. The honest
   answer is useful on its own: the Windows setting only reaches apps that use Windows text services, and
   many popular apps do not. Steps to check the setting come first, unflub last.
2. **`/alternatives/`**, "unflub, Grammarly, tinySpell and the AutoHotkey AutoCorrect script compared."
   People search for each of these by name. The page reuses `site/src/data/comparison.ts` and is fair about
   what each does better. Grammarly checks grammar and tone; unflub only fixes typos as you type. Linking
   to alternatives that beat us on something earns more trust than a table we always win.
3. **`/dyslexia/`**, only after testing with dyslexic typists. The demand signal is clear, and the topic
   needs care. unflub's candidates are words within two edits of what was typed, so phonetic spellings that
   are far from the word will often be left alone. The page would have to say that plainly and make no
   claims beyond what tests show. If we cannot test it, do not build it.

## Measuring

- Downloads by `ref`, once the allowlist has the channel values. Every draft uses
  `https://api.rmgn.ca/unflub/download/windows?ref=<channel>`, which counts as `other` until then.
- winget, Scoop and Chocolatey installs skip the download endpoint. The app's `install-source` file shows up
  in the anonymous usage counts by source.
- GitHub stars and traffic, Search Console clicks, Web Analytics referrers.

## Decisions for the owner

1. Is the "separate process with no network code, enforced by CI" claim true for v0.1? If not, cut it from
   every draft. The drafts mark it.
2. Extend the download `ref` allowlist with channel values? It is a small change in `rmgn-api`.
3. Turn on Cloudflare Web Analytics for the site, which means the CSP change and a privacy line?
4. Verify Search Console with a Domain property on `rmgn.ca`, as recommended, or only on `unflub.rmgn.ca`?
5. Which of the three new pages to build, and whether the dyslexia page is worth a round of user testing.
6. Launch v0.1 unsigned, with SmartScreen's "Windows protected your PC", or wait for SignPath? HN readers
   will mention it either way; the drafts say it plainly.

## Sources

All read on 2026-11-17.

- Show HN rules: https://news.ycombinator.com/showhn.html
- HN guidelines: https://news.ycombinator.com/newsguidelines.html
- HN FAQ, reposts and the /show threshold: https://news.ycombinator.com/newsfaq.html
- Handy's HN history, from the Algolia HN API: items 44302416 (Show HN, 3 points), 45399106 (237 points),
  46628397 (247 points), https://news.ycombinator.com/item?id=46628397
- Handy repo, 39,031 stars, MIT: https://github.com/cjpais/Handy; winget manifest:
  https://github.com/microsoft/winget-pkgs/tree/master/manifests/c/cjpais/Handy
- AutoCorrect PC Show HN: https://news.ycombinator.com/item?id=49276049; Store listing:
  https://apps.microsoft.com/detail/9ncfnqh7x86c
- Typen: https://github.com/TypenApp/typen; EkaKey: https://github.com/RanvirRox/EkaKey-autocorrect-globally
- Product Hunt launch guide: https://help.producthunt.com/en/collections/19793093-launch-guide, in particular
  "Build your product page" (name 40, tagline 60, description 500 characters, gallery 1270x760, first
  comment 5,000), "How to promote your launch" (never ask for upvotes) and
  https://help.producthunt.com/en/articles/481909-how-can-i-get-access-to-post (one-week wait)
- Subreddit rules, read from each sub's rules widget: r/windows, r/Windows11, r/productivity, r/opensource,
  r/software, r/rust, r/dyslexia
- Super User self-promotion: https://superuser.com/help/promotion
- Google verification: https://support.google.com/webmasters/answer/9008080
- Bing add and verify: https://www.bing.com/webmasters/help/add-and-verify-site-12184f8b
- IndexNow protocol: https://www.indexnow.org/documentation; Bing's IndexNow page with the
  `api.indexnow.org` endpoint: https://www.bing.com/indexnow/getstarted
- Cloudflare Crawler Hints: https://developers.cloudflare.com/cache/advanced-configuration/crawler-hints/
- Cloudflare Web Analytics setup, FAQ and CSP: https://developers.cloudflare.com/web-analytics/get-started/,
  https://developers.cloudflare.com/web-analytics/faq/
- winget submission: https://learn.microsoft.com/en-us/windows/package-manager/package/
- Scoop main bucket criteria, which exclude GUI apps:
  https://github.com/ScoopInstaller/Scoop/wiki/Criteria-for-including-apps-in-the-main-bucket
- Chocolatey moderation: https://docs.chocolatey.org/en-us/community-repository/moderation/
- awesome-rust contributing: https://github.com/rust-unofficial/awesome-rust/blob/main/CONTRIBUTING.md
- awesome-windows contributing: https://github.com/0PandaDEV/awesome-windows/blob/main/CONTRIBUTING.md
- AlternativeTo spell-checker and autocorrect tags: https://alternativeto.net/browse/all/?tag=spell-checker.
  Its FAQ sits behind a bot check and could not be read.

Things I could not check: AlternativeTo's submission rules; whether Crawler Hints covers Workers static
assets; whether Cloudflare's automatic Web Analytics injection rewrites Workers static-asset responses;
search volumes of any kind.
