<div align="center">

```
 ___ __      __  __ _  _ __  _ __ ___   _   _ __  __
/ __|\ \ /\ / / / _` || '__|| '_ ` _ \ | | | |\ \/ /
\__ \ \ V  V / | (_| || |   | | | | | || |_| | >  <
|___/  \_/\_/   \__,_||_|   |_| |_| |_| \__,_|/_/\_\
```

**Autonomy you can verify.**

*The control plane for autonomous AI agents.*

</div>

<!--
  Demo GIF (render with `vhs assets/tapes/demo.tape`, then uncomment):

  ![swarmux --demo](assets/demo.gif)

  Caption: scripted demo · `swarmux --demo`
-->

swarmux runs AI agents unattended and holds their work to account: independent
verification before anything counts as done, policy on every action, budgets
that hold, and a tamper-evident record of all of it. It starts with Claude Code.

- **Proves "done".** Gates swarmux runs itself, hidden checks the agent never
  sees, and evidence bundles you can re-check with `swarmux verify`.
  ([done-gate](guide/done-gate.md), [verifier](guide/verifier.md),
  [evidence](guide/ledger-and-evidence.md))
- **Spends on purpose.** Per-mission and per-job budgets, 5-hour and 7-day
  forecasts, and jobs a usage limit stopped resume on their own after the
  reset. ([usage governor](guide/governor.md))
- **Asks only when it matters.** One risk-ranked inbox; approve, deny or
  always allow with one key. ([mission control](guide/mission-control.md),
  [guard](guide/guard.md))

## Install

| Platform | Command |
|---|---|
| Windows | `winget install AmirhosseinGhanipour.Swarmux` or `scoop install swarmux` |
| macOS, Linux | `brew install amirhosseinghanipour/swarmux/swarmux` |
| Any (Rust 1.88+) | `cargo install swarmux` |

Then run `swarmux doctor`. It checks Claude Code, your settings, the Guard hook
and the telemetry sources.

The package manager channels and crates.io go live with the first release; until
then, build from source (`cargo install --path .`). Release archives are signed;
see [Install](guide/install.md) for by-hand installs, `swarmux self-update` and
uninstalling.

Native Windows · Linux · macOS · one Rust binary · no tmux · no Electron · no
login · no telemetry · MIT

## Starts with Claude Code

Claude Code is the first agent runtime swarmux supports; support for other
runtimes is on the roadmap. swarmux uses what Claude Code already offers: hooks, the status line and
`stream-json` output. Each job gets its Guard hooks through `--settings` on its
own `claude` command line, so swarmux never changes your Claude Code settings
files. To show swarmux in a session you start yourself, `swarmux statusline
--print-settings` prints the entry for you to add.

## Themes

<!-- theme strip: assets/themes.png, added with the demo kit -->

Twelve built-in themes, plus `wal`, base16 and matugen imports. List them with
`swarmux theme list` and export one for your terminal or another tool with
`swarmux theme export`. See [Themes](guide/themes.md).

## Quick start

```sh
swarmux --demo         # a scripted demo: no Claude Code, no account, nothing of yours is touched
swarmux                # the TUI: 1-9 switch screens (1 Mission control, 2 Missions, 7 Board, 8 Knowledge, 9 Follow-ups), 0 Focus, p sends a prompt, q detaches
swarmux workspace add ~/product  # a folder of several git repositories, worked on as one
swarmux daemon status  # the background service that keeps the work going with no window open
swarmux doctor         # checks Claude Code, the settings, the Guard hook and telemetry
swarmux fetch          # a neofetch-style card of your sessions, spend and limits
```

