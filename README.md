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
- **Private.** Runs offline. No accounts, no telemetry, no cloud.
- **Yours.** Pick the engine, how confident it must be, which apps it skips, how the
  correction pill looks, and your own words and shortcuts.

## How it works

See [docs/architecture.md](docs/architecture.md).

## Building

Requirements on Windows: Rust (stable) and the Visual Studio 2022 Build Tools with the
"Desktop development with C++" workload.

```sh
cargo test --workspace
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Code is MIT licensed. The unflub name and logo are trademarks of RMGN; see
[TRADEMARKS.md](TRADEMARKS.md).
