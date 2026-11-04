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

| Key | Does |
| --- | --- |
| `n` | new mission (for the selected session) |
| `e` | edit the selected mission and answer its questions (not while it is out) |
| `Tab` / `Shift+Tab` | next / previous field or answer in the composer |
| `Ctrl+S` | save and lint |
| `Ctrl+F` / `f` | protect the oracles the lint found |
| `Ctrl+R` / `r` | run it |
| `l` | lint again |
| `Enter` | open its job in Focus |
| `Esc` | close the composer |

