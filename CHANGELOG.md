# Changelog

All notable changes to swarmux are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Navigation: `Esc` goes back to the screen you came from, and `g` then a
  letter goes to a screen (`gm`, `gp`, `gk`; `gd`, `gw` and `gr` are kept for
  screens that are not built yet and go to the nearest one, saying so). The
  keys are now named actions behind a keymap, so they can be changed later;
  with the default keymap every existing key does what it did.
- Follow-through: what agents notice, ask or suggest is collected instead of
  dying in their last message. The Board's `schedule_followup` (with its
  `after_minutes` delay now honoured) and `ask_human` become follow-ups, and
  one cheap Haiku call per finished job (`[followups] extract`, cost in the
  ledger) lists the questions, decisions, suggested tasks, risks and
  blockers in its final message. Repeats across jobs and repositories are
  merged. Nothing is decided by a model: your standing orders
  (`[[followups.order]]`: kinds, repos, path globs, keywords, a cost limit;
  `auto-approve`, `always-ask` or `park`) decide, never for decisions, risks,
  blockers, urgent items or protected paths unless you say `allow_risky`
  (and never protected paths), and every automatic decision is in the ledger.
- The owner brief: three times a day (`brief_times`, default 09:00, 13:00,
  18:00) and on demand (`swarmux brief`, `swarmux followup brief`), as
  decision cards with why, where from and a reason you might say no.
  Only urgent items (a P1 risk, a blocker on running work) reach Mission
  control's inbox. Answer with one key on the new Follow-ups screen (`9`:
  `a` approve, `d` decline, `l` later, `o` open the job, `m` draft a mission)
  or `swarmux followup approve|decline|defer <id>`. Approved follow-ups are
  kept as intents; replies and brief delivery sit behind ports so a chat
  adapter can be added. A rubber-stamp guard asks for one spot check when
  you approve 95% or more of a kind over at least 20 decisions (schema v21).
- Cross-repo contracts: what each repository of a workspace provides
  (tables, columns, enums, functions, policies, OpenAPI endpoints and
  schemas, `.env.example` keys) and which files of which repositories
  consume it, read from the sources without a model. Pluggable extractors:
  SQL migrations and dumps (applied in file order, so renames and drops
  count), OpenAPI (JSON and block YAML), and consumer scans for Dart/Flutter,
  TypeScript/JavaScript (including Supabase's generated types) and Python.
  Scans are incremental by content hash (schema v22). `swarmux contracts
  [--workspace W] [--consumers] [show <symbol>]` and a Contracts tab on the
  Knowledge screen (`t`).
- Agents hear who consumes a file they read or edit (a provider's
  consumers, a consumer's provider shape) in the same 300-token pull as
  knowledge. A workspace mission's done-gate compares the contracts with
  those at job start: the consumer repositories' checks join the run, a
  removed or renamed symbol that is still used blocks with the exact places,
  and consumers outside the mission's repos raise an inbox item. Verdicts
  and deliveries are ledger entries.
- Knowledge: rules, facts and decisions per workspace (or repository), each
  with a why, a scope (repositories, path globs, task classes), anchors with
  content hashes, a status (proposed, shadow, enforced, advisory, stale,
  retired), its source and trust. Every change is a ledger event and the
  items are rebuilt from them (`swarmux knowledge rebuild`; schema v20).
  Anchors are hashed again on rescan, at job start and after a delivery;
  stale items are withheld and listed in one P3 inbox item.
- Rules with enforcers: `guard` (deny, a P1 approval that shows the rule, or
  a warning, on tool calls matched by tool, path and pattern; rules only
  forbid and fail closed), `gate` (a hidden check the done-gate runs for
  missions), `lint` (a visible check) and `prompt`. New acting rules start in
  shadow and log what they would have done. `swarmux rules` shows the
  registry: rules, enforced, failing, advisory-only.
- Knowledge reaches agents small and late: the workspace map and prompt
  rules at session start and again after each compaction (cache-stable),
  and the items a file is scoped or anchored to when an agent reads or edits
  it (`PreToolUse` `additionalContext`, 300 tokens, once per item per job).
  Every injection is on the ledger.
- `swarmux knowledge list|show|add|edit|promote|confirm|retire|trust|recheck|export|import`,
  and `swarmux knowledge init <workspace>`, which drafts the one-page map
  from the scan and opens it in your editor.
- The Knowledge screen (`8`): items by standing, the registry, stale items
  to confirm and shadow rules' hits; add, edit, promote, confirm, retire.

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
- The done-gate reads a job's diff for weakened tests (deleted or switched-off
  tests, dropped or loosened assertions, edited snapshots, wider tolerances,
  relaxed CI) and blocks the stop as *tests weakened* with the evidence lines;
  `allow_test_changes` in `.swarmux/gates.toml` says which changes are
  intended. The inbox item is P1 when the mission's checks are affected.
- Base-suite check: the checks run again with the original versions of the
  changed test and protected files laid over the agent's code, as its own gate
  step and ledger entry.
- Prompt-cache metric per job (read and written tokens, `—` when unknown), on
  the Ledger card, in the job detail and in Focus; `swarmux stats cache`.
- Ledger anchoring: `swarmux audit checkpoint`, `audit verify` against every
  signed checkpoint (cut short, rewritten), `audit export-checkpoints`; a
  checkpoint is taken when a mission is delivered and kept in its evidence
  bundle. The signing key is an ed25519 minisign key in the OS keychain.
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

- A key pressed with Ctrl or Alt (other than Ctrl+C) no longer counts as the
  plain letter: `Ctrl+q` does not quit and `Alt+x` is not `x`. `Shift` and
  AltGr characters (`D`, `[` on a German keyboard) work as before.
- The README is now a short overview; the reference moved to `guide/`.
- Windows: whole process trees are killed on cancel, Claude starts are
  staggered, config collisions are retried and long paths are supported in
  worktrees.

### Fixed

- A second result no longer finishes a job twice.
- A negative zero spend is never shown.
- The selected session stays selected when the list changes.
