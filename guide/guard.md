# Guard

The Guard checks the tool calls Claude Code reports, before they run. swarmux adds
a `PreToolUse` hook to each `claude` run (with `--settings`, never by editing
your own settings files). The hook runs `swarmux hook pre-tool`, which asks the
running swarmux over a local socket (a named pipe on Windows, a Unix socket
elsewhere) and prints the answer. Allowed and denied calls are answered at
once, in about a millisecond plus process start-up.

What the Guard sees is the tool calls Claude Code reports to that hook, and
nothing else. What it does when swarmux can't be reached is a setting
([below](#when-swarmux-cant-be-reached)). What it cannot do is listed under
[Limits](#limits).

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

on_unreachable = "auto"      # "auto", "deny", "ask" or "pass": see below
always_scope = "repo"        # "repo", "workspace" or "global": where "always allow" applies
always_expiry_days = 30      # 1 to 3650, or "never"
protected_paths = ["deploy/**"]  # more files whose writes are held, besides the built-in ones
hook_on_failure_block = false    # see "onFailure" below
```

The last five keys are read line by line: a value that isn't valid is replaced
by its default, the status bar and `swarmux doctor` say which line, and
`swarmux guard lint` lists every such line (exit 1) with the values in effect.
The rest of the file is read as before.

Built-in deny rules apply whatever the config says: `Bash(rm -rf /:*)`,
`Bash(rm -rf /*:*)`, `Bash(rm -rf ~:*)`, `Bash(rm -rf ~/:*)`,
`Bash(git push --force:*)` and `Bash(git push -f:*)`. They also catch these
commands after `sudo`, `env` or `VAR=value`, inside `&&` chains and inside
`$(...)`. They are a safety net, not a sandbox.

Built-in allow rules cover read-only and bookkeeping tools: `Read(**)` (inside
the session folder), `Glob`, `Grep`, `LS`, `TodoWrite`, `Task`, `Agent`,
`TaskOutput`, `ToolSearch` and `NotebookRead`. Deny rules override them.

## Irreversible calls

A call that can't be taken back (a delete, a force push, a history rewrite:
risk R4) is allowed by one thing only: you typing the name of what it acts on,
after the prompt has armed for 1.5 seconds. Nothing else resolves it to allow:
not an allow rule in `config.toml` (even a bare `Bash`), not an "always
allow", not an accepted rule offer, not `unmatched = "pass"`, not a model
of any kind, not a timeout (which denies). A call such a rule would have
allowed is held for you instead, and the reason says so.

This is enforced in the Guard (`GuardService`), not in the key handler: the
Guard rebuilds the confirmation from the held call and checks what was typed,
so another client of the Guard, now or later, can't get around it.

## Approvals

With `unmatched = "ask"` (the default), a call no rule matches waits for you:
no model is asked, in swarmux or anywhere else. With `unmatched = "pass"` it
goes back to Claude Code's own rules. A panel shows
the tool, what it acts on, the job and session (and the subagent, when the call
was made by one) and a countdown on every screen: `y` approves, `n` denies,
`a` always allows.

**Always allow** makes a narrow rule and keeps it with where it applies, who
made it and when it stops:

- *Scope.* By default the rule applies to the repository the call was made in
  (a job's worktree counts as its repository). `always_scope = "workspace"`
  applies it to the job's workspace, `"global"` everywhere; choosing global is
  choosing it for every repository.
- *Expiry.* 30 days by default; `always_expiry_days = "never"` is the explicit
  choice for a rule that doesn't expire. An expired rule no longer applies.
- *What it refuses.* An irreversible call, a shell line of several commands (or
  with `$(...)`, which one exact rule could never match again) and any call whose
  rule would be the bare tool name (an MCP tool, `WebSearch`, a tool swarmux
  doesn't know): those can be approved once. For the rest, write the rule you
  mean in `config.toml`.
- *Where it lives.* `guard-rules.toml` next to `config.toml`, one `[[rule]]` each
  with `id`, `rule`, `scope`, `root`, `source`, `created_at` and `expires_at`.
  Adding and removing a rule is an entry in the ledger (kind `policy`, event
  `added`, `removed` or `expired`, with the rule's hash).
- *The offered rule.* After ten clean approvals of the same ask, swarmux offers
  a broader rule (`r`). It is saved for the repository the approvals came from.

```sh
swarmux guard rules list          # config.toml rules and added rules, with scope and expiry
swarmux guard rules remove <id>   # remove an added rule (ledgered)
swarmux guard rules prune         # remove the expired ones (ledgered)
swarmux guard lint                # settings replaced by defaults, and the values in effect
```

Rules in `config.toml` `[guard] allow` keep working as they did: they apply
everywhere and never expire, and `list` shows them as `source config`. Edit
them in the file. Rules an earlier version appended there when you pressed `a`
can't be told from rules you wrote, so they stay there as global, permanent
rules until you remove them; an `a` from now on makes a scoped, expiring rule
in `guard-rules.toml` instead.

## When swarmux can't be reached

A hook that can't reach the app (not running, a connection that fails, no
answer) used to print nothing, and Claude Code carried on under its own
rules. That is now a setting, because for a job nobody is watching it means the
Guard is not there:

| `on_unreachable` | Unattended jobs | Other jobs |
| --- | --- | --- |
| `"auto"` (default) | denied | left to Claude Code's own rules |
| `"deny"` | denied | denied |
| `"ask"` | Claude Code asks (in `claude -p`: a refusal) | the same |
| `"pass"` | left to Claude Code's own rules | the same |

*Unattended* means jobs nobody is watching: a mission's jobs, reviewer jobs
and the jobs that continue them (send back, resume, retry). A job started from
your own prompt is attended. Each job's mode is passed to its hook in
`SWARMUX_GUARD_FAIL` (`closed`, `ask` or `open`); a denial reads "swarmux
unreachable (...); unattended jobs fail closed". If the app goes away while a
call is held, it answers the same way.

Every fallback, whichever mode, is a line in a spool file in the state folder
(`guard-fallback.jsonl`); swarmux puts the spool in the ledger the next time it
starts, as a `policy` entry with event `guard_fallback` (when, job, tool, a hash
of the hook input, why, what was printed). A fallback is therefore on record
only after that restart, and a spool the hook couldn't write is not.
`swarmux doctor` shows the mode and runs the hook against an unreachable app in
each mode. swarmux says at every start what the default means.

**`onFailure`.** Recent Claude Code versions accept `"onFailure": "block"` on a
hook, which makes a hook that crashes or times out block the call. With
`hook_on_failure_block = true` the key is added to the PreToolUse hook. swarmux
does not check which Claude Code versions know it (that arrives with the
version check); versions that don't know the key ignore it. Off by default.

## What the Guard keeps from agents

- **swarmux's own command line.** A shell command that runs swarmux (by name, by
  path, or as the running binary under another name, through `sudo`, `env`,
  `xargs`, `sh -c '...'` or `$(...)`) is denied unless the subcommand is `hook`,
  `board-mcp` or `statusline`. `swarmux audit checkpoint`, `swarmux daemon stop`,
  `swarmux gate`, `swarmux guard rules remove` and the rest are yours. The old
  checks on `swarmux knowledge`, `rules` and `holdout` stay as a second net.
- **swarmux's files.** The database, the audit key (when it is a file), `config.toml`,
  `guard-rules.toml`, the spool, the hidden checks, the verifier's copies and the
  status-line spool can be neither read nor written by a tool call. Paths are
  expanded (`~`, `$HOME`, `%USERPROFILE%`), made absolute, resolved (`..`,
  symlinks) and compared by path component, not by substring.
- **Files that steer agents.** A write to `.claude/settings*.json`,
  `.claude/hooks/**`, `.claude/commands/**`, `.claude/agents/**`, `.mcp.json`,
  `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md`, `.git/hooks/**` or `.swarmux/**` is held for you
  (denied for an unattended job), whatever the allow rules say; reading them is
  not held. `protected_paths` adds your own globs (relative to the repository).

## Limits

This is text matching on the calls Claude Code reports, not a sandbox. Agents
run as your operating-system user: a determined one finds another spelling of a
command or a path, and a tool Claude Code doesn't report to the hook is not
seen at all. The rules and the protected paths are matched as text, so a path
or command the matching does not recognise is not held. What the Guard gives
you is that the plain ways are refused, and that the decisions it makes and the
fallbacks the hook can record are in the ledger (a fallback only after the next
start of swarmux, see above). Rules that match shell text are a safety net.

**Loop breaker.** When an agent makes the same tool call and it fails with the
same error `loop_repeats` times in a row, swarmux stops the job and records
why (for example `loop guard: Bash failed the same way twice`). With the
[watchdog](watchdog.md) on, the agent is nudged first and stopped one failure
later.

[Knowledge rules](knowledge.md) with a `guard` enforcer are consulted after
the protected paths and before these rules. They only forbid: a rule can
deny a call or hold it for you as a P1 approval that shows the rule, never
allow one, and a deny here still wins over a rule's ask.

Decisions the Guard makes while swarmux is running are logged in the database
and the ledger with the rule (and its hash), the latency, who resolved a held
call (a key, a typed confirmation, a rule, a timeout) and the subagent, when
there was one. The Dashboard's green guard panel shows this run's counts and
latest decisions. `swarmux doctor` prints the policy and checks the hook end to
end.
