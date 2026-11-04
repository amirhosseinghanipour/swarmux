# Missions (screen 2)

A mission is an outcome contract: the goal, the checks that prove it, where
the agent may work, what it may never change and what it may cost. swarmux
keeps each one in `.swarmux/missions/<name>.toml` in the repository (agents
can't write there) and in its database:

```toml
title = "Fast search"
goal = "make search answer in under 50 ms on the bench corpus"

[acceptance]
checks = ["cargo test -p search", "sh scripts/bench.sh --max-ms 50"]

[constraints]
paths_rw = ["src/search/**"]
protected = ["tests/**", "scripts/bench.sh"]

[budget]
usd = 2.0
wall_minutes = 90
on_exhaust = "escalate"   # or "warn"
```

Saving a mission lints it at once: no checks (it can't run), checks whose
tests or scripts the agent could edit (`f` protects them), paths both
writable and protected, a goal too short to act on, no budget, no scope. One
Haiku call (no tools, no session file; `--bare` when `ANTHROPIC_API_KEY` is
set) adds at most three clarifying questions, which you answer inline. `r`
compiles the mission into a job for its session: the prompt states the goal,
the checks, the paths and your answers, and the job's done-gate gets the
mission's checks and protected paths on top of the repository's.

A mission goes `draft › linted › queued › running › verifying › delivered`.
It is delivered only when the gate passes after the job has ended; if no
stop of the job was judged, swarmux runs the checks itself. A failed job, a
gate that gave up or a spent budget escalates it; a mission nothing can check
is refused. Each card shows that path, a chip per check, the budget split by
phase (plan 10%, implement 60%, verify 20%, review 10%) and the topology
(one session for now). Every change of state is in the audit ledger.

## Planning first

A mission can plan before it builds. Press `p` on a card to switch it (or
set the default for every mission with `[missions] plan_first = true` in
`config.toml`). `r` then does not start the build: it asks Claude Code, in
plan mode with read-only tools, for a plan and a task graph. The plan comes
back as a card section and as an item in the inbox:

```
plan
status      waiting for you
Index first, then queries.
  t1 Build the index
    src/search/index.rs
  └ t2 Query path
      src/search/query.rs, tests/search_*.rs
y approve and build · n change (with a note) · c cancel
```

Each task has an id, a title, the files or folders it expects to touch, the
tasks it waits for and how it is checked. swarmux refuses a plan whose ids
repeat, whose dependencies don't exist or whose tasks wait on each other, and
asks again. You answer with one key, on the Missions screen or in the inbox:

| Key | Does |
| --- | --- |
| `y` | approve: the plan is stored, put on the audit ledger and the build starts with it in its prompt |
| `n` | change: type what is wrong and a new plan is asked for, which sees the old one and your note |
| `c` | cancel the plan; the mission stays as it was |

Planning is paid from the mission's plan share. Changing the contract
voids its plan.

When the build finishes, swarmux compares what changed with the approved
plan: files changed outside every planned area, and tasks none of whose areas
saw a change. Lockfiles and swarmux's own files don't count. When 30% or more
of the changed files are outside the plan, or 30% or more of its tasks were
never touched, the card says so, the mission shows what strayed, and an inbox
item asks you to look. The reviewer is told about the plan too, and reports
what it sees as concerns starting with `drift:`. The line is
`drift_threshold` (0 to 1, default 0.3); `plan_model` picks the model that
plans (default `sonnet`):

```toml
[missions]
plan_first = false
plan_model = "sonnet"
drift_threshold = 0.3
```

| Key | Does |
| --- | --- |
| `n` | new mission (for the selected session) |
| `e` | edit the selected mission and answer its questions (not while it is out) |
| `Tab` / `Shift+Tab` | next / previous field or answer in the composer |
| `Ctrl+S` | save and lint |
| `Ctrl+F` / `f` | protect the oracles the lint found |
| `Ctrl+R` / `r` | run it (or ask for its plan first) |
| `p` | plan first on / off for the selected mission |
| `y` / `n` / `c` | approve / change / cancel a proposed plan (also in the inbox) |
| `l` | lint again |
| `Enter` | open its job in Focus |
| `Esc` | close the composer |

