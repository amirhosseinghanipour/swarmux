# Independent verifier and hidden checks

An agent that can see the referee can game it: edit the test until it
passes, switch it off, special-case the one input it checks, or leave
something behind that makes the checks pass without the code being any
better (a `conftest.py` that forces every test to pass, a cargo runner that
runs nothing). So the gate's checks are not run where the agent works, and
what runs them is part of what is checked.

- **A clean copy, owned by swarmux.** At each judged stop, everything the
  worktree holds (committed or not, but nothing git ignores) is written into
  a commit of its own through a throwaway index, and that commit is checked
  out under `~/.swarmux/verify/<repo>/`. The agent's files, index and branch
  are not touched; build output, caches and files planted in ignored folders
  never get there. Copies of one repository share a cargo build folder, so a
  check after the first is an incremental build. The copy is removed after
  the checks.
- **The harness is checked too.** Files that decide how tests are found,
  built and run (`conftest.py`, `.cargo/config.toml`, `build.rs`, jest and
  vitest configuration, `package.json` scripts, a `Makefile`, the CI
  pipeline, and the rest of the list in [The test
  harness](#the-test-harness)) are in the diff like any other file, and the
  weakening detector does not read them. Every one the job created, changed,
  moved or deleted is a finding, and blocks the stop as *test harness
  changed* (a **P1** item in *Needs you* if the gate gives up).
- **The base suite and the hidden checks run on a copy made again.** After
  the visible checks, the copy is made again from what the agent left (what
  running the checks wrote into it, a build script that rewrote the source,
  is gone), the harness and the tests the job changed are put back as the
  commit the job started from had them (a file it created is removed), and
  the agent's other changes stay. The base suite runs there, then the hidden
  checks, after the tests the agent added are removed from that copy. A
  planted harness file can make the visible checks pass; it is not there when
  the hidden checks run. When the harness cannot be put back, the hidden
  checks do not run, and the verdict says so (see [When the gate cannot
  vouch](#when-the-gate-cannot-vouch)).
- **Hidden checks.** Kept in `~/.swarmux/holdout/<repo>/<check>/`, outside
  every repository. After the visible checks pass or fail, each hidden check's
  files are copied into the clean copy and it runs there. The agent, the
  evidence bundle and the ledger get only how many ran and how many failed;
  never a name, a command, a file or a line of output. A tool call that names
  `~/.swarmux/holdout` or `~/.swarmux/verify` is refused by the Guard, reads
  included, as is a shell line that runs the `swarmux holdout`, `rules` or
  `knowledge` commands; this is text matching, see the limits below.
- **Oracle pinning, twice.** Protected files are compared with their hashes
  from the start of the session in the worktree, and, through git, the clean
  copy is compared with the commit the job started from, so a committed edit
  or a protected file moved away counts too. Each hidden check's files are
  pinned by SHA-256 when added; a file that no longer matches fails its check.
- **Test-weakening detector.** See the section below. It blocks the stop as
  *tests weakened* unless `[verifier] test_edits = "allow"`.
- **Base suite.** After the checks pass, the files the job changed that are
  tests or protected are put back in the clean copy as the job's starting
  commit had them, and the checks run again over the agent's code. If the
  code only passes its own edited tests, this fails. The wall card shows
  `base suite ok` or `base suite failed`, and the result is its own entry in
  the audit ledger (`base_suite`). It runs only when the job changed such a
  file and everything else passed; `[verifier] base_suite = false` turns it
  off.
- **Told up front.** A mission's prompt says how it will be judged: no
  editing or weakening tests, no skips, no special-casing, and that some
  checks are hidden.

The wall card and Focus's gate lane show `hidden 2/3` and `tests tampered`.
When hidden checks exist for the repository and did not run, they show
`hidden: not run (reason)` instead, and the Work screen's *hidden checks*
line says `hidden checks: not run (reason)`: a missing chip never stands for
"none ran". `swarmux verify` runs a bundle's hidden checks again (all must
pass).

```sh
# a hidden test file and the command that runs it; DEST=SOURCE, DEST relative to the repo root
swarmux holdout add . bigger-inputs --file tests/hidden_add.rs=../keep/hidden_add.rs -- cargo test --test hidden_add
swarmux holdout add my-repo api-contract --timeout 300 -- npm run contract
swarmux holdout list          # every repository's hidden checks (for you, not the agents)
swarmux holdout list .        # this repository's
swarmux holdout rm . bigger-inputs
```

`<repo>` is a folder inside the repository (a worktree counts as its main
repository) or a name from `swarmux holdout list`. Keep the source of a
hidden file out of the repository: anything in the worktree, the agent can
read.

### How hidden checks are kept per repository

A repository is known by its folder name **and what it is**: a hash of its
remote URL and its root commit, like `shop.id-3fa9c1d2e4b7`. Two repositories
in folders of the same name do not share hidden checks, and a worktree or
a second clone of one repository does. A repository with no commit yet has
no identity and keeps the plain folder name.

Earlier versions kept hidden checks under the folder name alone
(`~/.swarmux/holdout/shop/`). The first time a repository of that name is
looked at (by the gate, `swarmux holdout` or `swarmux verify`), that folder is
**moved** to its identity key, the move is recorded in `holdout/.migrated`
and as a `policy` entry (`holdout_migrated`) in the audit ledger. A name is
handed out once: if two repositories had shared a folder name, the second one
starts with no hidden checks of its own, and `swarmux holdout list` is where
to see which key has them. `swarmux holdout add shop ...` with a bare name
finds the one repository that took the name over, and asks for a folder when
there are several.


## The test harness

A weakened test shows in the diff of the test. A changed harness does not:
the same tests pass, because something that runs them was changed. The list
below is data. These are the built-in defaults (a bare name matches at any
depth); extend them with `[verifier] substrate = [...]`, and say which you
change on purpose with `allow_substrate`.

| Ecosystem | Files |
| --- | --- |
| Python | `conftest.py`, `pytest.ini`, `pyproject.toml`, `setup.cfg`, `setup.py`, `tox.ini`, `noxfile.py`, `.coveragerc`, `sitecustomize.py`, `usercustomize.py`, `*.pth` |
| JavaScript, TypeScript | `jest.config.*`, `jest.setup.*`, `vitest.config.*`, `vitest.setup.*`, `karma.conf.*`, `.mocharc*`, `playwright.config.*`, `cypress.config.*`, `package.json`, `babel.config.*`, `.babelrc*`, `tsconfig*.json` |
| Rust | `build.rs`, `.cargo/config.toml`, `.cargo/config`, `Cargo.toml`, `rust-toolchain`, `rust-toolchain.toml` |
| Go | `go.mod`, `go.work` |
| Dart, Flutter | `dart_test.yaml`, `pubspec.yaml` |
| General | `Makefile`, `makefile`, `GNUmakefile`, `justfile`, `Taskfile.y*ml`, `.github/workflows/**`, `.gitlab-ci.yml`, `.circleci/**`, `azure-pipelines.yml`, `Jenkinsfile`, `.githooks/**`, `.husky/**`, `.pre-commit-config.yaml`, `.gitattributes`, `.npmrc`, `.yarnrc*`, `pip.conf`, `.pypirc` |
| Your gate | any `[[check]]` whose program is a path inside the repository (`./scripts/test.sh`), and `[verifier] substrate` |

A created, changed, moved or deleted file of the list is a finding unless it
matches `allow_substrate` or `allow_test_changes`. The finding reads like
`conftest.py: harness file created (pytest runs it before every test)`.

Manifests (`Cargo.toml`, `package.json`, `pyproject.toml`, `pubspec.yaml`,
`go.mod`) are both harness and where dependencies are declared, so a change
is read, not just flagged: one that only adds or bumps dependencies, or
changes the package's own metadata (`version`, `description`, `authors`, the
licence), is not harness tampering. Anything else in them is: a `[patch]` or
`[replace]` section, a `build =` script, a `[[test]]` with `harness = false`,
a `[profile]`, a pytest entry point or `addopts`, a `scripts.test` or `jest`
key, a `replace` directive in `go.mod`, a `dependency_overrides` section. If
the content cannot be read, the change counts as harness.

The scan is text, as the weakening detector is. It knows the files above and
the shapes named; a harness that is not on the list, or one this does not
recognise, is not seen, which is what `substrate` and the hidden checks
(run with the harness put back) are for.

### Dependencies

New or changed entries in a manifest or lockfile (`Cargo.toml` and
`Cargo.lock`, `package.json` with `package-lock.json`, `pnpm-lock.yaml` or
`yarn.lock`, `pyproject.toml`, `requirements*.txt`, `poetry.lock`,
`uv.lock`, `Pipfile.lock`, `go.mod`/`go.sum`, `pubspec.yaml`/`pubspec.lock`)
are listed with the package names, in the digest, on the wall card
(`dependencies changed`), in the ledger and in the bundle. By default they
do not block (`[verifier] dependencies = "flag"`); `"block"` makes them. No
network lookup is made: a package is named, not vetted.


## When the gate cannot vouch

Passing means the checks that were meant to judge the work did, as they
should. When one did not, the verdict is **Escalate**, never Pass, and a
person decides.

- **`gates.toml` cannot be read.** A syntax error, an unknown key, a value of
  the wrong kind. Every verdict, for an agent's stop and for a mission, is
  Escalate with the parse error until it is fixed; a mission still runs its
  acceptance checks, and the verdict is still Escalate. (`swarmux gate lint`
  says what is wrong.)

For a **mission**, which nobody is watching while it runs, three more
things keep the verdict at Escalate even though nothing is wrong with the
change:

- **Hidden checks exist and did not run.** The verdict says `hidden checks:
  not run (reason)`, in the bundle and on the screens. (A stop that somebody
  is attending still passes, with that note.) They do not run when there is
  no clean copy (`clean = false`, a check that names a `dir`, a copy that
  could not be made) or when the harness could not be put back.
- **The checks ran in the worktree.** Whenever the clean copy was not used,
  the bundle records `ran_in = "worktree"` and the mission is not passed.
  A repository that accepts that says so in `[verifier] worktree_checks =
  "allow"`; the record still says where they ran. A workspace mission whose
  checks name a folder per repository runs there, so it escalates until that
  is set (for a workspace, in the workspace root's `.swarmux/gates.toml`, see
  [Workspaces](workspaces.md)).
- **The change could not be read** (no diff): the tamper and harness scans
  did not happen.

**A plain job that ended** gets its gate run once more when its last stop was
not a pass on the tree it left (or it never had one). It is judged as a stop
is, attended: the three things above are noted, not escalated. A job that
cannot be verified at all shows `unverified: <why>` in *Needs you*.

## Check semantics

```toml
[gate]
level = "full"              # "fast", "full" (default) or "deep": checks above it are not run

[[check]]
name = "unit"
run = ["cargo", "test", "--workspace"]
required = true             # false: a failure is reported and blocks nothing
level = "full"              # "fast" | "full" | "deep": the lowest gate level that runs it
on_error = "fail"           # the only value: a check that cannot run never passes
env = ["DATABASE_URL"]      # environment variables this check may see (see below)
```

A check above the gate's level is not run, and the record says `check <name>
not run (level deep is above the gate's full)`, so the planned list and the
run list can differ and the difference is visible. `swarmux gate run --level
deep` runs more. `on_error` accepts `"fail"` only: any other value is
ignored with a warning, because a check that cannot run and passes anyway is
a fail-open gate.

A failing check is run once more; if it then passes it is shown as *flaky*
and counts as passed, never run a third time. The ledger keeps what each
check did, and a check that flaked `flaky_after` times in its last
`flaky_window` runs (3 of 10 by default) is called flaky on the wall card and
in the *Needs you* item of a job whose gate gave up, so a flaky check is not
blamed on the agent.

### What a check can see

A check is run with a clean environment: `PATH`, the home and temporary
folders, locale, the platform's own variables and the toolchains' (`CARGO_HOME`,
`RUSTUP_HOME`, `GOPATH`, `VIRTUAL_ENV`, `JAVA_HOME`, ...) and nothing else.
Variables named in the check's `env` or in `[verifier] env` are added, except
what no configuration can pass: `SWARMUX_*`, `ANTHROPIC_*`, `CLAUDE_*`,
`OPENAI_*`, `AWS_*`, `GITHUB_*` and the other providers' names, and anything
ending `_TOKEN`, `_SECRET`, `_PASSWORD`, `_API_KEY`. A name like that in an
`env` list is dropped with a warning. Agent code runs inside the checks
(they test it), so what a check sees, that code can read.

## Revert probe and selftest

```toml
[verifier]
revert_probe = true
```

With the probe on, a change that passes is run once more in a copy where its
non-test part is reverted but its tests stay. At least one check has to fail
there: the tests the job added or changed depend on the change. A change with
non-test files and no added or changed test fails the probe (nothing shows
the change is needed); so does one whose tests pass with the change
reverted. It works at the level of checks, not of single tests: it cannot
tell which test failed, and a repository whose tests already failed before
the job (the usual bug fix) shows failures that were there already. It is off
by default because it doubles a verification's cost.

`swarmux gate selftest [folder]` is the canary. In scratch copies of the
repository (its files are never touched) it makes the changes an agent that
wanted its work to count would make: it plants a harness file that forces the
tests to pass (a `conftest.py`, or a `.cargo/config.toml` runner, or a jest
config, by what the repository is) and deletes an assertion from a test. It
judges each copy with the repository's own `gates.toml` and exits 1 if the
gate lets one through, with why (a glob in `allow_substrate`, `test_edits =
"allow"`). The result is a `policy` entry (`gate_selftest`) in the ledger,
with the hash of the gate configuration that judged. A gate that has never
been seen to say no proves nothing by saying yes.


## Configuration

All of it is in `.swarmux/gates.toml`. The Guard holds an agent's tool-call writes to it (denies them for an unattended job); that is text matching, see [the limits](#what-this-does-not-establish). The keys
below are read one by one: a value that cannot be used falls back to its
default (strict) with a warning shown in the digest, in the ledger and by
`swarmux gate lint`, never a crash. Keys that existed before keep their
behaviour: a syntax error or an unknown key is a malformed file, and the
gate escalates.

```toml
[gate]
level = "full"

[verifier]
clean = true                        # run the checks in a clean copy
test_edits = "block"                # "block" | "allow"
base_suite = true
allow_test_changes = ["tests/legacy/**"]
substrate = ["scripts/ci/**"]       # more harness files, besides the built-in list
allow_substrate = ["Makefile"]      # harness files this repository changes on purpose
revert_probe = false                # revert the non-test part and require a failing check
worktree_checks = "escalate"        # "escalate" | "allow": a mission whose checks ran in the worktree
dependencies = "flag"               # "flag" | "block": new or changed dependencies
env = ["DATABASE_URL"]              # environment variables every check may see
flaky_after = 3                     # flaked this many times...
flaky_window = 10                   # ...in this many runs: called flaky (0: off)
```

| Key | Value | An invalid value |
| --- | --- | --- |
| `[gate] level` | `fast`, `full`, `deep` | `full` |
| `[[check]] required` | `true`, `false` | `true` |
| `[[check]] level` | `fast`, `full`, `deep` | `full` |
| `[[check]] on_error` | `"fail"` | ignored (`fail`) |
| `[[check]] env`, `[verifier] env` | list of names | bad entries left out; credentials refused |
| `[verifier] substrate`, `allow_substrate` | list of globs (no `..`) | bad entries left out |
| `[verifier] revert_probe` | `true`, `false` | `false` |
| `[verifier] worktree_checks` | `escalate`, `allow` | `escalate` |
| `[verifier] dependencies` | `flag`, `block` | `flag` |
| `[verifier] flaky_after`, `flaky_window` | numbers | `3`, `10` |

Every accepted key is enforced; a key this version does not know is an error
(the gate escalates), not an ignored line.

## What this does not establish

- **The agent's code runs in the verifier.** Checks test the code the agent
  wrote, so that code runs, as your OS user, in the copy, with hidden check
  files placed beside it. A harness put back from the base does not stop the
  code under test from reading its neighbours, and nothing here is a
  sandbox. The hidden checks are kept from the agent's tools and from its
  text, not from code that runs in the copy.
- **Text matching.** The harness list, the manifest reading, the weakening
  detector and the Guard's fence on the holdout folders read paths and text;
  a harness that is not on the list is not seen.
- **Same user.** swarmux, the checks and the agents run as you. The ledger
  and its key are not out of the agents' reach (see [the ledger](ledger-and-evidence.md)).
- **A passing gate is checks passing**, run as described, on the files that
  were there. It is not proof that the code is correct, and a bundle says
  what ran, not that nothing was missed. The checks are the ones you wrote or
  swarmux guessed; a check that does not exist is not run.
- **Hidden checks are finite.** They reduce the ways an agent can pass by
  gaming the visible checks; they do not remove them. Read `hidden 2/3`, never
  a bare tick.
- **The reviewer is a model.** Its concerns are an opinion shown below the
  deterministic results, and it never changes a verdict.


## Test-weakening detector

An agent graded by tests can make the tests easier instead of the code
better. The detector reads the job's diff against the commit it started from
(no model, no tokens) and names what it finds, each with the line that shows
it:

| Finding | What it looks for |
| --- | --- |
| test file deleted | a test file removed, unless its lines live on elsewhere |
| tests removed | fewer test declarations in a file than before (`#[test]`, `def test_`, `it(`, `func Test`, `testWidgets(`, `@Test`) |
| existing tests changed | lines of an existing test removed or changed |
| a test was switched off | `#[ignore]`, `@pytest.mark.skip`/`xfail`, `it.skip`/`.only`/`xit`, `t.Skip`, Dart `skip:`, `@Disabled`, and the like |
| assertions dropped | fewer assertions in a changed test file |
| assertion loosened | `assert!(true)`, `assert True`, `toBeTruthy`, `expect.anything()` and similar added |
| snapshot or golden file changed | `.snap`, `__snapshots__`, `golden`, `.golden` files edited or deleted (new ones are fine) |
| tolerance relaxed | `abs_tol`, `rtol`, `atol`, `epsilon`, `delta`, `approx(.., abs=)` made larger |
| test or CI configuration changed | a test step removed from a workflow, `continue-on-error`, `\|\| true`, a lowered `fail_under`, a changed `"test"` script |

It is a heuristic over text, tuned to stay quiet on honest work: a renamed
file, a new test, a test moved to another file and an assertion rewritten
with the same strength are not findings. It will not catch everything; the
harness check, the base suite and the hidden checks are the other layers.

When the gate gives up, the item in *Needs you* is **P1** if the finding
touches what the mission's checks run (a protected path, a path a check
names, or any test file when checks exist) or a harness file, else **P2**
(for instance only CI configuration).

Say that a test change is intended in `.swarmux/gates.toml`, which the Guard
keeps agents' tool calls from writing. Matching paths are not reported and not put back for the base
suite:

```toml
[verifier]
allow_test_changes = ["tests/legacy/**", "old_api_test.go"]
```

## Review step

Press `v` on the Dashboard to have the next jobs reviewed (or set
`[review] enabled = true`). When a reviewed job succeeds, swarmux queues a
second, lean, read-only run in the same folder: `claude -p --model <review
model> --tools Read,Grep,Glob --no-session-persistence --setting-sources
project,local --strict-mcp-config`. It gets the prompt, the job's answer
and the `git diff` (uncommitted changes in tracked files, capped at 48 KB)
and must answer `{"verdict": "pass" | "concerns", "concerns": [...],
"diffs_checked": n}`. The verdict and its cost are stored with the job and
shown on the Dashboard and in the Ledger. A review that fails or answers
something else shows as `review failed: …`; it never changes the job's own
result.

The reviewer is gated by risk: a change is scored the way the inbox scores a
tool call, and only an elevated one gets a reviewer. That is a big change
(around 300 lines or more), one that deletes files, touches builds,
dependencies, pipelines, migrations, secrets or `.claude/`, or edits or
switches off existing tests. A routine change shows `review skipped` with
the score and why; set `when = "always"` to review every job.

```toml
[review]
enabled = false   # review new jobs by default (v toggles it while running)
model = "opus"    # the reviewer's model
when = "risk"     # "risk": only changes whose risk is elevated; "always": every job
```
