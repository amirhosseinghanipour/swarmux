# Changelog

All notable changes to swarmux are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Workspaces: `swarmux workspace add|list|show|rescan|rm` records a folder of
  several git repositories (two levels down; the folder need not be one),
  with each repository's default branch, stacks (Flutter, Dart, Node, Rust,
  Python, Go, Supabase, Ruby, .NET, Unity, Godot) and commands (scripts, make
  targets, just recipes). A session on a workspace (`w` on the Sessions
  screen) runs its jobs from the root with each repository it touches added
  with `--add-dir` and its `CLAUDE.md` loaded; with worktrees on, each job
  gets a worktree per repository, laid out like the workspace. Sessions and
  wall cards are grouped by workspace.
- Workspace missions: `repos = [...]` names the repositories a mission
  touches, producers first, and `repo: command` checks (or
  `[acceptance.by_repo]`) run in each. Once the gate passes the mission lands
  in every repository in that order, or in none: a repository that can't take
  its merge puts back the ones before it and goes to your inbox.
- Mission control, the home screen: an alarm-grade inbox of what needs you
  (ranked by urgency, with batches, shelving and focus mode), a wall of session
  cards and a strip of limits and data health, scaled to the window size.
- Missions: state the outcome, the checks that prove it, what the agent may
  not touch and the budget. swarmux lints it, runs it under its own gate and
  calls it delivered only when the gate passes.
- Task graphs: an approved plan whose tasks can run side by side runs as one
  job per task, each in its own worktree and limited to the files it claims
  (writes elsewhere are refused). Finished tasks merge into a swarmux branch in
  dependency order; a conflict sends the agent back once to rebase, then asks
  you (`[missions] parallel`).
- The background service: jobs, approvals, missions, the watchdog, the
  governor, the Board and notifications keep going with no window open.
  `swarmux` attaches to it (starting it when needed) and `q` detaches;
  several windows can attach at once and all show the same screen.
  `swarmux daemon start|stop [--wait]|status|run`; one service per data
  folder, a local pipe or socket only your user can open, a size-capped log,
  and a version check between window and service. `swarmux --no-daemon`
  runs everything in the window as before, and `swarmux doctor` reports the
  service.
- Jobs the service stopped on `swarmux daemon stop` resume by themselves,
  in their own Claude session, when it starts again.
- Missions carry on after a restart: a task graph picks up where it was
  (merged tasks stay merged, cut-off tasks continue in their worktree and
  Claude session, nothing merges twice), and single-job missions, their
  verification and plans being drafted continue too.
- The Board: the agents of a mission share typed state (claims, facts,
  decisions, questions) through a per-job `swarmux-board` MCP server. A claim
  that overlaps another live job's is refused naming the holder, new sessions
  start with a digest of at most 1500 tokens, and a question to you lands in the
  inbox. Board text is treated as untrusted: cleaned, capped and always shown
  quoted with its job and time (`[board]`, screen `7`).
- Done-gate: swarmux runs the repository's checks itself when an agent says it
  is finished, and sends the agent back with the failure until they pass.
  Tests and other protected files cannot be edited to get past it.
- Independent verifier: checks run in a clean copy of the work, followed by
  hidden checks the agent never sees (`swarmux holdout`). Edited or disabled
  tests block the stop. A risk-gated reviewer audits the diff.
- Evidence bundles: every delivered mission leaves a verifiable JSON record, and
  `swarmux verify` re-checks it at its head commit in a clean checkout.
- Ledger screen with a card per run, a digest of what happened while you were
  away, `swarmux brief`, and `swarmux labels` for what became of each branch
  (merged, reverted).
- Tamper-evident, hash-chained audit ledger (`swarmux audit verify`, `tail`).
- Usage governor: per-job and per-mission budgets, 5-hour and 7-day forecasts,
  queue holds with a stated reason, model downgrade, and auto-resume of jobs a
  usage limit stopped.
- Focus view for one session: situation card, gate lane, action stream and
  checkpoints, plus a classic dashboard.
- Per-job git worktrees with port blocks and checkpoints
  (`swarmux worktree`).
- Notifications for background alerts: Windows toast, terminal sequences and
  opt-in `ntfy` push.
- Themes: twelve built-in, `wal`, base16 and matugen imports with hot reload,
  transparent background, Nerd Font detection, colour-blind-safe checks and
  `swarmux theme list` and `export`. Layouts, a constellation and spectrum
  screen and a motion setting.
- `swarmux fetch`: a neofetch-style card of your sessions, spend and limits.
- `swarmux statusline` for Claude Code's status line data.
- `swarmux install`, `uninstall` and `self-update`, with minisign signature
  verification, plus signed release archives and package manager templates.
- `swarmux doctor` now also summarizes the telemetry sources.
- Telemetry collectors for spend, burn, limits, sessions, tools, git, GitHub
  and system load, read locally with no message text.
- Guard: one permission policy across sessions with a loop breaker, optional
  JEV (bring your own key) for fuzzy decisions, one-call session routing and
  lean mode.
- Prompt library with `{{variables}}`, queue and dispatch with a concurrency
  cap and RAM guard, and the read-only review step.
- Community files: contributing guide, code of conduct, security policy, issue
  and pull request templates, and the user guide in `guide/`.

### Changed

- The README is now a short overview; the reference moved to `guide/`.
- Windows: whole process trees are killed on cancel, Claude starts are
  staggered, config collisions are retried and long paths are supported in
  worktrees.

### Fixed

- A second result no longer finishes a job twice.
- A negative zero spend is never shown.
- The selected session stays selected when the list changes.
