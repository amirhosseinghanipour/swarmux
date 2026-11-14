# Changelog

All notable changes to swarmux are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Intervene: message a running agent from Work. `m` on a running job opens a
  line (and the stream tab, the conversation so far) and sends your words to its
  agent in one of two modes, `Tab` or `M` to switch: **queue** (the default) keeps
  the message until the job's turn ends and then continues the Claude session
  with it; **steer** stops the turn (the same stop as Kill) and continues the
  session with the message at once, after a second confirmation
  (`intervene_confirm_steer`). Steering is refused while the Guard holds one of
  the job's calls, and no message approves anything. swarmux does not write
  into a running `claude`: a message never arrives mid-turn, and a steer cuts the
  step in flight short (the agent is told). Every message is an `operator` entry
  in the ledger (action `intervene`: mode, text, job, outcome, delivery time),
  written before anything is sent. A receipt says *queued*, *handed over* or
  *delivered*, and only calls it delivered once the agent's turn with it
  started. The same operation is `Engine::intervene` in the application layer and
  the console model gains the `intervene` action kind (enabled, or disabled with
  its reason) and a `ReceiptView`, so a daemon API or a phone can offer it. New
  `[console]` keys `intervene_mode`, `intervene_max_chars` (default 2000) and
  `intervene_confirm_steer`, which warn and keep the default when wrong; actions
  `work.message` (`m`) and `work.message_mode` (`M`). The demo shows a queued
  message delivered. The console JSON gains the `intervene` action
  (`schema_version` unchanged: additive).
