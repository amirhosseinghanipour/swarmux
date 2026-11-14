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

Inside DECIDE the order is priority first (P1, P2, P3), then the mission's
[due time](missions.md#due) if its contract has one (the earlier first; a
mission with none comes after the ones that have one, since a due time is only
a tiebreak after priority), then how soon a held call is denied, then who has
waited longest.

A decision's third line is its cost of waiting, from data that exists: the
mission's due time when its contract sets one (`due 16:00`, in your clock, with
the date when it isn't today, and how late once it has passed), when a held
call is denied if you don't answer (`auto-deny in 9:30`), and what waits on it
through the plan's task dependencies (`blocks A5 release (task dependency)`).
Nothing is shown for a due time the contract doesn't have.

Below the queue, CAPACITY shows the 5-hour and 7-day usage windows with the
pace's forecast, and how many jobs the scheduler is running, then how
verification has gone over the last hours:

```text
last 24h first-run 86% (6/7) · 9 verified · 0.4/h
         claim→verified 2m10s med · 9m20s p90
         41 checks · 3 sealed
```

These are worked out from the ledger's entries (`swarmux stats verification`
prints them too):

- **first-run** is the share of jobs whose first judged claim (the agent
  saying it is done) the gate passed. A job whose first claim the window began
  in the middle of is left out, not counted as a first run.
- **verified** counts the jobs the gate passed in the window, and `/h` is that
  over the time the ledger covers (a ledger younger than the window covers
  less).
- **claim→verified** is the median and the 90th percentile of the time from a
  job's first claim to the gate's pass. Both moments are ledger entries written
  as swarmux processes them, so a claim the gate passes at once measures about
  nothing; the span grows with every block and with the agent's rework in
  between.
- **checks** counts the check results the gate recorded; **sealed** counts the
  evidence bundles sealed.

A figure with nothing behind it (no claim was judged yet) is left out, never
shown as zero. `newest only` is added when the ledger is too long to read back
to the start of the window. The window is `metrics_hours` in
[`[console]`](configuration.md#console), a day by default.

## The selected outcome

Four tabs, `1` to `4`:

1. **work product**: the header (state, mission, repository, model, spend,
   what it is done when, and the due time if the contract has one); plan
   against drift (the planned tasks with their files and, under each, the files
   that changed in its areas with the lines git counted in them
   (`M src/invoice/round.rs +22 -5`), the files that changed outside the plan
   with their counts, and planned tasks nothing touched; drift is reported, not
   blocking); the agent's claim quoted against the checks
   (each check's status, time and first failing line (for a check that could
   not start, why it could not), then the gate's stages:
   protected pins, test-weakening, base suite, hidden checks as counts only,
   evidence gaps, drift, reviewer); the evidence (bundle, ledger range,
   signature); and what landing does: each repository into
   `swarmux/<mission>/integration`, producers first, all or none, your own
   branches untouched.
