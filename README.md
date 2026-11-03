# swarmux

**A prompt-first command center for Claude Code.**

Write, template, refine and queue prompts. Dispatch them to several Claude Code
sessions and their subagent teams. Watch every agent, model, token, decision and
review live in one terminal.

> Status: early development.

## Features

- **Prompt library** with `{{variables}}` and a real editor
- **Queue & dispatch** across sessions with a concurrency cap, priorities and a RAM guard
- **Live dashboard**: the lead, its subagents (model, context, activity), the
  job's phase from plan to ship, the guard, the reviewer, a per-actor
  timeline, token use per actor and the session log
- **Guard**: one permission policy across all sessions and an automatic loop
  breaker, settled in code in about a millisecond so the models never see it
- **JEV (optional, bring your own key)**: the hosted Jev model for fuzzy
  decisions such as unmatched tool calls and which session should take a prompt
- **Let swarmux route**: one cheap Haiku call picks the session for a prompt
- **Review step**: a read-only reviewer model audits a finished job's diff
- **Done-gate**: when an agent says it is finished, swarmux runs the
  repository's tests itself and sends it back with the failure until they
  pass; tests and other oracles it may not edit are protected
- **Independent verifier**: the checks run in a clean copy swarmux makes of
  the work, not in the agent's worktree, followed by hidden checks the agent
  never sees; edited or switched-off tests block the stop
- **Missions**: state the outcome, the checks that prove it, what the agent
  may not touch and the budget; swarmux lints it (at most three questions),
  runs it as a job under its own gate and calls it delivered only when the
  gate passes
- **Ledger screen**: every run as a card with its outcome, evidence, cost,
  model mix and what became of it (merged, reverted); a digest of what
  happened while you were away; re-run, fork, search or verify from any card
- **Evidence bundles**: every delivered mission leaves a JSON receipt that
  `swarmux verify` re-checks at the head commit in a clean checkout
- **Lean mode** per session: skip your user-level Claude Code settings
- **Audit ledger**: a tamper-evident, hash-chained record of every job,
  agent event, review and guard decision

Single Rust binary. Windows, macOS, Linux. Works with Claude Code only.

## Try it

```sh
cargo run              # TUI: 1-6 switch screens (1 Mission control, 2 Missions), 0 Focus, p sends a prompt, q quits
cargo run -- doctor    # checks Claude Code, the settings, the Guard hook and telemetry
```

## Configuration

Optional `~/.swarmux/config.toml` (these are the defaults):

```toml
max_concurrent = 3       # jobs running at the same time
min_free_ram_mb = 1536   # queued jobs wait while free memory is lower
default_model = "sonnet" # model for new sessions
```

## Telemetry

Background collectors feed the dashboard's stations. Each runs on its own
thread; a source that fails keeps its last reading and shows why. Nothing is
estimated: what isn't known shows as `—`.

- **Spend & burn** from Claude Code's transcripts (`~/.claude/projects`, or
  `$CLAUDE_CONFIG_DIR/projects`), like `ccusage`: cost per day, week, model
  and project, five-hour billing blocks with the burn rate and a projection,
  a 12-week activity calendar and the token flow of the last hour. Only
  usage numbers, model names, timestamps and folders are read, never message
  text. Read positions are cached in `~/.swarmux/usage.db`, so after the
  first pass only new lines are read.
- **Session wall**: every Claude Code session on the machine, live ones
  (active in the last two minutes) first, swarmux's own marked, with each
  session's context use and a near-compaction flag at 80%.
- **Usage limits** from Claude Code's `rate_limit_event` (five-hour and
  weekly windows): use, reset countdown and when the current pace reaches
  the limit. The latest reading is kept across restarts.
- **Jobs**: token flow and cache hit ratio, tool calls (count, average
  time, a live ticker), most read and edited files, the agent tree with each
  agent's current tool, the latest `TodoWrite` list, and each session's MCP
  servers, plugins, skills, agents and Claude Code version.
- **Git** for the selected session's folder (branch, ahead/behind, staged,
  unstaged and untracked files, diff size, last five commits) and **GitHub**
  through `gh` when it is installed and logged in (open pull requests, the
  latest CI run of the branch).
