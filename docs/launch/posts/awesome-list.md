# Awesome lists

Two lists fit. Both want proof that people use the project, so neither is a launch-day item. A third,
awesome-tauri, no longer lists apps; its README only has guides, articles, templates, plugins and
integrations.

## rust-unofficial/awesome-rust

Read 2026-11-17: https://github.com/rust-unofficial/awesome-rust/blob/main/CONTRIBUTING.md

- Needs at least 50 GitHub stars, 2,000 crates.io downloads, or another popularity measure stated in the PR.
- Template: `[ACCOUNT/REPO](https://github.com/ACCOUNT/REPO) [[CRATE](https://crates.io/crates/CRATE)] - DESCRIPTION`.
  unflub has no published crate, so the crate part is left out, as other app entries do.
- Alphabetical within the section.
- Section: **Applications > Productivity**. It already has Windows desktop apps, such as Beetroot.

### Before opening the PR

- [ ] 50 or more stars.
- [ ] Repo public, a release out.
- [ ] Entry placed alphabetically under Productivity; check the neighbours by hand.

### Entry

```markdown
* [ReimagineMedias/unflub](https://github.com/ReimagineMedias/unflub) - Phone-style autocorrect for every text field on Windows. Fixes typos as you type, offline, and leaves code alone.
```

### PR title

`Add unflub to Productivity`

### PR body

```markdown
Adds unflub under Applications > Productivity.

unflub is a Windows app, written in Rust, that fixes typos as you type in any text field. The engine
(`unflub-core`) is platform-free Rust with no `unsafe`; the Windows adapter calls it from a low-level
keyboard hook within a 1 ms budget per key. MIT licensed.

Popularity: <N> stars on GitHub.
```

## 0PandaDEV/awesome-windows

Read 2026-11-17: https://github.com/0PandaDEV/awesome-windows/blob/main/CONTRIBUTING.md

- At least 50 GitHub stars, and either 30 days since the first commit or a commit in the last 30 days.
- One PR per app. Title-cased AP style, in the format `* [List Name](link)`. Alphabetical in the section.
- They also accept an "Add Application" issue, which opens the PR for you.
- The CONTRIBUTING file opens with: "Vibecoded slop and tools that don't fall in the category of 'awesome'
  are not welcomed on this list and PR's will be rejected." The README also says the maintainers do not
  accept AI-powered PRs or contributions.

Because of that last point I have not drafted PR text for this list. If Matias submits, he should write the
issue or PR himself. He should also decide whether unflub, which was built with AI coding agents, is
something these maintainers would want at all. Ask them in an issue first if unsure.

Facts for the entry, in the list's own format, for Matias to use or ignore:

- Section: **Productivity**. Neighbours include AutoHotkey, Beetroot and KatMouse.
- Open-source entries add the list's `[![Open-Source Software][oss]](repo link)` badge after the
  description.
- Link the site, `https://unflub.rmgn.ca`, and the repo in the badge.
