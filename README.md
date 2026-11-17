# unflub

**Phone-style autocorrect for every text field on your computer.** Free, open source, and
nothing you type leaves your computer.

> [!NOTE]
> unflub is in early development. Windows comes first; macOS and Linux follow on the same engine.

## Why

On a phone, a fast typo fixes itself. On a desktop, it gets a red underline and waits for you.
unflub fixes the word the moment you finish it, in any app: chat, browser, editor, terminal
prompts. Press Backspace right after a fix to put your word back, and unflub remembers.

- **Fast.** Native Rust. Corrections are decided inside the keyboard hook in microseconds.
- **Private.** No accounts and nothing you type ever leaves your computer. See
  [Privacy](#privacy) for exactly what it does send.
- **Yours.** Pick the engine, how confident it must be, which apps it skips, how the
  correction pill looks, and your own words and shortcuts.

## Privacy

**The part of unflub that sees your keys has no network code. CI enforces it.** unflub is two programs: `unflub-engine.exe` sees
your keys and has no way to reach the internet, and `unflub.exe` (the tray icon) handles
updates and the rest without ever seeing a key. See
[docs/architecture.md](docs/architecture.md#two-processes).

- **What you type stays on your computer.** Words go to the engine and the correction pill,
  and to the Stats page on this computer, nowhere else.
- **The daily update check is anonymous.** It sends the unflub and Windows versions and a
  random number from 0 to 99 that changes with every version, so new releases can reach a few
  people first. No ID, nothing that tells your computer apart from anyone else's.
- **Usage stats and crash reports are on by default, and you choose on first run.** Once a day:
  how many fixes and undos, which settings, how fast, how often it crashed. Crash reports say
  where in unflub's code it crashed, with no file paths, names or text. Both have a switch in
  Settings > Privacy, which also shows the exact payload.
- **The Stats page is local.** It keeps which words unflub fixed, on this computer only, so you
  can see your top typos. Turn the list off or clear it on that page.
- Organizations can turn updates, stats, crash reports and remote config off with machine
  policy (`HKLM\Software\Policies\RMGN\unflub`, see
  [docs/architecture.md](docs/architecture.md#network-and-privacy)).

## How it works

See [docs/architecture.md](docs/architecture.md).

unflub stays light in the background: CI checks its startup time, per-key latency, idle memory
and CPU, also on a simulated low-end machine. See [docs/performance.md](docs/performance.md).

## Uninstall

Settings > Apps > Installed apps > unflub > Uninstall, or `winget uninstall RMGN.unflub` if you
installed it with winget.
This removes the app, its shortcut and start at login. To remove your settings and personal
dictionary too, tick **Delete the application data** in the uninstaller. Details, and how to
clean up by hand: [docs/releasing.md](docs/releasing.md#uninstalling).

## Building

Requirements on Windows: Rust (stable) and the Visual Studio 2022 Build Tools with the
"Desktop development with C++" workload.

```sh
cargo test --workspace
```

Releases are built by GitHub Actions from a version tag; see [docs/releasing.md](docs/releasing.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Code is MIT licensed. The unflub name and logo are trademarks of RMGN; see
[TRADEMARKS.md](TRADEMARKS.md).
