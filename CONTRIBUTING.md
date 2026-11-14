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

Windows, Linux and macOS are all supported; CI runs the same checks. Write
code that does not assume a shell, a path separator or a tmux.

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