- **System**: CPU per core with history, memory and swap, network rates,
  the home disk, battery, uptime and Claude Code processes.

```toml
[telemetry]
usage = true          # transcripts
system = true
git = true
github = true
usage_secs = 15       # how often each source is read
system_secs = 1
git_secs = 5
github_secs = 60
# transcripts_path = "D:/claude/projects"

# Prices are built in (USD per million tokens); override or add models.
# Cache writes default to 1.25x (5 min) and 2x (1 h) the input price,
# cache reads to 0.1x.
[pricing."claude-opus-5-5"]
input = 4.0
output = 20.0
cache_read = 0.20
```

## Mission control

Screen 1 is where swarmux starts. It answers three questions: does anything
need me, who is doing what, and will I hit a limit.

- **Needs you** (left): held tool calls, failed jobs, done-gates that gave
  up, reviews with concerns, finished results and stalled jobs, most urgent
  first. Only things that need
  a human decision or a look are here; everything else goes to the change log.
- **Wall**: one card per session in a fixed order (by creation), with model,
  permission mode, task, context, activity, todo progress, done-gate chips
  (`✓ cargo test`, `✗ cargo test`, `gate 1/3`, `evidence missing`) and cost.
  Cards never move when a state changes.
- **Top strip**: sessions by state, queue, burn, machine load, the 5-hour and
  weekly limits with a marker for how far through the window you are, and a
  note on any data source that has gone stale: usage, system, git, GitHub,
  `claude agents`, and the sessions' status lines while a job runs.
- **Changes**: what happened, newest first.

The screen fills the window it gets. Up to about 120x36 it shows the strip, the
inbox and a session list. Around 160x48 it adds the wall, the change log and
the token flow. From 220x60 it adds burn, model mix, agents, tools, files, git,
pull requests, MCP servers, system load and a 12-week activity calendar. From
300x80 it adds a peek at the agent the inbox selection points to.

| Key | Does |
| --- | --- |
| `Up` / `Down` | select an item (or a wall card, after `Tab`) |
| `Tab` | switch the arrow keys between the inbox and the wall |
| `Enter` | open the selected item's or card's job in Focus |
| `y` / `n` / `a` | approve, deny or always allow the selected held tool call (from any other screen they answer the oldest) |
| `x` | dismiss an item (held tool calls stay until answered) |
| `A` | approve every ask of the selected one's signature at once (same tool, pattern and tier) |
| `s` | shelve the selected item for `1` 15 min, `2` 60 min or `3` until its job ends |
| `u` | bring every shelved item back |
| `F` | focus mode on or off |
| `r` / `R` | take or turn down an offered rule |

### The inbox as an alarm panel

The inbox follows alarm-management practice, so that it stays small enough
to act on.

- **Risk.** Every held call is scored from how hard it is to undo
  (R0 read, R1 worktree, R2 repository, R3 outside the machine, R4
  irreversible), how far it reaches, how new the pattern is here, how the
  agent's earlier jobs went and, when JEV is on, what it made of the call.
  The score gives a tier: routine, elevated or destructive.
- **Priorities.** P1 is an irreversible call or a secret on its way out;
  P2 is anything that blocks an agent or has broken; P3 is the rest. The
  header counts them, and `swarmux stats alarms` shows the mix against the
  5 / 15 / 80 % target, the rate of new P1 and P2 items against one per ten
  minutes, the busiest ten minutes and how long items took to be looked at.
- **Irreversible calls** are never one key. `y` opens a dialog that arms
  for 1.5 seconds, then wants the target (the branch or path) typed, or
  picked with `Tab` and a digit from a list whose order changes every time.
  They are never batched, never always-allowed and never shelved.
- **Batches.** Asks with one signature (tool, pattern, tier) show `×N`;
  `A` approves them together. Never across tiers, never P1.
- **Floods.** Ten items inside ten minutes is a flood: items are folded by
  cause, the first one in stays and the rest are held out of the list, with
  a banner. P1 is never folded.
- **Focus mode** (`F`) shows `‖ 3 waiting · next ≤1:40` instead of hiding
  things: P1 shows at once, P2 waits for a breakpoint (a prompt sent, a diff
  closed, Focus closed, another card, ten quiet seconds) and at most two
  minutes, P3 until focus ends and at most 45 minutes. Nothing is
  auto-approved.
