# Done-gate

An agent's "done" is not taken on its word. Every `claude` job gets `Stop`
and `SubagentStop` hooks; when the agent tries to finish, swarmux:

1. **Runs the repository's checks** itself, in a clean copy of the work (see
   [Independent verifier](verifier.md); outside a
   git repository, in the repository root), each as a child process: the
   program and its arguments directly, never through a
   shell, with no console window, killed (with everything it started) at its
   timeout, output kept to 64 KB. A check that fails is run once more; passing
   then marks it *flaky* instead of failing the gate. Checks run only when
   the session used a tool that can change files since the last pass.
2. **Compares protected files with their hashes** from the start of the
   session, so an edit that got past the Guard is still caught.
3. **Looks for evidence**: if the last message says the tests pass or names a
   test command, that command's latest run in the job's stream must have
   succeeded.
4. **Reads the diff for weakened tests**: deleted tests, switched-off tests,
   looser assertions, edited snapshots, wider tolerances, relaxed CI. A
   finding blocks the stop as *tests weakened*, with the lines that show it
   (see [Independent verifier](verifier.md#test-weakening-detector)).
5. **Reads the diff for the harness that runs the tests**: a `conftest.py`
   planted, a `.cargo/config.toml` runner, a `build.rs`, a jest config, a
   `package.json` test script, a `Makefile`, the CI pipeline. Every one
   created, changed, moved or deleted blocks the stop as *test harness
   changed* unless the repository allows it (see [The test
   harness](verifier.md#the-test-harness)). New or changed dependencies are
   listed.
6. **Runs the checks against the original tests**: the test and protected
   files the job changed, and the harness it changed, are put back as the
   job's starting commit had them (what the job created is removed), and the
   checks run again, then the hidden checks, in a copy made again from what
   the agent left. Passing requires the tests that existed when the job began,
   unmodified (*base suite*).

Passing also means the gate could do all of that. If it could not, the verdict
is **Escalate**, never Pass: `gates.toml` that cannot be read, and, for a
mission, hidden checks that exist and did not run, or checks that ran in the
worktree instead of a clean copy (see [When the gate cannot
vouch](verifier.md#when-the-gate-cannot-vouch)). When a plain job (a session
you were watching) ends, the gate runs once more unless its last stop passed on
the tree it left, and a job that cannot be verified is shown as `unverified`.

Anything wrong blocks the stop: the agent continues with a short digest (the
failing check, its first failing lines verbatim, the changed protected files,
the claims nothing backs). After `max_blocks` blocks the agent is let go and
the job shows up in *Needs you*. Every verdict is recorded in the audit
ledger and shows as chips on the job's wall card.

Writes to **protected paths** are refused by the Guard before any rule is
looked at: `Edit`/`Write`/`MultiEdit`/`NotebookEdit` on them, and shell
commands that redirect into them or remove, move, copy over or edit them in
place. Reading and running them is fine. `.swarmux/**` (the gate's own
configuration) is always protected.

Without a `.swarmux/gates.toml` the checks are guessed from the files at the
repository root:

| Found | Check |
| --- | --- |
| `Cargo.toml` | `cargo test --workspace --quiet` |
| `go.mod` | `go test ./...` |
| `package.json` with a real `test` script | `pnpm test` with `pnpm-lock.yaml`, else `npm test` |
| `pytest.ini`, `conftest.py`, or pytest settings in `pyproject.toml` / `setup.cfg` / `tox.ini` | `pytest -q` |

`.swarmux/gates.toml` (commit it with the repository):

```toml
[gate]
max_blocks = 3            # blocks before the user decides (0: never block)
evidence = true           # claimed test runs must appear in the stream
subagent_checks = false   # subagents' stops: protected files and evidence only
detect = true             # with no [[check]], guess the checks as above
protected = ["tests/oracles/**", "golden.json"]   # relative to the root; a bare name matches anywhere

[[check]]
name = "tests"
run = ["cargo", "test", "--workspace"]   # a list of arguments: no shell
timeout_secs = 600

[[check]]
name = "lint"
run = ["cargo", "clippy", "--", "-D", "warnings"]
required = true           # false: a failure is reported and blocks nothing
level = "fast"            # "fast" | "full" | "deep": from which [gate] level it runs (default "full")
env = ["DATABASE_URL"]    # environment variables it may see besides the allowlist (never credentials)

[verifier]
clean = true              # run the checks in a clean copy (false: in the worktree)
test_edits = "block"      # "block": weakened tests block the stop; "allow"
base_suite = true         # run the checks again against the original tests
allow_test_changes = ["tests/legacy/**"]   # test changes that are intended: not reported, not put back
substrate = ["scripts/ci/**"]    # more harness files than the built-in list
allow_substrate = ["Makefile"]   # harness files this repository changes on purpose
revert_probe = false      # revert the non-test part of a change and require a failing check
worktree_checks = "escalate"     # "escalate" | "allow": a mission whose checks ran in the worktree
dependencies = "flag"     # "flag" | "block": new or changed dependencies
```

The repository root is the nearest folder with a `.swarmux/gates.toml`, else
the nearest with `.git`. A `Stop` hook may take up to 30 minutes; a gate
still running then lets the agent stop.

```sh
swarmux gate show            # the gate that applies here: checks, protected paths, limits, the policy hash
swarmux gate lint            # what is wrong with gates.toml, and what fell back to a default; --strict fails on warnings
swarmux gate run             # run the checks now, as at a stop (recorded in the ledger); exits 1 unless the gate passes
swarmux gate run --dir ../x  # for another repository
swarmux gate run --in-place  # in the folder itself: no clean copy, no hidden checks
swarmux gate run --level deep  # run the checks up to that level (fast, full, deep)
swarmux gate selftest        # plant a harness file and delete an assertion in scratch copies: the gate must refuse both
```

A `gates.toml` that cannot be read (a syntax error, an unknown key, a value of
the wrong kind) used to read as an empty gate with a note. It now makes every
verdict Escalate with the parse error until it is fixed, so a typo cannot
quietly turn the gate off. The keys added with the harness check are read one
by one: a value that cannot be used falls back to its strict default with a
warning (see [Configuration](verifier.md#configuration)).

```text
$ swarmux gate lint
✗ .swarmux/gates.toml: unclosed table, expected `]`
  The gate cannot pass until this is fixed: every verdict is escalate.
```

