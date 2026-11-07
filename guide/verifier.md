# Independent verifier and hidden checks

An agent that can see the referee can game it: edit the test until it
passes, switch it off, or special-case the one input it checks. So the
gate's checks are not run where the agent works.

- **A clean copy, owned by swarmux.** At each judged stop, everything the
  worktree holds (committed or not, but nothing git ignores) is written into
  a commit of its own through a throwaway index, and that commit is checked
  out under `~/.swarmux/verify/<repo>/`. The agent's files, index and branch
  are not touched; build output, caches and files planted in ignored folders
  never get there. Copies of one repository share a cargo build folder, so a
  check after the first is an incremental build. The copy is removed after
  the checks.
- **Hidden checks.** Kept in `~/.swarmux/holdout/<repo>/<check>/`, outside
  every repository. After the visible checks pass or fail, each hidden check's
  files are copied into the clean copy and it runs there. The agent, the
  evidence bundle and the ledger get only how many ran and how many failed;
  never a name, a command, a file or a line of output. Tool calls that name
  `~/.swarmux/holdout`, `~/.swarmux/verify` or run `swarmux holdout` are
  refused by the Guard, reads included.
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
`swarmux verify` runs a bundle's hidden checks again (all must pass).

```sh
# a hidden test file and the command that runs it; DEST=SOURCE, DEST relative to the repo root
swarmux holdout add . bigger-inputs --file tests/hidden_add.rs=../keep/hidden_add.rs -- cargo test --test hidden_add
swarmux holdout add my-repo api-contract --timeout 300 -- npm run contract
swarmux holdout list          # every repository's hidden checks (for you, not the agents)
swarmux holdout list .        # this repository's
swarmux holdout rm . bigger-inputs
```

`<repo>` is a folder inside the repository (a worktree counts as its main
repository) or the repository's folder name, lowercased. Keep the source of a
hidden file out of the repository: anything in the worktree, the agent can
read.


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
base suite and the hidden checks are the other layers.

When the gate gives up, the item in *Needs you* is **P1** if the finding
touches what the mission's checks run (a protected path, a path a check
names, or any test file when checks exist), else **P2** (for instance only
CI configuration).

Say that a test change is intended in `.swarmux/gates.toml`, which agents
cannot write. Matching paths are not reported and not put back for the base
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

