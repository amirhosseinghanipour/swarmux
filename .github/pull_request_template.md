## What and why

<!-- One change per pull request. Link the issue if there is one. -->

## Checklist

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Tests added or updated
- [ ] `guide/` updated if behavior or a setting changed
- [ ] `CHANGELOG.md` has a line under Unreleased
- [ ] Layering kept: domain has no I/O, ports live in their feature module, migrations are append-only
