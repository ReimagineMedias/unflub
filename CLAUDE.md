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
- Core engine (`crates/unflub-core`): SymSpell (MIT) frequency lists in `data/`, embedded with `include_str!`. Index build
  ~100 ms release, ~15 MB heap. Boundary decision p99 ~90 us, hard unknown words ~150 us max. Ranking weights live in
  `src/rank.rs` (`TUNING`) and `src/distance.rs`; after changing them run
  `cargo test --release -p unflub-core --test corpus -- --nocapture` (owner corpus 22/25, common typos 103/111) and
  `cargo run --release -p unflub-core --example bench`.
- Windows `ComprehensiveCheck` measured 2026-11-16 (Windows 11 26300, en-US): ~3 ms per typo, and its auto-replace list
  has `recieve`->`receive`, `becuase`->`because` but nothing for `teh`. `CORRECTIVE_ACTION_REPLACE` is rare; don't rely on it.
- `unflub-windows` tests call the real `ISpellChecker` but never install hooks or inject input. Try the hook live with
  `cargo run --release --bin unflub -- --demo` (rules teh/adn/recieve). Never run it in automation on a machine someone
  is typing on.
