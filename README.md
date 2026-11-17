# unflub

**Phone-style autocorrect for every text field on your computer.** Free, open source, and
nothing you type leaves your machine.

> [!NOTE]
> unflub is in early development. Windows comes first; macOS and Linux follow on the same engine.

## Why

On a phone, a fast typo fixes itself. On a desktop, it gets a red underline and waits for you.
unflub fixes the word the moment you finish it, in any app: chat, browser, editor, terminal
prompts. Press Backspace right after a fix to put your word back, and unflub remembers.

- **Fast.** Native Rust. Corrections are decided inside the keyboard hook in microseconds.
- **Private.** No accounts, no telemetry, no cloud. The only connection it makes is a daily
  update check, which you can turn off.
- **Yours.** Pick the engine, how confident it must be, which apps it skips, how the
  correction pill looks, and your own words and shortcuts.

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
