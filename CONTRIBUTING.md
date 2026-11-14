# Contributing to swarmux

Thanks for helping. Bug reports, small fixes and focused features are all
welcome. For anything large, open an issue first so we can agree on the shape.

## Build and test

You need Rust 1.88 or newer and Claude Code (only for the tests that talk to the
real `claude`; most tests use a fake).

```sh
cargo build
cargo run -- doctor
```

Before you open a pull request, all three must pass:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Some test suites only build with the `fake-claude` feature (they drive the real
job, mission, worktree and workspace code against a fake `claude`), so the
commands above do not run them. **Run them before merging a change to missions,
gates, jobs, worktrees or the verifier**, and fix what they find:

```sh
cargo test -p swarmux-infrastructure --features fake-claude --no-fail-fast
cargo test -p swarmux --features fake-claude --no-fail-fast
```

Windows, Linux and macOS are all supported; CI runs the same checks. Write
code that does not assume a shell, a path separator or a tmux.

## Screenshots

The images in the README (`assets/screens/*.png`) are captured from
`swarmux --demo`, the scripted scenario, so they show no real data. One command
regenerates all of them:

```sh
scripts/screenshots.sh
```

It builds a release binary (or uses the one in `SWARMUX_BIN`), plays the demo in
a `tmux` pane of each size (180x50, plus 120x36 for the compact Deck), waits
for the scenario to finish, presses the keys that reach each screen, captures
the pane with its colours and draws it to a PNG with `scripts/render_ansi.py`.
It needs `tmux`, Python 3 with `pip install pillow` (and optionally
`fonttools`), a monospace font (JetBrains Mono, else DejaVu Sans Mono; or point
`SWX_FONT` and `SWX_FONT_BOLD` at TTF files), and runs on Linux or macOS. Set
`THEME` to capture another built-in theme, and `OUT` to write somewhere else.

Look at every image before you commit it. The held call's countdown and the
clock-driven figures differ by a little from run to run. If you change the demo
or a screen, regenerate the images in the same pull request, and keep each PNG
under about 150 KB.

## Architecture

The workspace is split in layers. Dependencies point inward only:

```
domain  <-  application  <-  infrastructure
                         <-  tui
```

- `crates/domain`: entities and pure rules. No I/O: no files, processes,
  clocks, network or SQL.
- `crates/application`: use cases and the ports (traits) they need. Ports live
  in the feature module that uses them, not in a shared catch-all.
- `crates/infrastructure`: adapters that implement the ports: SQLite, the
  `claude` and `git` CLIs, the system, the keychain.
- `crates/tui`: the terminal interface. It talks to application services.
- `src/main.rs`: the composition root. It is the only place that knows every
  crate and wires adapters to use cases.

Put new rules in the domain with unit tests, orchestration in the application
layer, and anything that touches the outside world behind a port.

## Database migrations

Migrations are append-only. Never edit or reorder a migration that has shipped;
add a new one at the end of the list in `crates/infrastructure/src/sqlite`.

## Pull requests

- Keep a pull request to one change, and add or update tests with it.
- Use conventional commit messages (`feat:`, `fix:`, `docs:`, `test:`,
  `refactor:`, `chore:`), with an optional scope such as `feat(tui):`.
- Regenerate `schema/` after changing a configuration key (`swarmux config schema
  --out schema`); a test fails when it is stale. Tag a compatibility shim with
  `COMPAT(name, version, date)` (see [COMPATIBILITY.md](COMPATIBILITY.md)).
- Update the matching page in `guide/` when behavior or a setting changes, and
  add a line to `CHANGELOG.md` under Unreleased.
- Do not commit secrets, local settings or personal files.

## Reporting security problems

Please do not open a public issue. See [SECURITY.md](SECURITY.md).

By contributing you agree that your work is released under the [MIT license](LICENSE).