- **Shelving** hides a P2 or P3 item for a while. It is logged and comes
  back by itself, and the header shows what is shelved.
- **Rules.** After ten identical approvals with no refusal, the ask offers
  `r make it a rule?` (an allow rule such as `Bash(cargo test:*)`).
- **Load.** The header shows how much of the last 15 minutes you spent in
  the inbox. Above 70 % new jobs wait, and the queue says
  `held: you're at 78% load`; reviews keep running. Set the line with
  `load_hold_at` in `[governor]`, or `1.0` to turn the hold off.

## Missions (screen 2)

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

## Ledger (screen 5) and evidence bundles

A mission is delivered only with a bundle. When the gate passes, swarmux
seals `.swarmux/evidence/<mission>.json` (and keeps the same JSON in its
database): the contract, the commit the work ended at (`head_sha`, with the
branch point `base_sha`), the diff's size and hash, the checks the passing
verdict ran (command, exit code, time, the first failing line on a failure),
every gate verdict with the digest of its ledger entry, the checkpoints, the
reviewer's verdict, the cost by phase and by model, and the stretch of the
ledger that tells the story with the hash it ended at. The bundle's digest is
the SHA-256 of its JSON without the signature, and the sealing is itself an
entry in the audit ledger. The signature is only that hash for now: it shows
a bundle was edited, it does not say who wrote it. A mission that cannot be
sealed (no passing verdict on the ledger, no git repository) is escalated
instead of delivered, and missions delivered before bundles existed get one
the next time swarmux starts.

```sh
swarmux verify fast-search          # by mission
swarmux verify .swarmux/evidence/fast-search.json
```

`verify` checks the signature, the stored copy, the sealing entry and the
whole ledger chain, then makes a temporary clean checkout of `head_sha`
(`git worktree add --detach`, removed afterwards), runs each check there and
compares: a check that passed must pass again, one that failed must fail with
the same exit code. It prints `match` or `mismatch` and exits 1 on a
mismatch. Work that was still uncommitted when the bundle was sealed cannot
be reproduced and is reported as a mismatch; jobs in their own worktree
(the default inside a git repository) are committed at each gate pass, so
they are covered.

