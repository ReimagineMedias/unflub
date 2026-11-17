# Contributing

Thanks for helping. A few rules keep the project fast and easy to review.

## Workflow

- Branch from `main` as `type/short-topic` (`feat/undo-learning`, `fix/42-caret-reset`).
- One pull request per outcome. Titles follow Conventional Commits with a scope:
  `feat(core): rank candidates by keyboard adjacency`.
- Fill in the pull request template: problem, change, verification. For anything visual, add
  before/after screenshots or a short recording.

## Code

- `unflub-core` stays platform-free and `#![forbid(unsafe_code)]`. OS calls belong in adapter
  crates.
- Nothing on the keyboard-hook path may allocate per key in the common case, block, or take
  more than 1 ms. Add a benchmark when you change it.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` must pass.

## Typos we miss or get wrong

Open an issue with the word you typed, what you meant, and the previous word. These become test
cases in `crates/unflub-core/tests/`.
