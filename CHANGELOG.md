# Changelog

What changed in each unflub release, newest first. Generated from the Conventional Commit
titles of merged pull requests by `scripts/prepare-release.mjs`.

## [0.1.0-beta.1](https://github.com/ReimagineMedias/unflub/releases/tag/v0.1.0-beta.1) - 2026-11-18

### New

- **engine:** separate keyboard engine process, safer undo and signed updates (#13) ([7fabb51](https://github.com/ReimagineMedias/unflub/commit/7fabb51c77fe7e2ac38a2919d968f5f3f1ab694b))
- **app:** first-run tour, anonymous stats and on/off switch (#12) ([928e0d1](https://github.com/ReimagineMedias/unflub/commit/928e0d158643495a468f3b0c1bb3cc40c320161d))
- **app:** tray app with live settings and themes (#5) ([acede6b](https://github.com/ReimagineMedias/unflub/commit/acede6b23945ade7d62d927f347a380ede640cc3))
- **windows:** correction pill at the caret (#3) ([9999bde](https://github.com/ReimagineMedias/unflub/commit/9999bde8d14f12aae5aaf08b75f239cb44048dfb))
- **windows:** keyboard hook, injection and spell checker (#2) ([c356e5f](https://github.com/ReimagineMedias/unflub/commit/c356e5f43cedf5b6e85b9c7bab3a78c76c0c18d4))
- **core:** bundled autocorrect engine (#1) ([53c87e7](https://github.com/ReimagineMedias/unflub/commit/53c87e72224af6fabe385763b34dd2b8c0605eb4))
- **site:** unflub.rmgn.net with seo pages and turnstile downloads (#7) ([3d57bc5](https://github.com/ReimagineMedias/unflub/commit/3d57bc5ff08018232bcd3dcb9d862c5f0fea2c43))

### Fixes

- **release:** prepare-release survives commands that print to the terminal (#23) ([e6b8fe2](https://github.com/ReimagineMedias/unflub/commit/e6b8fe249f356f91eb31d1f98f2506b7462fc8f6))
- **release:** a first beta may precede the version in the files (#22) ([6d48636](https://github.com/ReimagineMedias/unflub/commit/6d48636951b9b29873eef76076b1673d4c07e4e2))
- **release:** keep release files apart from the repo's assets folder (#21) ([f637504](https://github.com/ReimagineMedias/unflub/commit/f637504f3792ad9482e8bd94597348c1a63dcbf0))
- **release:** find the engine where a native build puts it (#20) ([212346d](https://github.com/ReimagineMedias/unflub/commit/212346d33a5d0f054fe390c41750b1fef562ff33))
- **site:** download fallback and turnstile notice (#9) ([7f232b8](https://github.com/ReimagineMedias/unflub/commit/7f232b83137df1e7d9b9fffb9b756e4702aaafde))

Plus 6 internal changes (refactoring, docs, CI).