From a checkout, use `cargo run` and `cargo run -- doctor`. See
[the scripted demo](guide/install.md#the-scripted-demo) for what `--demo` shows.

## What it does

- **Missions**: state the outcome, the checks that prove it, what the agent
  may not touch and the budget; swarmux lints it (at most three questions),
  runs it as a job under its own gate and calls it delivered only when the
  gate passes; a mission can plan first, so you approve a task graph before
  anything is built and see how far the build strayed from it
- **Mission control**: what needs you, who is doing what, and whether you will
  hit a limit, on one screen
- **Workspaces**: a folder of several repositories (an app, its backend, an
  admin site) worked on as one: a job sees every repository it touches, in
  a worktree per repository, and a mission lands in each of them in order,
  or in none
- **Knowledge**: the rules, facts and decisions of a workspace, each scoped,
  anchored to the code it is about and withheld once that code changes; a
  one-page map every session starts with (and gets again after a
  compaction), the items a file is about when an agent opens it, and rules
  the Guard and the gate enforce, proven in shadow first
- **Background service**: close the window and the work goes on; `swarmux`
  attaches again from any terminal, several windows at once, and a restart
  picks missions and jobs up where they were
- **Prompt library** with `{{variables}}` and a real editor
- **Queue & dispatch** across sessions with a concurrency cap, priorities and a RAM guard
- **Live dashboard**: the lead, its subagents (model, context, activity), the
  job's phase from plan to ship, the guard, the reviewer, a per-actor
  timeline, token use per actor and the session log
- **Guard**: one permission policy across all sessions and an automatic loop
  breaker, settled in code in about a millisecond so the models never see it
- **Done-gate**: when an agent says it is finished, swarmux runs the
  repository's tests itself and sends it back with the failure until they
  pass; tests and other oracles it may not edit are protected
- **Independent verifier**: the checks run in a clean copy swarmux makes of
  the work, not in the agent's worktree, followed by hidden checks the agent
  never sees; edited or switched-off tests block the stop
- **Review step**: a read-only reviewer model audits a finished job's diff
- **Ledger screen**: every run as a card with its outcome, evidence, cost,
  model mix and what became of it (merged, reverted); a digest of what
  happened while you were away; re-run, fork, search or verify from any card
- **Evidence bundles**: every delivered mission leaves a verifiable JSON record that
  `swarmux verify` re-checks at the head commit in a clean checkout
- **Test-weakening detector and base suite**: deleted, skipped, loosened or
  re-blessed tests block a stop, and the checks are run again against the
  original tests ([verifier](guide/verifier.md))
- **Anchored ledger**: signed checkpoints of the audit ledger's head, kept
  with your key, expose a ledger that was cut short or rewritten
  ([ledger](guide/ledger-and-evidence.md#anchoring-the-ledger))
- **Cache-read metric**: how much of each job's input came from the prompt
  cache, on the Ledger card and in Focus; `swarmux stats cache`
- **Audit ledger**: a tamper-evident, hash-chained record of every job,
  agent event, review and guard decision
- **JEV (optional, bring your own key)**: the hosted Jev model for fuzzy
  decisions such as unmatched tool calls and which session should take a prompt
- **Watchdog**: notices loops, stalls, full contexts and runaway spending,
  and tries a ladder of recoveries (nudge, fresh session, diagnosis, fork
  from a checkpoint, stronger model, replan) before it asks you
- **Let swarmux route**: one cheap Haiku call picks the session for a prompt
- **Lean mode** per session: skip your user-level Claude Code settings

Single Rust binary. Windows, macOS, Linux. Works with Claude Code only.

## User guide

- [Install, update and uninstall](guide/install.md)
- [Configuration](guide/configuration.md)
- [Telemetry sources](guide/telemetry.md)
- [Mission control](guide/mission-control.md) and the [inbox](guide/mission-control.md#the-inbox-as-an-alarm-panel)
- [Work: decide and land](guide/work.md)
- [Missions](guide/missions.md)
- [The Board: shared state between a mission's agents](guide/board.md)
- [Follow-through: what your agents leave behind, brought to you three times a day](guide/follow-through.md)
- [Ledger and evidence bundles](guide/ledger-and-evidence.md)
- [Focus and the classic dashboard](guide/focus.md)
- [Themes and layouts](guide/themes.md)
- [Guard](guide/guard.md)
- [Sessions: lean mode and routing](guide/sessions.md)
- [JEV](guide/jev.md)
- [Done-gate](guide/done-gate.md)
- [Independent verifier, hidden checks and review](guide/verifier.md)
- [Worktrees and checkpoints](guide/worktrees.md)
- [Workspaces: several repositories worked on as one](guide/workspaces.md)
- [Knowledge: rules, facts and decisions your agents keep to](guide/knowledge.md)
- [Contracts: what your repositories promise each other](guide/contracts.md)
- [Usage governor](guide/governor.md)
- [Watchdog and recovery ladder](guide/watchdog.md)
- [Notifications](guide/notifications.md)
- [The background service](guide/daemon.md)

## Project

[Contributing](CONTRIBUTING.md) · [Code of conduct](CODE_OF_CONDUCT.md) ·
[Security](SECURITY.md) · [Changelog](CHANGELOG.md) · [Releasing](RELEASING.md)

## License

MIT
