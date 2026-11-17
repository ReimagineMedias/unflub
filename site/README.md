# unflub.rmgn.net

The marketing site for unflub. Astro, static output, hosted on Vercel. Nothing in the code is Vercel-specific:
`dist/` plus the headers in `vercel.json` work on any static host.

```sh
npm install
npm run dev        # http://localhost:4321
npm run build      # dist/
npm run preview    # serve dist/ with Astro
npm run serve      # serve dist/ with the production headers from vercel.json (CSP, caching), port 4322
npm run check      # astro check (types)
npm run lint       # html-validate on dist/
npm test           # pages answer 200, links resolve, metadata, JSON-LD, headers (needs a build)
```

## Where things live

| What | File |
|---|---|
| Links, version, download URL | `src/data/site.ts` |
| Performance numbers | `src/data/performance.ts` (keep in step with `docs/performance.md`) |
| FAQ (also emitted as FAQPage JSON-LD) | `src/data/faq.ts` |
| Comparison table | `src/data/comparison.ts` |
| Typing demos (one engine for every scene) | `src/scripts/typer.ts` |
| Pill and app-window styles | `src/styles/scenes.css` (pill copies `crates/unflub-windows/src/overlay.rs`) |
| Settings recreation | `src/components/Settings.astro` (tokens copied from `desktop-app/app/src/styles.css`) |
| Security and cache headers, trailing slashes | `vercel.json` |

Every script is emitted as a file (`vite.build.assetsInlineLimit: 0`) so the CSP can stay at
`script-src 'self'`. The tests fail if a page gains an inline script.

## Images

`npm run assets` renders the favicon set and the Open Graph image (`scripts/assets/og.html`) with
the local Chrome and writes them to `public/`. Commit the results. `npm run shots` saves
screenshots of every page into `.local/shots/` (needs a running preview).

## Deploying

Vercel's Git integration deploys the site: the project's Root Directory is `site`, and `vercel.json` sets the
build, `dist/` output, trailing slashes and headers. `.github/workflows/site.yml` only checks, builds and tests.
Build-time settings, both public values, set in Vercel and as GitHub repository variables:

| Variable | What it does when set |
|---|---|
| `PUBLIC_CF_BEACON_TOKEN` | Adds Cloudflare Web Analytics (cookieless) to every page |
| `PUBLIC_TURNSTILE_SITE_KEY` | Download buttons pass a Cloudflare Turnstile check before the download starts |

Actions in the workflow are pinned to commit SHAs with the tag in a comment; update both together.
