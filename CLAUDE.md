# unflub

System-wide, phone-style autocorrect. RMGN product, site at unflub.rmgn.ca (Cloudflare, like rmgn.ca).
Keep `AGENTS.md` identical in substance.

Git workflow: PRs. Branch `type/topic`, open a PR on ReimagineMedias/unflub, merge only when the owner says so (squash).
Repo is private until v0.1, then flipped public.

## Findings

- Build from PowerShell, not Git Bash: Git Bash's `/usr/bin/link` shadows MSVC `link.exe` and every link fails.
- Windows `ISpellChecker` measured 2026-11-16: `Check` on a correct word ~0.04 ms, `Suggest` 5-100 ms. Only `Check` (and the
  auto-replace lookup via `ComprehensiveCheck`, when fast) may run on the hook path. Top suggestion right on 20/23 real typos;
  it flags identifiers like `useState`, so code-skip rules matter.
- The hook path budget is 1 ms worst case; corrections are applied synchronously inside the hook so fast typing can't interleave.
- Brand: default theme palette `#2D1916` espresso, `#79453A` brown, `#F2E6E3` blush paper, `#B06A5C` rose. Other themes
  (RMGN paper, quiet ink, System accent) are settings, not replacements. Fonts Manrope + IBM Plex Mono.
- Pill default: brief underline on the fixed word plus a small capsule (~1 s). Capsule, underline-only and off are all settings.
- Site (`site/`): Astro static, deployed by `.github/workflows/site.yml` to Cloudflare Workers static assets
  (`site/wrangler.jsonc`, custom domain unflub.rmgn.ca). Needs secrets `CLOUDFLARE_API_TOKEN` (scoped to Account Workers
  Scripts:Edit and rmgn.ca Workers Routes:Edit + DNS:Edit only) and `CLOUDFLARE_ACCOUNT_ID`. Deploys with the lockfile's
  wrangler (`npx --no-install`), no third-party deploy action; workflow actions are SHA-pinned. Performance figures live in `site/src/data/performance.ts`; keep them in step with
  docs/performance.md. Scripts must stay external files (CSP is `script-src 'self'`; a test enforces it).
- In Git Bash, `--flag=/path`-style args get rewritten to Windows paths (MSYS); pass names, not leading-slash paths.
