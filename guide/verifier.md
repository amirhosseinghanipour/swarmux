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
- **Tamper scan.** The change since the job started is read for lines of
  existing tests removed or changed (moved lines don't count, new tests are
  fine) and for tests switched off in code: `#[ignore]`, `@pytest.mark.skip`,
  `t.Skip`, `it.skip`, `.only`, `@Disabled` and the like. Either blocks the
  stop unless `[verifier] test_edits = "allow"`.
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