2. **stream**: the job's action stream, the same rows as [Focus](focus.md),
   and the conversation with its agent: what you said to it appears in the
   stream as `» you` rows, and under it **YOUR MESSAGES** lists each message
   with where it stands. See [Message a running agent](#message-a-running-agent).
3. **diff**: the sealed diff's totals and the files behind them with their
   counts (`A` added, `M` modified, `D` deleted; a binary file says `binary`
   instead of counts), and the session's uncommitted changes read when you
   open the tab, again with the files git counted before the text. A bundle
   sealed before per-file counts were kept shows its totals only and says so:
   the counts are never worked out afterwards. Before the evidence is sealed,
   the files are the ones counted when the build was checked against its plan.
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
| `s` | send the work back with a note, or deny a held call with one, see [Send back](#send-back) |
| `m` / `M` | message the running agent, see [Message a running agent](#message-a-running-agent); `M` switches the mode between queue and steer |
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
`work.evidence`, `work.send_back`, `work.message`, `work.message_mode`, `work.tab_product`, ...) and can be rebound
in [`[keys.work]`](configuration.md#keys).

There is no pause, and no merge to main: landing is only ever into the
mission's integration branch.

## Message a running agent

`m` tells the agent of the selected outcome's **running** job something, from
inside swarmux, without leaving Work. It opens the stream tab (the transcript so
far, as in Focus) and a line at the bottom of the screen. Type the message and
press `Enter` to send it, `Esc` to drop it; nothing is sent before `Enter`. A
panel above the line shows the mode, what the mode does, and what you typed.
`Tab` switches between the two modes while you type, and `M` switches the mode
for the messages after (`intervene_mode` in [`[console]`](configuration.md#console)
sets the one the line opens in). The row is dim, or `m` says why, when the job
is not running (an ended job is [sent back](#send-back) with a note instead),
when swarmux did not start its session (it began in an earlier run: open it in
a terminal with `t` in Focus to take over), for a reviewer, and for a task of a
task graph (its tasks are not messaged one by one).

Your words reach the agent the way a note does: the job's Claude session
**continues**, in the same working folder, with your message as its next
prompt. swarmux does not write into a running `claude`: a job is one turn, and
its input is closed after the prompt. That is what makes both modes simple to
test, and it limits what they can promise.

| Mode | What it does | What it does not guarantee |
| --- | --- | --- |
| **queue** (default) | keeps the message until the job's turn has ended, then continues the session with it | that the agent reads it *during* the turn: it does not. If the job fails, is interrupted or is escalated, the message goes with the resume. If you stop the job yourself (`x`) the message is **not** sent, and says so |
| **steer** | stops the turn now (the same stop as `x`: the `claude` process and what it started are killed) and continues the session with your message at once | that the agent's last step finished: a file edit or a shell command in flight is cut short, and the agent is told so and told to check the working folder. It asks for a second `Enter` first (`intervene_confirm_steer`) |

**A call held by the Guard** stays held, in both modes: a message approves
nothing. Steering is *refused* while a call of that job is held, since stopping
the agent would strand the call; answer it first (a denial can carry a note,
`s`), or queue the message. Queueing leaves the call as it is.

**Two or more queued messages** on one job go as one continuation, in order.
At most five wait on a job.

**What the receipt says.** After `Enter` the notice says what happened, and the
**YOUR MESSAGES** block keeps it up to date:

- *queued*: taken and recorded, waits for the turn to end;
- *handed to the agent as job #N*: the session continued; the agent has said
  nothing yet, so this is **not** called delivered;
- *delivered*: the agent's turn with your message started (swarmux saw its
  first report; the time is recorded);
- *not delivered*: with the reason (you stopped the job; the continuation
  ended before it reported; the mission was delivered first, or is no longer
  where a message can follow it). Your text is kept in the ledger to send again.

A mission's job is followed by the mission: when the turn ends the message waits
until the mission has taken that in (escalated or verifying), as a send back
does, and the gate judges the agent's next claim with the checks run again.

**What is not kept.** The queue and the receipts live in this run of swarmux. A
message still waiting when swarmux stops is not delivered (the job is marked
interrupted: resume it, or send it back with a note). The `» you` rows and the
receipts are shown for this run only; the ledger has the record.

Each message is an `operator` entry in the [ledger](ledger-and-evidence.md#what-you-told-an-agent),
written **before** anything is sent: a message that cannot be recorded is not
taken and nothing happens. It is your authority, not the agent's output; the
Guard is unchanged and keeps judging the agent's tool calls. Prompting a
session from Focus (`p`) is unchanged: it queues a new job.

## Send back

`s` sends the outcome back to its agent with a note you write. What it does
depends on what the outcome is:

- **Work that ended**: a job that stopped (failed, was interrupted or
  cancelled), a mission that was escalated, or work that finished and waits for
  you (verified, not accepted). The agent carries on in the same Claude session
  and the same worktree, with your note as its next instruction (the resume
  path, with your words as the prompt). If the job belongs to a mission, the
  mission follows the new job (escalated or verifying becomes queued again),
  and the gate judges the agent's next claim with the checks run again,
  whether or not it changed a file first.
- **A held call**: the call is denied, and your note reaches the agent as the
  reason (Claude Code shows a denial's reason to the agent), so it can change
  what it does instead of trying the same call again.

Nothing is sent by itself. `s` opens a line at the bottom of the screen
(`send job #12 back with a note`); type the note and press `Enter` to send it,
`Esc` to drop it. A panel above says where the note goes and what happens
next, and lists the suggested notes: `Tab` (or `Down`) puts the next one on
the line, `Shift+Tab` (or `Up`) the previous one. You can edit it or send it
as it is; nothing is sent until you press `Enter`. The suggestions are yours
to set: `send_back_notes` in [`[console]`](configuration.md#console) (an
empty list turns them off).

Where it can't work, the row is dim with the reason: the job is still running
(stop it first, or [message it](#message-a-running-agent)), the work is delivered (it has landed: start a new mission),
the mission runs as a task graph (its tasks are not sent back one by one), or
its budget is spent (raise it in the contract first, or the agent would only be
stopped again).

Each note is recorded in the [ledger](ledger-and-evidence.md): an entry of kind `operator`
with the actor `operator`, the time, your note, the job, where it stood
(`escalated`, `verifying`, `failed`, `held`, ...), and which job the agent
carries on as (or which call was denied). Like every entry it is chained, so
verifying the ledger covers it. A library of saved notes comes later; for now
the notes are free text.