- Verifier and gate hardening. The verifier now checks what runs the tests as
  well as the tests, and says so when it could not check something.
  - The harness is a finding. Created, changed, moved or deleted, any file of a
    built-in list (`conftest.py`, `pytest.ini`, `pyproject.toml`, `jest.config.*`,
    `vitest.config.*`, `package.json`, `build.rs`, `.cargo/config.toml`,
    `Cargo.toml`, `Makefile`, `.github/workflows/**`, `dart_test.yaml`,
    `pubspec.yaml`, and more, see [the verifier](guide/verifier.md#the-test-harness)),
    any `[[check]]` program that is a path in the repository, and
    `[verifier] substrate = [...]` blocks the stop as *test harness changed*
    (P1 when the gate gives up) unless `allow_substrate` or `allow_test_changes`
    matches. Manifests are read, not just flagged: a change that only adds
    dependencies or touches the package's metadata is not harness; a `[patch]`,
    a `build =`, a test script or a pytest entry point is.
  - The base suite and the hidden checks run on a copy made again from what the
    agent left, with the harness and changed tests put back as the commit the job
    started from had them (what the job created is removed). A planted
    `conftest.py`, a cargo runner that runs nothing or a `build.rs` that rewrites
    the source can make the visible checks pass; they are not there when the hidden
    checks run.
  - No silent skips. Hidden checks that exist and did not run show `hidden
    checks: not run (reason)` on the wall card, in Focus, on the Work screen, in
    the ledger and in the bundle. A mission whose hidden checks did not run, whose
    checks ran in the worktree (`ran_in = "worktree"`), or whose change could not
    be read is Escalate, never Pass.
  - A `.swarmux/gates.toml` that cannot be read makes every verdict Escalate with
    the parse error (it used to read as an empty gate and pass with a note);
    `swarmux gate lint [--strict]` says what is wrong.
  - `swarmux gate selftest` plants a harness file and deletes an assertion in
    scratch copies and fails (exit 1) when the gate lets either through; the
    result is a `policy` entry in the ledger. `[verifier] revert_probe = true`
    reverts the non-test part of a change and requires a failing check.
  - `[[check]]` takes `required`, `level` (`fast`, `full`, `deep`; with `[gate]
    level` and `swarmux gate run --level`), `on_error = "fail"` and `env`. A flaky
    check is named after `flaky_after` flakes in `flaky_window` runs. Checks run
    with a clean environment: no `SWARMUX_*`, provider or other credentials,
    whatever `env` lists.
  - New or changed dependencies in a manifest or lockfile (Cargo, npm, pnpm, yarn,
    pip, poetry, uv, go, pub) are listed with the package names, and block with
    `[verifier] dependencies = "block"`.
  - A plain job that ends gets its gate run once more when its last stop was not
    a pass on the tree it left, and `unverified` when that cannot be done.
  - Evidence bundles are `schema` 2 and record how the verdict was reached (where
    the checks ran, planned against run, hidden not run, base suite, what was found).
    Bundles of schema 1 still read and verify; a fixture pins their bytes.
  - Every gate verdict in the ledger carries `policy_sha256`, the hash of the gate
    configuration that judged it.
  - The exploit corpus in `crates/infrastructure/tests/verifier_seeded.rs` covers
    a planted `conftest.py`, a `.cargo/config.toml` runner, a `build.rs`, a jest
    config's `testPathIgnorePatterns`, a `package.json` test script and more,
    next to the honest fixes that must pass.
- Guard hardening. Nothing below changes what a rule matches; it changes what
  the Guard does when it can't decide, and who may decide.
  - `[guard] on_unreachable` (`"auto"`, `"deny"`, `"ask"`, `"pass"`) sets what a
    job's hook prints when swarmux can't be reached. Each job's mode is passed in
    `SWARMUX_GUARD_FAIL`. The hook leaves every fallback in a spool file, and
    swarmux writes the spool into the ledger at its next start (kind `policy`,
    event `guard_fallback`). `swarmux doctor` shows the mode and runs the hook
    against an unreachable app in each mode.
  - `swarmux guard rules list | remove <id> | prune` and `swarmux guard lint`.
    "Always allow" rules are kept in `guard-rules.toml` with their scope,
    source, creation time and expiry; adding, removing and expiring one is a
    ledger entry (kind `policy`, events `added`, `removed`, `expired`).
  - `[guard] always_scope` (`"repo"`, `"workspace"`, `"global"`),
    `always_expiry_days` (1 to 3650, or `"never"`), `protected_paths` and
    `hook_on_failure_block`, all read line by line: a
    value that isn't valid is replaced by its default and named by the status
    bar, `swarmux doctor` and `swarmux guard lint`.
  - A shell command that runs swarmux from a job is denied unless the
    subcommand is `hook`, `board-mcp` or `statusline`. Tool calls can no longer
    read or write the database, the audit key file, `config.toml`, the rules
    file, the spool, the hidden checks or the verifier's copies; paths are
    compared by component after expanding `~` and `$HOME` and following
    symlinks. Writes to `.claude/settings*.json`, `.claude/hooks`, `.mcp.json`,
    `CLAUDE.md`, `AGENTS.md`, `.git/hooks` and `.swarmux` are held for you
    (denied for unattended jobs).
  - A subagent's tool calls carry its `agent_id` and `agent_type` into the
    decision record, the ledger and the inbox.
  - Decisions in the ledger carry the hash of their rule and who resolved them.
  - The hook can add `"onFailure": "block"` to the PreToolUse hook
    (`hook_on_failure_block`, off by default). Which Claude Code versions know
    the key is not detected yet.
- The Deck (`g d`): the task graph in the middle (a card for every job and
  every task of a plan built as a task graph, left to right from producer to
  consumer, each with a state shape, its repository, model, spend and one dot
  per configured check; lines only for recorded relationships: task
  dependency, cross-repo contract and file claim, each met, pending or
  blocked), and around it the sessions strip (the Sessions screen's content),
  what needs you, verification figures with their sample size, limits and
  budget with the forecast, the Guard's exceptions (routine allows as one
  number) and a timeline. `Enter` on a card opens its outcome in Work, and
  `Esc` comes back. At 120x36 the graph becomes a list and some panels fold
  (`Tab` brings them). Its keys are rebindable under `[keys.deck]`. The
  `g d` chord goes to it (it was a stand-in for Mission control).
  See [The Deck](guide/deck.md).
- `[deck]` in `config.toml`: which panels show and in which order, the
  graph's direction (`"lr"`) and how many sessions the strip lists.
  Mistakes are ignored with a line on the Deck, never a failure;
  `swarmux deck lint` lists them and `swarmux deck show` prints the layout.
- `swarmux --demo` has a second mission, `Release 2.4`, a plan of three tasks
  built as a task graph, so the Deck shows a task dependency (its graph state is
  written in advance: the demo has no worktrees to merge in).
- Send back with a note (`s` on Work). Work that stopped, was escalated or
  waits for you resumes in its own Claude session and worktree with your note
  as its next instruction, a mission follows the new job, and the gate judges
  the agent's next claim with the checks run again. A held tool call is denied
  and your note reaches the agent as the denial's reason. You type the note on
  a line at the bottom, press Enter to send it, Esc to drop it; Tab picks from
  suggested notes (`send_back_notes` in the new `[console]` section), and
  nothing is sent until you confirm. Each note is recorded in the ledger (kind
  `operator`: who, when, the note, the job, where it stood, what happened
  next). See [Work](guide/work.md#send-back).
- Per-file counts. The Work diff tab and plan against drift show each changed
  file with the lines git counted in it (`M src/invoice/round.rs +22 -5`),
  planned tasks with the files that changed in their areas, files outside the
  plan with their counts, and planned tasks nothing touched. New evidence
  bundles keep the list with their totals; a bundle sealed before this shows
  its totals only, and says so. The console JSON gains `files`, `changed` and
  `outside_files` (additive: schema version unchanged).
- Verification figures under the Work queue and in `swarmux stats
  verification`: first-run gate pass rate, claim to verified (median and 90th
  percentile), verified an hour, checks run and bundles sealed, worked out from
  the ledger over the last `metrics_hours` hours (a day by default). A figure
  with nothing behind it is left out, never shown as zero. The console JSON
  gains a `metrics` section.
- An optional `due` on a mission contract (`due = 2026-11-14T16:00:00+03:30`,
  RFC 3339, also a field in the Missions composer). Work shows `due 16:00` where
  it shows the cost of waiting, and among decisions of one priority the earlier
  due time comes first; a due time already past is a lint warning. A mission
  with no `due` behaves as before. See [Missions](guide/missions.md#due).
- A check that could not start shows why it could not as its failing line, in
  the Work checks and claim-against-checks views.
- The Work screen (`g w`): decide and land in one place. A ranked queue (decide,
  ready to land, running, landed) with the cost of waiting from data that
  exists (the auto-deny countdown, what a plan's task dependencies hold up) and
  the usage windows below it; and for the selected outcome four tabs (work
  product, stream, diff, checks) showing plan against drift, the agent's claim
  against the checks, the evidence, and what landing does. A held tool call
  shows the call, the rule, the risk, the countdown and the agent's own words,
  quoted and labelled. Actions (answer a held call, approve or change a plan,
  land, kill, retry, resume, open the evidence in the Ledger) are named actions
  in the keymap, shown dim with their reason when they can't be taken. The tab
  bar counts the decisions waiting. See [Work](guide/work.md).
- `landing` in `[missions]`: `"auto"` (the default, as before) lands a workspace
  mission the moment its gate passes; `"manual"` leaves it verified, waiting
  under READY TO LAND until you land it from Work (`l`). Landing on request
  merges each repository into `swarmux/<mission>/integration`, producers first,
  all or none, and puts back what it merged if one repository can't take its
  merge, exactly as the automatic landing does. A bad value is refused with its
  line. See [Configuration](guide/configuration.md#landing).
- A console read model, the data layer for the Work and Deck screens and later
  a daemon API or phone client: ranked outcome groups (decide, ready to land,
  running, landed), each outcome's contract, plan and drift, checks and gate
  stages, the agent's claim, evidence references and what landing does; the
  Guard's holds with the countdown and the agent's own words quoted from its
  stored session stream; and the Deck's graph of task dependencies, contract
  links and file claims. Plain serde data with a versioned, golden-tested JSON
  shape; no screen uses it yet.
- Customizable hotkeys: a `[keys]` section in `config.toml` rebinds any
  action (one key, a list, a chord like `"g w"`, `ctrl+x`, `alt+x`,
  `shift+tab`, `f5`) or unbinds it with an empty list; what you don't mention
  keeps its keys. `Esc`, `Ctrl+C`, the typed-confirm challenge and the answers
  to approvals (`y`, `n`, `a`, `c`) are not rebindable. A mistake in `[keys]`
  never costs you the keys: the line is ignored, its action keeps the default
  keys, the status bar says so and points to the new `swarmux keys lint`
  (every problem, exit 1 on errors, warns about new overlaps). `swarmux keys
  list` shows every action with its keys and default, `swarmux keys export`
  prints the keys in use as TOML to paste into the config.
- `?` opens the keys that work on the current screen and the global ones,
  from the keymap in use; `Esc` or `?` closes it. Footer hints, tab numbers,
  the shelve prompt and the "press 6 then n" style messages are written from
  the keymap too, so a rebound key shows as what it is. Every footer now ends
  with `? help` before `q quit`; a footer that is too wide drops the other
  hints from the end instead of being cut mid-hint, and those two go last.
- Navigation: `Esc` goes back to the screen you came from, and `g` then a
  letter goes to a screen (`gm`, `gp`, `gk`, `gw`; `gd` and `gr` are kept for
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
- Guard: one permission policy across sessions with a loop breaker, one-call
  session routing and lean mode.
- Prompt library with `{{variables}}`, queue and dispatch with a concurrency
  cap and RAM guard, and the read-only review step.
- Community files: contributing guide, code of conduct, security policy, issue
  and pull request templates, and the user guide in `guide/`.

### Changed

- Honest wording: the README and guides now say what the code does. "Proves
  done" became "decides done with checks it runs itself"; hidden checks are "kept
  outside the repository and out of the agent's tools", shown as counts, and `not
  run (reason)` when they could not run; the Guard is described as checking the
  tool calls Claude Code reports, with its fail modes and a Limits section;
  budgets name where they are enforced; the tagline gets a definition of
  "verify". New pages: what leaves your machine, and when you do not need
  swarmux. No behaviour changed.
- Hidden checks are kept per repository by its folder name and what it is (its
  remote and root commit), like `shop.id-3fa9c1d2e4b7`, so two folders of one
  name no longer share them. Checks kept under the plain folder name move to the
  identity key the first time that repository is looked at (recorded in
  `holdout/.migrated` and in the ledger as `holdout_migrated`); a name is handed
  out once.
- A workspace mission's checks run in the task folder, so by default its gate
  now says Escalate and nothing lands. Put `[verifier] worktree_checks = "allow"`
  in `.swarmux/gates.toml` at the workspace root (now copied into each task
  folder) to let it land; the record still says the checks ran in the worktree.
- The base suite and the hidden checks no longer run in the copy the visible
  checks ran in. `swarmux gate run` prints `gate cannot pass this: escalate` when
  the gate could not vouch for work in which nothing was wrong.
- Irreversible calls (risk R4) are allowed only by the typed confirmation, and
  the Guard enforces it, not only the key handler. A call that an allow rule
  (even a bare `Bash`) or `unmatched = "pass"` used to allow is now held for
  you. Update an `allow` rule that you relied on to run `rm -rf` or
  `git reset --hard` unasked: it is asked now.
- "Always allow" (`a`) and the offered rule (`r`) now apply to the repository
  the call was made in (`always_scope`) and expire after 30 days
  (`always_expiry_days`), and are written to `guard-rules.toml`, not
  `config.toml`. They are refused for irreversible calls, shell lines of
  several commands and calls whose rule would be the bare tool name; those can
  be approved once. Rules already in `config.toml` `[guard] allow` keep working
  everywhere and never expire; `swarmux guard rules list` shows them as
  `source config`.
- Jobs nobody watches (a mission's jobs, reviewers and what continues them) are
  denied when their hook can't reach swarmux. Before, every job fell back to
  Claude Code's own rules. Set `[guard] on_unreachable = "pass"` for the old
  behaviour. Jobs you start from your own prompt behave as before.
- Every job's environment now has `SWARMUX_GUARD_FAIL` and `SWARMUX_GUARD_SPOOL`.
- A key pressed with Ctrl or Alt (other than Ctrl+C) no longer counts as the
  plain letter: `Ctrl+q` does not quit and `Alt+x` is not `x`. `Shift` and
  AltGr characters (`D`, `[` on a German keyboard) work as before.
- The README is now a short overview; the reference moved to `guide/`.
- Windows: whole process trees are killed on cancel, Claude starts are
  staggered, config collisions are retried and long paths are supported in
  worktrees.

### Removed

- JEV, the optional hosted decision model (bring your own key), with
  everything around it: the `[jev]` section of `config.toml`,
  `swarmux jev set-key` and `swarmux jev test`, the consent question, the
  watchdog's JEV rung and the JEV-first step of "let swarmux route". An LLM
  judge that reads text an agent can influence is a target for prompt
  injection, its reasons can talk a wrong approval into place, and it sent
  decision context to a third party. What changes:
  - A tool call no rule matches goes to you (`unmatched = "ask"`) or back to
    Claude Code (`"pass"`), exactly as configured, with no model in between.
  - "Let swarmux route" asks Haiku directly.
  - The watchdog ladder is nudge, fresh session, diagnosis, fork, stronger
    model, replan, inbox; its `"judge"` rung is gone.
  - Only your typed confirmation can allow an irreversible call, as before;
    no automated resolver (a timeout, "always allow", a key press) can.
  - What you must do: nothing breaks. An old `[jev]` section is ignored (so is
    `"judge"` in `[watchdog] rungs`) with a warning at start-up, in
    `swarmux doctor` and in `swarmux guard lint`; delete it to clear the
    warning. A key you stored for it is not deleted: `swarmux doctor` says
    whether one is still in the OS keychain (service `swarmux`, account
    `typesafe` or `openrouter`) and how to remove it. The unused
    `jev_consent` table stays in the database.
  - History is kept: decisions and ledger entries made with JEV read as before,
    shown as "jev (removed)", and the ledger's hash chain is untouched.

### Fixed

- An "always allow" of a shell line with several commands made a rule that
  never matched the same line again. Such lines are refused now.
- A second result no longer finishes a job twice.
- A negative zero spend is never shown.
- The selected session stays selected when the list changes.
