# unflub.rmgn.ca

The marketing site for unflub. Astro, static output, served by Cloudflare Workers static assets.

```sh
npm install
npm run dev        # http://localhost:4321
npm run build      # dist/
npm run preview    # serve dist/ with Astro
npm run serve      # serve dist/ with the production _headers (CSP, caching), port 4322
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
| Security and cache headers | `public/_headers` |

Every script is emitted as a file (`vite.build.assetsInlineLimit: 0`) so the CSP can stay at
`script-src 'self'`. The tests fail if a page gains an inline script.

## Images

`npm run assets` renders the favicon set and the Open Graph image (`scripts/assets/og.html`) with
the local Chrome and writes them to `public/`. Commit the results. `npm run shots` saves
screenshots of every page into `.local/shots/` (needs a running preview).

## Deploying

`.github/workflows/site.yml` deploys on every push to `main` that touches `site/`, from the
`production` environment, with the wrangler pinned in `package-lock.json` (`npx --no-install wrangler
deploy`). It needs two secrets, `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`. Scope the token to
these permissions only:

| Scope | Permission |
|---|---|
| Account | Workers Scripts: Edit |
| Zone `rmgn.ca` | Workers Routes: Edit |
| Zone `rmgn.ca` | DNS: Edit |

The first deploy creates the `unflub.rmgn.ca` custom domain on the `rmgn.ca` zone. Actions in the
workflow are pinned to commit SHAs with the tag in a comment; update both together.
