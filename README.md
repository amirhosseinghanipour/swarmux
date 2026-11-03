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
- **History**: every prompt with cost, outcome and review; re-run or fork from any point
- **Lean mode** per session: skip your user-level Claude Code settings
- **Audit ledger**: a tamper-evident, hash-chained record of every job,
  agent event, review and guard decision

Single Rust binary. Windows, macOS, Linux. Works with Claude Code only.

## Try it

```sh
cargo run              # TUI: 1-6 switch screens (1 is Mission control), p sends a prompt, t theme, L layout, q quits
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
  note on any data source that has gone stale.
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

## Focus (screen 2)

Screen 2 is one agent in detail: the job you last sent, or the one you open
with `Enter` from Mission control, the queue or history. It opens with the same
sigil and colour the card had. From top to bottom:

- **Header**: session, state, the prompt, model, permission mode, folder,
  time, cost and context.
- **Situation**: `doing`, `because`, `next`, how `sure` swarmux is and what it
  is `unsure` about. All of it comes from what swarmux saw: the last tool
  call, the todo list, the done-gate, the review, loop stops and failed tool
  calls. Nothing is guessed by a model.
- **Gate**: the done-gate's checks and the review as chips, then the
  checkpoints (empty until the job has any) and the recovery trail (loop-breaker
  stops).
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
shown on the Dashboard and in History. A review that fails or answers
something else shows as `review failed: …`; it never changes the job's own
result.

```toml
[review]
enabled = false   # review new jobs by default (v toggles it while running)
model = "opus"    # the reviewer's model
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

1. **Runs the repository's checks** itself, in the repository root, each as a
   child process: the program and its arguments directly, never through a
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
```

The repository root is the nearest folder with a `.swarmux/gates.toml`, else
the nearest with `.git`. A `Stop` hook may take up to 30 minutes; a gate
still running then lets the agent stop.

```sh
swarmux gate show            # the gate that applies here: checks, protected paths, limits
swarmux gate run             # run the checks now (recorded in the ledger); exits 1 if one fails
swarmux gate run --dir ../x  # for another repository
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

## License

MIT
