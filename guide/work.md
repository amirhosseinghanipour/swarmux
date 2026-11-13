# Work (`g w`)

Work is where you decide and land. The left column is a ranked queue of
outcomes (a mission, or a plain job that belongs to none); the right pane
shows the selected one, with the work product beside the decision. It reads
what swarmux already keeps, and shows nothing it has no data for.

Go to it with `g w` (a chord: `g`, then `w`). The tab bar shows how many
decisions wait: `g w work 3`.

## The queue

| Group | What is in it |
| --- | --- |
| **DECIDE** | outcomes that need you: a held tool call, a failed job, a gate that gave up, an escalated mission, a plan to approve, a merge that can't happen, a reviewer's concerns, a build that strayed from its plan. Ranked P1, P2, P3, then by how soon a held call is denied |
| **READY TO LAND** | the gate passed and the work has not landed |
| **RUNNING** | a job is out for it, or it waits for a slot |
| **LANDED** | delivered with proof (the newest are listed, the rest counted) |

A decision's third line is its cost of waiting, from data that exists: when a
held call is denied if you don't answer (`auto-deny in 9:30`), and what waits
on it through the plan's task dependencies (`blocks A5 release (task
dependency)`). Below the queue, CAPACITY shows the 5-hour and 7-day usage
windows with the pace's forecast, and how many jobs the scheduler is running.

## The selected outcome

Four tabs, `1` to `4`:

1. **work product**: the header (state, mission, repository, model, spend,
   what it is done when); plan against drift (the planned tasks with their
   files, what changed outside the plan, planned tasks nothing touched; drift
   is reported, not blocking); the agent's claim quoted against the checks
   (each check's status, time and first failing line, then the gate's stages:
   protected pins, test-weakening, base suite, hidden checks as counts only,
   evidence gaps, drift, reviewer); the evidence (bundle, ledger range,
   signature); and what landing does: each repository into
   `swarmux/<mission>/integration`, producers first, all or none, your own
   branches untouched.
2. **stream**: the job's action stream, the same rows as [Focus](focus.md).
3. **diff**: the sealed diff's totals, and the session's uncommitted changes
   read when you open the tab. Per-file counts are not kept yet.
4. **checks**: every check and every stage of the gate, with what each found.

A held tool call shows the call, the rule that held it, its risk, how long
it has waited and when it is denied, then the agent's own words, quoted and
labelled as such (never interpreted), and the last steps up to the call. A
call the Guard refused shows the same, without a countdown.

## Actions

The ACTIONS panel lists what the outcome allows, with the key as your keymap
has it. An action that can't be taken now is shown dim, with the reason.

| Key | Does |
| --- | --- |
| `y` / `a` / `n` | allow once, always allow, deny the held call (the same answers as everywhere; an irreversible call still asks you to type its target, and is never always-allowed) |
| `y` / `n` | on a proposed plan: approve it, or send it back with a note |
| `l` | land the work in `swarmux/<mission>/integration` (needs `landing = "manual"`, see [Configuration](configuration.md#landing)) |
| `x` | stop the outcome's running job |
| `r` / `R` | queue the job's prompt again / continue an interrupted or failed job |
| `e` | open the evidence in the Ledger, on this job |
| `Enter` | open the job in Focus |
| `Up` / `Down`, `k` / `j` | select an outcome |
| `1` to `4` | the tabs |
| `PgUp` / `PgDn` | scroll the right pane |

On Work, `y`, `n` and `a` answer the call or plan of the **selected** outcome
only, never another; with nothing held on it they say so. Every key is an
action in the keymap (`work.land`, `work.kill`, `work.retry`, `work.resume`,
`work.evidence`, `work.tab_product`, ...) and can be rebound in
[`[keys.work]`](configuration.md#keys).

There is no send back with a note (a plan is sent back with its own note), no
pause, and no merge to main: landing is only ever into the mission's
integration branch.
