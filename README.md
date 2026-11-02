# swarmux

**A prompt-first command center for Claude Code.**

Write, template, refine and queue prompts. Dispatch them to several Claude Code
sessions and their subagent teams. Watch every agent, model, token, decision and
review live in one terminal.

> Status: early development. Not usable yet.

## Planned

- **Prompt library** with `{{variables}}`, a real editor and one-key refinement
- **Queue & dispatch** across sessions with a concurrency cap and RAM guard
- **Live dashboard**: lead session, subagents, context, tokens, cost, timeline, log
- **Guard**: one permission policy across all sessions and an automatic loop
  breaker, settled in code in under a millisecond so the models never see it
- **JEV (optional, bring your own key)**: the hosted Jev model for fuzzy
  decisions such as unmatched tool calls and which session should take a prompt
- **Let swarmux route**: one cheap Haiku call picks the session for a prompt
- **Review step**: a reviewer model checks each job's diff before it is done
- **History**: every prompt with cost and outcome; re-run or fork from any point

Single Rust binary. Windows, macOS, Linux. Works with Claude Code only.

## Try the skeleton

```sh
cargo run              # TUI: 1-5 switch screens, q quits
cargo run -- doctor    # checks Claude Code, the settings and the Guard hook
```

## Configuration

Optional `~/.swarmux/config.toml` (these are the defaults):

```toml
max_concurrent = 3       # jobs running at the same time
min_free_ram_mb = 1536   # queued jobs wait while free memory is lower
default_model = "sonnet" # model for new sessions
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

## License

MIT