The ledger screen shows a card per run (outcome, title, cost; then time,
checks passed and the bundle's digest, the models paid, the label). The
strip on top gives pass^3 (the chance of three delivered missions in a row
at the current rate), delivered over ended, dollars per verified outcome,
gate blocks and hand-offs per mission, and a 28-day activity strip; under it
the digest of what happened while you were away (since the previous run
ended, when that was over 30 minutes ago), else the last 24 hours.
Outcome labels come from `gh`: a pull request found for the branch is `pr
open`, `merged` or `closed`, and a merge that a merged revert pull request
undid within seven days is `reverted`; each new label is appended to the
ledger.

| Key | Does |
| --- | --- |
| `/` | search titles, outcomes, labels and prompts |
| `o` | filter: all, delivered, needs you, labelled |
| `v` | verify the selected bundle in the background |
| `l` | ask GitHub what became of the delivered branches |
| `r` / `f` | re-run / fork the job |
| `Enter` | open it in Focus |

```sh
swarmux brief --hours 12   # what happened lately, as text
swarmux labels             # sync the outcome labels now
```

## Focus (`0`)

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

## Themes & layouts

swarmux paints its own background (the default `reference` theme is OLED
black, with orange for Opus/Sonnet and the lead, blue for Haiku and green for
the Guard), so your terminal's colour scheme never shows through.

| Key | Does |
| --- | --- |
| `t` | next theme (on Focus `t` opens the session in a terminal tab instead): `reference`, `catppuccin-mocha`, `tokyo-night`, `gruvbox-dark`, `rose-pine`, `nord`, `dracula` |
| `L` | next layout: `command` (the dashboard above), `btop` (a dense grid of small panels), `minimal` (lead and log), then yours |
| `Tab` / `Shift+Tab` | move the focus highlight between panels (`Esc` clears it) |

```toml
theme = "tokyo-night"   # any name above
layout = "btop"         # command, btop, minimal or one of [[layouts]]
icons = "nerd"          # "ascii" (default), "unicode" or "nerd" for Nerd Font glyphs
gaps = 1                # blank cells between panels, 0-4
fps = 10                # frames per second, 1-30
low_power = false       # true: 4 fps, no easing or pulses
splash = true           # the start-up screen (or run with --no-splash)

[[layouts]]             # your own: rows top to bottom
name = "focus"
rows = [
  { panels = ["steps"], height = 1 },
  { panels = ["lead:2", "guard", "usage"], height = 8 },
  { panels = ["timeline:3", "who:2"], weight = 1 },
  { panels = ["log"], weight = 2 },
]
```

A row is `height` rows tall, or shares what is left by `weight`; `"id:n"`
gives a panel `n` shares of its row. Panel ids: `legend`, `command`, `steps`,
`target`, `reviewer`, `lead`, `guard`, `delegate`, `subagents`, `back`,
`timeline`, `who`, `log`, `mix` (tokens by model as a ring), and the
telemetry panels `usage`, `burn`, `tokens`, `context`, `agents`, `todos`,
`tools`, `files`, `git`, `ci`, `system`, `wall`, `calendar`, `env`, which
show "waiting for data" until their feed is connected. A panel too small for
its full view collapses to one line; when even that doesn't fit, the least
important panels are hidden first.

Colours are truecolor when `COLORTERM` is `truecolor` or `24bit` (or inside
Windows Terminal) and fall back to the 256-colour palette otherwise.

## Review step

Press `v` on the Dashboard to have the next jobs reviewed (or set
`[review] enabled = true`). When a reviewed job succeeds, swarmux queues a
second, lean, read-only run in the same folder: `claude -p --model <review
model> --tools Read,Grep,Glob --no-session-persistence --setting-sources
project,local --strict-mcp-config`. It gets the prompt, the job's answer
and the `git diff` (uncommitted changes in tracked files, capped at 48 KB)
and must answer `{"verdict": "pass" | "concerns", "concerns": [...],
"diffs_checked": n}`. The verdict and its cost are stored with the job and
shown on the Dashboard and in the Ledger. A review that fails or answers
something else shows as `review failed: …`; it never changes the job's own
result.

The reviewer is gated by risk: a change is scored the way the inbox scores a
tool call, and only an elevated one gets a reviewer. That is a big change
(around 300 lines or more), one that deletes files, touches builds,
dependencies, pipelines, migrations, secrets or `.claude/`, or edits or
switches off existing tests. A routine change shows `review skipped` with
the score and why; set `when = "always"` to review every job.

```toml
[review]
enabled = false   # review new jobs by default (v toggles it while running)
model = "opus"    # the reviewer's model
when = "risk"     # "risk": only changes whose risk is elevated; "always": every job
```

## Guard

Every tool call a swarmux job makes goes through the Guard first. swarmux adds
a `PreToolUse` hook to each `claude` run (with `--settings`, never by editing
your own settings files). The hook runs `swarmux hook pre-tool`, which asks the
running swarmux over a local socket (a named pipe on Windows, a Unix socket
elsewhere) and prints the answer. Allowed and denied calls are answered at
once, in about a millisecond plus process start-up. If swarmux isn't
reachable, the hook prints nothing and Claude Code's own rules apply.

Rules use Claude Code's syntax:

| Rule | Matches |
| --- | --- |
| `Bash` | any use of the tool |
| `Bash(cargo test:*)` or `Bash(cargo test *)` | `cargo test` with or without more arguments |
| `Bash(git status)` | exactly that command |
| `Edit(src/**)`, `Read(**/*.md)`, `Write(/notes.txt)` | paths relative to the session folder; a bare name like `Read(.env)` matches at any depth; `//abs/path` is absolute |
| `WebFetch(domain:example.com)` | that host (`*.example.com` for subdomains) |

`Edit(...)` rules also cover `Write`, `MultiEdit` and `NotebookEdit`. Paths use
`/` on every system and Windows paths are compared without regard to case. A
shell line like `a && b | c` is allowed only when every command in it is
allowed, and command substitution (`$(...)`, backticks) is never allowed by a
prefix rule. Deny rules always beat allow rules.

```toml
[guard]
allow = ["Bash(cargo test:*)", "Edit(src/**)"]
deny = ["Read(.env)", "WebFetch"]
unmatched = "ask"            # or "pass": leave unmatched calls to Claude Code
approval_timeout_secs = 120  # unanswered approvals are denied after this
loop_repeats = 2             # 0 turns the loop breaker off
```

Built-in deny rules apply whatever the config says: `Bash(rm -rf /:*)`,
`Bash(rm -rf /*:*)`, `Bash(rm -rf ~:*)`, `Bash(rm -rf ~/:*)`,
`Bash(git push --force:*)` and `Bash(git push -f:*)`. They also catch these
commands after `sudo`, `env` or `VAR=value`, inside `&&` chains and inside
`$(...)`. They are a safety net, not a sandbox.

Built-in allow rules cover read-only and bookkeeping tools: `Read(**)` (inside
the session folder), `Glob`, `Grep`, `LS`, `TodoWrite`, `Task`, `Agent`,
`TaskOutput`, `ToolSearch` and `NotebookRead`. Deny rules override them.

**Approvals.** With `unmatched = "ask"`, a call no rule matches waits for you.
A panel shows the tool, what it acts on, the job and session and a countdown
on every screen: `y` approves, `n` denies, `a` always allows calls exactly like
it. An always-allow rule applies at once and is added to `[guard] allow` in
`config.toml` when the file's layout is simple enough to edit safely;
otherwise it lasts for this run and swarmux says so.

**Loop breaker.** When an agent makes the same tool call and it fails with the
same error `loop_repeats` times in a row, swarmux stops the job and records
why (for example `loop guard: Bash failed the same way twice`).

Every decision is logged in the database with its rule and latency; the
Dashboard's green guard panel shows this run's counts and latest decisions.
`swarmux doctor` prints the policy and checks the hook end to end.

## Lean mode

Press `l` on the Sessions screen to make a session lean. Its jobs run with
`--setting-sources project,local`, so your user-level Claude Code settings
(hooks, plugins) don't run inside them, which makes jobs start much faster.
Project and local settings and swarmux's own Guard hook still apply. Lean
sessions are marked `lean` in the list.

## Let swarmux route

Press `P` on the Dashboard (instead of `p`) or `D` on the Prompts screen
(instead of `d`) and swarmux picks the session. The status bar says
`routing…`, then the chosen session and a one-line reason, and the job's log
and the guard / JEV panel record it before the prompt is queued.

Routing makes one cheap `claude -p --model haiku` call with no tools, no
session file and none of your user-level settings. It reads the first 2000
characters of the prompt and, for each session, its name, folder, model and
latest three prompts, and answers with a session and a model suggestion
(shown, not applied). The call's cost (usually well under $0.001) is shown
next to the reason. If Haiku fails or names an unknown session, the selected
session takes the prompt and swarmux says why. Routing uses your Claude plan.

## JEV (optional, bring your own key)

JEV is a hosted decision model from TypeSafe AI, called the way the
open-source [jev-layer](https://github.com/typakon4/jev-layer) project calls
it. swarmux can ask it two fuzzy questions:

- a tool call no Guard rule matches: allow or deny, with a confidence;
- "let swarmux route": which session, with a confidence (Haiku is only asked
  when JEV isn't sure).

Answers at or above `threshold` are applied and logged as `jev` with their
confidence and latency; anything else (lower confidence, no key, a timeout,
an HTTP error, an invalid reply) falls back to asking you, or to Haiku, and is
logged too. JEV is asked off the UI thread and never holds up the screen.

```toml
[jev]
provider = "typesafe"   # "off" (default), "typesafe" or "openrouter"
threshold = 0.85        # answers below this go to you instead
# timeout_ms = 2000     # default: 2000 for typesafe, 5000 for openrouter
```

| Provider | Endpoint | Model | Key |
| --- | --- | --- | --- |
| `typesafe` | `https://api.typesafe.ai/v1/systemone` | `jev-1.14.0` | `TYPESAFE_API_KEY` |
| `openrouter` | `https://openrouter.ai/api/alpha/decisions` | `typesafe/jev-1.14` | `OPENROUTER_API_KEY` |

Keys are read only from those environment variables or the OS keychain,
never from `config.toml`, and are never written to the database or logs.

```sh
swarmux jev set-key typesafe   # asks for the key without echoing it, keeps it in the keychain
swarmux jev test               # one sample decision: verdict, confidence, latency
swarmux doctor                 # shows the provider and whether a key is found
```

**Privacy.** With JEV on, decision context goes to the chosen provider: the
tool name and a summary of its command, path or URL (at most 200
characters), or the first 200 characters of a prompt and your session names.
Never file contents, diffs or folders. The first time a provider is enabled,
swarmux shows exactly this and the host, and uses JEV only after you press
`y`; the answer is remembered per provider.

## Done-gate

An agent's "done" is not taken on its word. Every `claude` job gets `Stop`
and `SubagentStop` hooks; when the agent tries to finish, swarmux:

1. **Runs the repository's checks** itself, in a clean copy of the work (see
   [Independent verifier](#independent-verifier-and-hidden-checks); outside a
   git repository, in the repository root), each as a child process: the
   program and its arguments directly, never through a
   shell, with no console window, killed (with everything it started) at its
   timeout, output kept to 64 KB. A check that fails is run once more; passing
   then marks it *flaky* instead of failing the gate. Checks run only when
   the session used a tool that can change files since the last pass.
2. **Compares protected files with their hashes** from the start of the
   session, so an edit that got past the Guard is still caught.
3. **Looks for evidence**: if the last message says the tests pass or names a
   test command, that command's latest run in the job's stream must have
   succeeded.

Anything wrong blocks the stop: the agent continues with a short digest (the
failing check, its first failing lines verbatim, the changed protected files,
the claims nothing backs). After `max_blocks` blocks the agent is let go and
the job shows up in *Needs you*. Every verdict is recorded in the audit
ledger and shows as chips on the job's wall card.

Writes to **protected paths** are refused by the Guard before any rule is
looked at: `Edit`/`Write`/`MultiEdit`/`NotebookEdit` on them, and shell
commands that redirect into them or remove, move, copy over or edit them in
place. Reading and running them is fine. `.swarmux/**` (the gate's own
configuration) is always protected.

Without a `.swarmux/gates.toml` the checks are guessed from the files at the
repository root:

| Found | Check |
| --- | --- |
| `Cargo.toml` | `cargo test --workspace --quiet` |
| `go.mod` | `go test ./...` |
| `package.json` with a real `test` script | `pnpm test` with `pnpm-lock.yaml`, else `npm test` |
| `pytest.ini`, `conftest.py`, or pytest settings in `pyproject.toml` / `setup.cfg` / `tox.ini` | `pytest -q` |

`.swarmux/gates.toml` (commit it with the repository):

```toml
[gate]
max_blocks = 3            # blocks before the user decides (0: never block)
evidence = true           # claimed test runs must appear in the stream
subagent_checks = false   # subagents' stops: protected files and evidence only
detect = true             # with no [[check]], guess the checks as above
protected = ["tests/oracles/**", "golden.json"]   # relative to the root; a bare name matches anywhere

[[check]]
name = "tests"
run = ["cargo", "test", "--workspace"]   # a list of arguments: no shell
timeout_secs = 600

[[check]]
name = "lint"
run = ["cargo", "clippy", "--", "-D", "warnings"]

[verifier]
clean = true              # run the checks in a clean copy (false: in the worktree)
test_edits = "block"      # "block": edited or switched-off tests block the stop; "allow"
```

The repository root is the nearest folder with a `.swarmux/gates.toml`, else
the nearest with `.git`. A `Stop` hook may take up to 30 minutes; a gate
still running then lets the agent stop.

```sh
swarmux gate show            # the gate that applies here: checks, protected paths, limits
swarmux gate run             # run the checks now, as at a stop (recorded in the ledger); exits 1 if one fails
swarmux gate run --dir ../x  # for another repository
swarmux gate run --in-place  # in the folder itself: no clean copy, no hidden checks
```

## Independent verifier and hidden checks

An agent that can see the referee can game it: edit the test until it
passes, switch it off, or special-case the one input it checks. So the
gate's checks are not run where the agent works.

- **A clean copy, owned by swarmux.** At each judged stop, everything the
  worktree holds (committed or not, but nothing git ignores) is written into
  a commit of its own through a throwaway index, and that commit is checked
  out under `~/.swarmux/verify/<repo>/`. The agent's files, index and branch
  are not touched; build output, caches and files planted in ignored folders
  never get there. Copies of one repository share a cargo build folder, so a
  check after the first is an incremental build. The copy is removed after
  the checks.
- **Hidden checks.** Kept in `~/.swarmux/holdout/<repo>/<check>/`, outside
  every repository. After the visible checks pass or fail, each hidden check's
  files are copied into the clean copy and it runs there. The agent, the
  evidence bundle and the ledger get only how many ran and how many failed;
  never a name, a command, a file or a line of output. Tool calls that name
  `~/.swarmux/holdout`, `~/.swarmux/verify` or run `swarmux holdout` are
  refused by the Guard, reads included.
- **Oracle pinning, twice.** Protected files are compared with their hashes
  from the start of the session in the worktree, and, through git, the clean
  copy is compared with the commit the job started from, so a committed edit
  or a protected file moved away counts too. Each hidden check's files are
  pinned by SHA-256 when added; a file that no longer matches fails its check.
- **Tamper scan.** The change since the job started is read for lines of
  existing tests removed or changed (moved lines don't count, new tests are
  fine) and for tests switched off in code: `#[ignore]`, `@pytest.mark.skip`,
  `t.Skip`, `it.skip`, `.only`, `@Disabled` and the like. Either blocks the
  stop unless `[verifier] test_edits = "allow"`.
- **Told up front.** A mission's prompt says how it will be judged: no
  editing or weakening tests, no skips, no special-casing, and that some
  checks are hidden.

The wall card and Focus's gate lane show `hidden 2/3` and `tests tampered`.
`swarmux verify` runs a bundle's hidden checks again (all must pass).

```sh
# a hidden test file and the command that runs it; DEST=SOURCE, DEST relative to the repo root
swarmux holdout add . bigger-inputs --file tests/hidden_add.rs=../keep/hidden_add.rs -- cargo test --test hidden_add
swarmux holdout add my-repo api-contract --timeout 300 -- npm run contract
swarmux holdout list          # every repository's hidden checks (for you, not the agents)
swarmux holdout list .        # this repository's
swarmux holdout rm . bigger-inputs
```

`<repo>` is a folder inside the repository (a worktree counts as its main
repository) or the repository's folder name, lowercased. Keep the source of a
hidden file out of the repository: anything in the worktree, the agent can
read.

## Worktrees and checkpoints

Inside a git repository every job runs in its own worktree, on its own branch
`swarmux/<session>/<n>`, under `~/.swarmux/worktrees`. Two jobs never edit the
same files, and each gets a block of ports so dev servers don't collide:
`SWARMUX_PORT_BASE` is the first port, `SWARMUX_PORT_COUNT` how many are yours
(also `SWARMUX_WORKTREE`, `SWARMUX_BRANCH`, `SWARMUX_JOB`). Files listed in
`copy` (by default `.env`, `.env.local` and `.claude/settings.local.json`) are
copied from the repository into each worktree when they exist; they are never
committed by swarmux. Outside a git repository nothing changes: the job runs
in the session's folder. A session can be set to `on`, `off` or `auto` (the
default: on inside a repository).

When the done-gate passes, and before Claude Code compacts its context,
swarmux takes a checkpoint: the Claude session id, a commit holding the
worktree's files, the ledger entry that records it, and a handoff (goal,
state, what failed, next step). The handoff is handed to the session that
starts next, so a resumed or compacted session picks up where the last one
stopped. A job interrupted by quitting swarmux continues in its own worktree
and Claude session when you retry it; at most the step that was running is
lost. Reclaiming a worktree first commits what is left in it, so the branch
always keeps the work.

```toml
[worktree]
# root = "D:/swarmux-worktrees"            # default: ~/.swarmux/worktrees
copy = [".env", ".env.local", ".claude/settings.local.json"]  # globs allowed in the file name
port_base = 20000     # first port handed out
port_block = 20       # ports per job
port_slots = 500      # jobs that can hold a block at once
stale_hours = 12      # worktrees of jobs that ended this long ago are reclaimed
```

```sh
swarmux worktree list                 # worktrees, their ports, branches and state
swarmux worktree gc [--dry-run]       # reclaim stale worktrees (also runs at start and hourly)
swarmux worktree checkpoints [JOB]    # checkpoints of a job, or the newest ones
swarmux worktree mode <session> on|off|auto
```

## Usage governor

Before a queued job starts, swarmux looks at the subscription windows (5-hour
and weekly) as the status lines and the jobs' own `rate_limit_event`s report
them, and forecasts where each is heading at the current pace:

- **Hold.** While a window is at `hold_at` or above, new jobs wait. The queue
  says why and until when (`deferred: 5h at 97%, resumes 15:58`) and they start
  by themselves once the window resets. Reviews are verification: they keep
  running until the window is actually full.
- **Downgrade.** From `downgrade_at`, or when the forecast reaches `hold_at`
  within `horizon_minutes`, work on a pricier model runs on `downgrade_to`.
- **Auto-resume.** A job a usage limit stopped is resumed in its own Claude
  session and worktree after the window resets, also across a restart of
  swarmux.
- **Budget.** Each job is told its cap and the windows in one line after its
  prompt, and runs with `--max-budget-usd` when it has a cap. A mission's job
  may spend the planning and implementation shares of its budget; the
  verification (20%) and review (10%) shares stay reserved.
- **Battery.** On battery at most `battery_concurrency` jobs run at once.

```toml
[governor]
enabled = true
hold_at = 0.97            # new jobs wait from here until the window resets
downgrade_at = 0.90       # pricier models run on downgrade_to from here
downgrade_to = "sonnet"
horizon_minutes = 30      # how far ahead a window is forecast
battery_concurrency = 2
# max_job_usd = 5.0       # --max-budget-usd for every job (missions use their own budget)
budget_line = true        # tell each job its budget
load_hold_at = 0.70       # new jobs wait while you spend more of your time than this in the inbox
```

## Audit ledger

Everything swarmux stores about a job (queued, started, each agent event,
the review, how it finished) and every guard, loop-breaker, JEV and routing
decision is also appended to a ledger in `~/.swarmux/swarmux.db`. Each entry
holds the SHA-256 of the previous entry plus its own content, so editing,
removing or reordering anything already recorded breaks the chain. Only
swarmux writes it; the agent runner never gets a handle on it, and the table
refuses changes and deletions. The chain has no secret key: someone who
rewrites every entry after an edit gets a valid chain with a different head,
so keep a head hash you printed earlier to compare against.

```sh
swarmux audit verify      # walks the chain; prints the head hash, exits 1 at the first break
swarmux audit tail -n 20  # the newest entries
swarmux doctor            # includes the ledger head and whether it verifies
```

Long texts (prompts, answers, tool output) are kept up to 8 KB per field,
with the SHA-256 and length of the full text.

## Notifications

When something needs you and swarmux is in the background, it can tell you.
It only does so for the inbox's own priorities: a **P1** item always notifies
(unless the kill switch is off), a **P2** item notifies once, and only while
the terminal window is unfocused and outside quiet hours; **P3** never does.
More than three new items at once become a single summary. Items that are
held back by deferral are not sent until they are due.

Focus comes from the terminal's focus reports. A terminal that sends none
counts as unfocused after 30 seconds without a key.

```toml
[notify]
enabled = true            # kill switch: false silences everything, P1 included
toast = true              # native Windows toast (no PowerShell process); default on Windows
terminal = "osc777"       # "off", "osc777", "osc9" or "bell"; default off on Windows, osc777 elsewhere
quiet = "22:00-08:00"     # local time; P2 stays silent (and is not sent later). P1 ignores it
# ntfy = "https://ntfy.sh/my-private-topic"   # opt-in push to your phone
```

- `terminal` writes a notification escape sequence to the terminal:
  `osc777` for VTE, foot, WezTerm and Ghostty; `osc9` for iTerm2, ConEmu and
  Kitty; `bell` for the plain bell. Pick one, not both.
- `ntfy` is off until you set a topic URL. Only a short title (`swarmux P2`)
  and the kind of item (`an approval is waiting`) are sent: never a command,
  path, prompt or session name. Use a topic name nobody can guess.
- The toast uses the PowerShell application id so Windows shows it without
  an installed app; swarmux does not start PowerShell.

## License

MIT
