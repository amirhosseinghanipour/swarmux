# Focus (`0`)

Focus is one agent in detail: the job you last sent, or the one you open
with `Enter` from Mission control, Missions, the queue or the ledger (`0` goes
back to it). It opens with the same sigil and colour the card had. From top
to bottom:

- **Header**: session, state, the prompt, model, permission mode, the job's
  branch and worktree (or its folder), time, cost and context.
- **Situation**: `doing`, `because`, `next`, how `sure` swarmux is and what it
  is `unsure` about. All of it comes from what swarmux saw: the last tool
  call, the todo list, the done-gate, the review, loop stops and failed tool
  calls. Nothing is guessed by a model.
- **Gate**: the done-gate's checks and the review as chips, then the
  checkpoints taken when the gate passed or before a compact, and the recovery
  trail (loop-breaker stops).
- **Timeline**: one row per actor, with your own prompts and answers marked.
- **Action stream**: tool calls as a tree with their results, subagents
  indented, diff lines tinted, and a "you were here" line where you last looked.
- **Answer**: the final report, formatted; `d` swaps in the working-tree diff.
- **Reviewer**: only when the job has a review. Windows from 220x60 add the
  agent's plan and its subagents.

| Key | Does |
| --- | --- |
| `Up` / `Down`, `PgUp` / `PgDn`, `Home` / `End` | scroll the stream (`End` follows the newest row) |
| `J` / `K` | scroll the answer or diff |
| `[` / `]` | step through the job's checkpoints |
| `d` | working-tree diff in place of the answer |
| `t` | open the session in a new Windows Terminal tab (`wt -w 0 nt -d <folder> claude --resume <id>`) |
| `p` / `P` | prompt the session / let swarmux pick one |
| `v` | review the next jobs |
| `D` | the classic panel grid below, and back |

## Classic dashboard (`D` on Focus)

The panel grid earlier versions opened with. From top to bottom:

- **Header**: which model orchestrates and which audits, a legend of the
  actors in this job (Haiku shows its output tokens per second), the exact
  `claude` command line and how many subagents are mounted.
- **Steps**: `plan · fork · delegate · parallel · merge · audit · ship`, the
  current one highlighted. The step comes from what the job did: the first
  guard or routing decision is *fork*, a Task/Agent call is *delegate*, two
  subagents at once is *parallel*, all of them back is *merge*, the review
  is *audit*, done (and passed) is *ship*.
- **Where prompts go**: the selected session, highlighted, with its model,
  lean flag and folder (`↑↓` changes it).
- **Reviewer** (dashed, left): its model, what it checks, loop-breaker stops
  (`<error again>`) and its verdict (`<before done>`), with its calls, tokens
  and activity.
- **Lead**: model, context used of the window, cost and the tokens served
  from cache.
- **Guard / JEV**: decisions this run and their p95 latency, the routing
  pick (with JEV's confidence when JEV decided), the tool-gate verdict mix,
  stops and calls handed to you, and the latest decision.
- **Subagent cards**: one per subagent, orange for Sonnet/Opus and blue for
  Haiku (from `model:` in `.claude/agents/<type>.md` of the session folder or
  `~/.claude/agents`; `—` when unknown), with its task, activity, isolated
  context and latest tool result. **Back to lead** shows which have reported
  back and the review's verdict.
- **Session timeline** (one row per actor, phases on the x-axis, a "now"
  cursor), **who sees what** (tokens each actor read or wrote, from Claude
  Code's usage reports) and `tail -f session.log`.

Nothing is made up: a figure that isn't known yet shows as `—` (Claude Code
doesn't report the effort level, for example). Spinners turn only while
something runs. The layout collapses as the window shrinks (who sees what
goes first, then the reviewer becomes one line and the cards a list); below
80×24 it asks for a bigger window.

