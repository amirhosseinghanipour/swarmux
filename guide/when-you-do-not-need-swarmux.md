# When you do not need swarmux

swarmux is a layer around Claude Code for work that runs without you. If your
work is not that, Claude Code alone, or another tool, is enough, and swarmux
adds a second thing to run and learn.

## Claude Code alone is enough when

- **You work in one session and watch it.** Claude Code's own permission
  modes, its auto mode and its prompts decide what the agent may do while you
  look at the screen. The Guard adds rules across sessions; with one session
  there is little to add.
- **You read the diff before anything lands.** If every change goes through
  your own review, the gate's checks are a second run of what you will read
  anyway. A passing gate is checks passing, not a review
  ([verifier](verifier.md#what-this-does-not-establish)).
- **You want an agent to loop on a goal or a schedule.** Claude Code offers goal,
  loop and scheduled runs (check its current documentation for what they do).
  swarmux's value there is verification,
  budgets and a record, not running or repeating work.
- **You want a second opinion on a change.** Claude Code offers review features
  that read a diff, as swarmux's review step does ([review](verifier.md#review-step)).
  Neither is a deterministic check.
- **You only need a cost limit.** `claude --max-budget-usd` and your account's
  limits cap spend. swarmux passes the same flag per job
  ([governor](governor.md)) and adds per-mission budgets and window forecasts;
  if you do not need those, you do not need swarmux for this.

## Another tool is a better fit when

- **You need a different agent.** swarmux starts with Claude Code and runs
  nothing else.
- **You need a sandbox.** Agents under swarmux run as your operating-system
  user, and the Guard matches text ([Guard limits](guard.md#limits)). If the
  agent must not be able to read or change something, use a container, a
  virtual machine or a separate account, and run swarmux inside it if you like.
- **You want a graphical or remote client.** swarmux is a terminal program with
  a local background service; it has no network listener
  ([daemon](daemon.md)).
- **You want continuous integration.** The gate runs where swarmux runs, on the
  files an agent left. It does not replace your CI, which should still run on
  what you merge.

## swarmux earns its place when

- work runs while you are away, so something other than you must decide
  whether it is done;
- several sessions or repositories are involved, and you want one inbox, file
  claims and ordered landing ([workspaces](workspaces.md),
  [missions](missions.md));
- a job needs a cost cap and rules that do not depend on you being at the
  keyboard ([guard](guard.md), [governor](governor.md));
- someone must be able to check afterwards what ran, what passed and what did
  not run ([evidence](ledger-and-evidence.md)).

Before you rely on it, read what it does not establish:
[the verifier's limits](verifier.md#what-this-does-not-establish),
[the Guard's limits](guard.md#limits) and
[what leaves your machine](what-leaves-your-machine.md).
