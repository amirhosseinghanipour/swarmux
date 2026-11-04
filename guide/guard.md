# Guard

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
why (for example `loop guard: Bash failed the same way twice`). With the
[watchdog](watchdog.md) on, the agent is nudged first and stopped one failure
later.

Every decision is logged in the database with its rule and latency; the
Dashboard's green guard panel shows this run's counts and latest decisions.
`swarmux doctor` prints the policy and checks the hook end to end.

