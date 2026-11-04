# Watchdog

swarmux watches every running job for five signs of trouble:

- **loop**: the same call fails the same way `loop_repeats` times in a row
  (the Guard's setting);
- **oscillation**: one call keeps failing the same way between other steps
  (fix, test, same error, fix, test, same error);
- **stall**: no events for `stall_secs` with nothing running, one command or
  subagent running for over `stall_tool_secs`, or `stall_turns` calls in a
  row that only repeat earlier ones. Waiting for you never counts;
- **context**: the window is `context_high` full, or was compacted
  `compactions` times;
- **burn**: spending about `burn_usd_per_min` or more, over five minutes.

When one fires it climbs a recovery ladder, cheapest first. Each rung is
tried once, and after each one the agent gets time to settle
(`nudge_cooldown_secs`, `restart_cooldown_secs`) so it is never nudged again
and again. A job that has ended climbs at once.

1. **Nudge.** The agent's next matching tool call is not run, and the reason
   it gets back says what the watchdog saw and what to try instead. A silent
   agent can't be reached this way, and a full context isn't helped by it,
   so those skip this rung.
2. **JEV.** With JEV on and agreed to, it judges: carry on (the watchdog
   stands down), change approach (the agent is told), or stop (the next rung
   follows at once). Skipped when JEV is off.
3. **Fresh session.** A checkpoint and a handoff, then the work goes on in a
   new session with a clean context, in the same worktree.
4. **Diagnosis.** One Haiku call reads the last steps and names what went
   wrong; the answer goes to the agent and into the next handoff.
5. **Fork.** The worktree goes back to the last checkpoint from before the
   trouble (the files of the failed attempt stay in a commit under
   `refs/swarmux/set-aside`) and a new session starts from there.
6. **Stronger model.** A new session on the next model in `escalate_to`.
7. **Replan.** A new session that plans smaller steps first.
8. **Inbox.** You decide, with what was tried and the diagnosis attached.

New sessions are capped at `max_restarts` per run of work. Every alarm, rung
and outcome goes to the audit ledger, the job's log and the change log, and
Focus shows the recovery trail. With the watchdog on, the loop breaker stops
a job one failure after the nudge rather than at the nudge.

```toml
[watchdog]
enabled = true
rungs = ["nudge", "judge", "fresh", "diagnose", "fork", "escalate", "replan", "inbox"]
stall_secs = 120           # silence that counts as a stall
stall_tool_secs = 600      # how long one command may run
stall_turns = 15           # calls in a row that only repeat earlier ones (0 = off)
oscillation_repeats = 3    # same failure between other steps (0 = off)
context_high = 0.85
compactions = 2            # 0 = off
burn_usd_per_min = 1.0     # 0 = off
nudge_cooldown_secs = 90
restart_cooldown_secs = 180
max_restarts = 3
escalate_to = ["sonnet", "opus"]
```
