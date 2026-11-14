# Worktrees and checkpoints

Inside a git repository every job runs in its own worktree, on its own branch
`swarmux/<session>/<n>`, under `~/.swarmux/worktrees`. Two jobs never edit the
same files, and each gets a block of ports so dev servers don't collide:
`SWARMUX_PORT_BASE` is the first port, `SWARMUX_PORT_COUNT` how many are yours
(also `SWARMUX_WORKTREE`, `SWARMUX_BRANCH`, `SWARMUX_JOB`). Files listed in
`copy` (by default `.env`, `.env.local`, `.claude/settings.local.json` and
`CLAUDE.local.md`, Claude Code's personal instructions file, which is
usually git-ignored) are
copied from the repository into each worktree when they exist; they are never
committed by swarmux. Outside a git repository nothing changes: the job runs
in the session's folder. A session can be set to `on`, `off` or `auto` (the
default: on inside a repository).

A job of a [workspace](workspaces.md) session gets a worktree set instead:
one worktree per repository it touches, under a task folder laid out like
the workspace.

When the done-gate passes, and before Claude Code compacts its context,
swarmux takes a checkpoint: the Claude session id, a commit holding the
worktree's files, the ledger entry that records it, and a handoff (goal,
state, what failed, next step). The handoff is handed to the session that
starts next, so a resumed or compacted session picks up where the last one
stopped. A job interrupted by quitting swarmux continues in its own worktree
and Claude session when you retry it; at most the step that was running is
lost. Reclaiming a worktree first commits what is left in it, so the branch
always keeps the work.

```toml
[worktree]
# root = "D:/swarmux-worktrees"            # default: ~/.swarmux/worktrees
copy = [".env", ".env.local", ".claude/settings.local.json", "CLAUDE.local.md"]  # globs allowed in the file name
port_base = 20000     # first port handed out
port_block = 20       # ports per job
port_slots = 500      # jobs that can hold a block at once
stale_hours = 12      # worktrees of jobs that ended this long ago are reclaimed
```

```sh
swarmux worktree list                 # worktrees, their ports, branches and state
swarmux worktree gc [--dry-run]       # reclaim stale worktrees (also runs at start and hourly)
swarmux worktree checkpoints [JOB]    # checkpoints of a job, or the newest ones
swarmux worktree mode <session> on|off|auto
```

