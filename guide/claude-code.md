# Claude Code: launch profiles, versions and conformance

swarmux starts Claude Code (`claude -p`) for every job. This page says how, how
much of your and the repository's own Claude Code configuration a job loads,
which Claude Code versions the checks were run against, and the canaries that
try the Guard against the Claude Code you have installed.

Everything on it is marked by how it is known:

- **observed**: seen in a real `claude` call on Claude Code 2.1.323, on the
  machine that wrote this page (the spike, below);
- **docs**: stated in Claude Code's own documentation (read 2026-11-14) and not
  run;
- **not verified**: neither.

Claude Code changes often (more than a hundred releases in four months), so a
behaviour here is a statement about the versions named, not about later ones.
`swarmux doctor --conformance` tries the ones swarmux relies on against the
version you have.

## Launch profiles

`[runtime] profile` chooses how much configuration a job loads.

| Profile | Settings files loaded | MCP servers | Used for |
| --- | --- | --- | --- |
| `inherit` | all of Claude Code's: yours, the repository's `.claude/settings.json`, `.claude/settings.local.json` | all of yours and the repository's `.mcp.json` | jobs you start yourself |
| `controlled` | only `controlled_sources` (`["user"]` by default) | only swarmux's: the Board's, and the ones you list under `[runtime.mcp.*]` (`--strict-mcp-config`) | jobs nobody watches, in a repository you have not trusted; every reviewer |

`profile = "auto"` (the default) picks `controlled` for an unattended job (a
mission's, and the jobs that continue it) in a repository that is not trusted, and `inherit` for everything else. `"inherit"` and
`"controlled"` force one for every job; a reviewer is always controlled, and
loads only `reviewer_sources` (none by default), so what an agent wrote into
`.claude/settings.json` never reaches the agent that checks its work. One job
can be given a profile of its own (`JobService::dispatch_with_profile`); a
mission's own choice has no place in the contract yet.

### What a controlled job loses

*Observed* (`--setting-sources user`): the repository's `CLAUDE.md` is **not**
loaded. A repository that keeps its instructions there gives a controlled job
none of them. *Docs*: the same flag leaves out `.claude/settings.json`,
`.claude/settings.local.json` and `.mcp.json`. So a controlled job also loses
the repository's own hooks, skills in `.claude/`, permission rules and `env`.
Two ways to give them back: trust the folder, or add `"project"` to
`controlled_sources` (which loads all of the repository's, and is the same as
trusting every repository).

### Trust

Whether a repository may load its own configuration is a question for a trust
record, which is not built yet. Until it is, the folders you list in
`config.toml` stand in:

```toml
[runtime.repos."/work/app"]
trusted = true          # this folder and everything inside it
```

The seam is `swarmux_application::runtime::RepoTrust`; `ConfiguredTrust` is the
interim implementation. A profile chosen for the job, `controlled` in the
setting, and the reviewer never depend on trust. A repository can narrow what
runs in it and never widen it: nothing in a repository file turns a controlled
job into an inherit one.

### The arguments

The golden tests (`claude_cli/command.rs`) pin these. `inherit`, as swarmux has
always started it, with `--setting-sources project,local` added for a
[lean](sessions.md) session:

```
-p --output-format stream-json --input-format stream-json --verbose
--forward-subagent-text --session-id <id> --model <m> --permission-mode <mode>
```

`controlled` adds:

```
--setting-sources user --strict-mcp-config
```

(`--setting-sources ""` for a lean session, which loads none), and the
`--mcp-config` swarmux generates: the Board's server, if the job is on a
board, and the servers you allowed. A reviewer is always:

```
-p --output-format stream-json --input-format stream-json --verbose --model <m>
--tools Read,Grep,Glob --permission-mode default --max-turns 12
--setting-sources "" --strict-mcp-config --no-session-persistence
--system-prompt <the reviewer's>
```

Every job also gets the Guard's hook as `--settings` JSON. A controlled job's
settings add `"disableAllHooks": false` (*docs*: `--settings` takes precedence
over project and local settings, so a lower tier that switched hooks off does
not). *Observed*: an empty `--setting-sources ""` is accepted, and with it the
repository's `.claude/settings.json` `env` and permission rules did not apply
while the `--settings` hook still ran.

Even with no settings source, a run still reported plugins and MCP servers that
come from elsewhere (claude.ai connectors, bundled plugins): *observed*, 5
plugins and 7 MCP servers after `--setting-sources ""` against 13 and 9 with the
defaults. Whether `--strict-mcp-config` removes the connectors from that list is
*not verified*; the runtime record says what the run reported (below).

## Configuration

Every key of `[runtime]`, with its default. A value that is wrong is replaced
by the default and named by the status bar and `swarmux doctor`; a key that is
not known is named too, never silently ignored.

```toml
[runtime]
profile = "auto"                       # "auto", "inherit" or "controlled"
controlled_sources = ["user"]          # settings files a controlled job loads: "user", "project", "local"
reviewer_sources = []                  # the same for a reviewer (none)
canary = "each"                        # "each", "cached" or "off"
canary_cache_minutes = 60              # 1 to 1440, with "cached"
canary_timeout_secs = 90               # 10 to 600
on_error = "refuse"                    # "refuse" or "warn": a failed or impossible check
hook_failure = "auto"                  # "auto", "block" or "off": onFailure on the guard hook
statusline = "auto"                    # "auto", "on" or "off": swarmux's status line

[runtime.repos."/work/app"]            # folders whose own Claude Code settings may load
trusted = true

[runtime.mcp.docs]                     # a server a controlled job may use besides the Board's
command = "docs-server"                # a program, or url = "https://..." (transport "http" or "sse")
args = ["--stdio"]
env = { DOCS_TOKEN = "..." }
```

A server needs either `command` or `url`; one that has both, neither or the
wrong kind of value is left out and named.

## Versions

| | Version | How known |
| --- | --- | --- |
| Tested | 2.1.323 only | observed: the spike and the conformance run |
| Floor | 2.1.295, or 2.1.299 on Windows | docs: `onFailure: "block"` arrived in 2.1.295; before 2.1.299 a hook that set no shell failed open on Windows without Git for Windows |
| Latest seen | 2.1.325 | docs (changelog, 2026-11-13) |

The range is declared in `claude_cli/runtime/range.rs` and nowhere else.
Between the floor and the tested version nothing was run, and swarmux does not
say it works there; the canaries before each mission decide. Below the floor, or
when the version cannot be read, an unattended mission is refused. Outside the
tested range `swarmux doctor` warns.

## Before an unattended mission starts: the deny canary

When a mission's job is about to start, swarmux makes one tiny call to `claude`
(`--model haiku`, a scratch folder under the temp folder, a few turns) with the
same settings sources and MCP flags as the job. The Guard's hook is replaced by
`swarmux hook canary deny`, and an allow rule lets `touch <file>` through, so
the only thing that can stop the call is the hook. The canary passes when the
file was not made **and** the hook was asked (it logs each call it gets). If the
call ran, or the hook was never asked, the mission's job is refused (it fails
with `refused before starting: ...`, and the mission escalates as for any
failed job). A canary that cannot tell (the model made no call twice, a timeout,
no network) is treated as a failure.

The result is in the [ledger](ledger-and-evidence.md#what-a-run-was-launched-with-and-what-it-reported)
(kind `runtime`, event `canary`), and the job's `job_started` entry points at it
(`canary_seq`). The check also covers the version (above) and the host's policy
(next section); neither spends a call.

```toml
[runtime]
canary = "each"             # "each" (default), "cached" or "off"
canary_cache_minutes = 60   # with "cached"
canary_timeout_secs = 90
on_error = "refuse"         # "refuse" (default) or "warn": what a failed or impossible check does
```

`each` runs it before each mission start; jobs that start within 30 seconds of a
canary that passed share it (a task graph starts several at once). `cached`
reuses a result for `canary_cache_minutes` while the version, profile and host
policy stay as they were. `off` and `on_error = "warn"` are your choices and
are said in `swarmux doctor`; they let a mission start without the check, and
the ledger entry says so. The canary calls the model, so a mission cannot start
offline unless you chose one of those.

## `swarmux doctor --conformance`

```
swarmux doctor --conformance [--profile controlled|inherit]
```

runs the whole set against the installed Claude Code (five calls to `haiku`,
each a few turns) and prints a result per canary. It exits non-zero when a
required one does not pass. On 2.1.323 on the machine that wrote this page:

| Canary | Expected | Verified by |
| --- | --- | --- |
| `deny`: the hook denies a call an allow rule would let through | call stopped, the hook's reason comes back | observed, with a stand-in hook |
| `hook_exit_1`: the hook exits 1 with `onFailure: "block"` | call stopped | observed, with a stand-in hook |
| `hook_timeout`: the hook does not answer in time, `onFailure: "block"` | call stopped | observed, with a stand-in hook |
| `hook_invalid_json`: the hook prints text that is not JSON | **the call runs**, even with `onFailure: "block"` (a note, not a failure) | observed, with a stand-in hook |
| `subagent`: a subagent makes the call | the hook is asked, with `agent_id` and `agent_type` | observed, with a stand-in hook |
| `unknown_tool`: an unattended job's call to a tool no rule knows | asked or denied, never allowed or passed | by the Guard's own policy, no model |
| `guard_subagent`: a subagent's call is judged like the lead's | same verdict | by the Guard's own policy, no model |

"With a stand-in hook" means the spike drove `claude` with small Node scripts in
place of `swarmux hook canary`, because the machine that ran the spike has no
build of this branch. Running `swarmux doctor --conformance` itself against the
real Claude Code is for whoever has built it; it prints what it saw. The tests
(`tests/conformance.rs`, `crates/infrastructure/tests/canary.rs`) run the same
command against a fake `claude` that behaves as the real one was seen to, so
they show the harness, not Claude Code.

What the invalid-JSON note means: the Guard's hook either prints valid JSON or
exits with a code, so it never relies on Claude Code stopping a call after
garbage; it is a limit of Claude Code that the canary records.

## The host's policy

Claude Code's managed settings sit above everything swarmux passes with
`--settings`. `swarmux doctor` and the start of an unattended mission read the
places Claude Code documents (*docs*): `managed-settings.json` and
`managed-settings.d/*.json` and `managed-mcp.json` in `C:\Program Files\ClaudeCode`
(Windows), `/Library/Application Support/ClaudeCode` (macOS) or
`/etc/claude-code` (Linux), and on Windows the `HKLM` and `HKCU` policy keys
`SOFTWARE\Policies\ClaudeCode`. They report `allowManagedHooksOnly`,
`allowManagedPermissionRulesOnly`, `disableAllHooks`, `allowManagedMcpServersOnly`
and managed MCP servers, and **refuse an unattended mission** whose Guard hook
would be dropped:

- `disableAllHooks` in managed settings: *docs*, every hook, the Guard's too, is off.
- `allowManagedHooksOnly`: *docs* list only managed, SDK and force-enabled plugin
  hooks as still running. They do not mention `--settings` hooks, so swarmux
  assumes the Guard's is dropped. *Not verified*: that needs a managed file,
  which the spike did not write (it would change the whole machine's policy).
- `managed-mcp.json`: *docs* say `--mcp-config` and `--strict-mcp-config` then
  make Claude Code exit at startup, which the Board and the controlled profile
  pass. *Not verified.*
- A managed file that is not valid JSON: *docs* say Claude Code then refuses to
  start.

It does not read what Claude Code fetches from the server for Team and
Enterprise plans, nor the macOS managed-preferences domain, and says so
(`claude doctor` reports the server-managed source). A lock is reported when
any document it found sets it; Claude Code ranks its managed sources and uses
the first that has policy, so this can report a lock that is not in force. That
errs toward refusing. On the machine that wrote this page no managed file or
registry key exists, and `claude doctor` says the account has no organisation
policy.

## Fidelity

- `CLAUDE.local.md` is in the default [worktree](worktrees.md) `copy` list, next to
  `.env`, `.env.local` and `.claude/settings.local.json`, so the agent in a
  worktree has your personal instructions.
- swarmux's status line replaces yours only when you have none of your own
  (`[runtime] statusline = "auto"`). `"on"` always replaces it, `"off"` never.
  swarmux's status line is how it hears what the session itself reports, so with
  your own kept that feed is missing for those jobs.
- The Guard's hook carries `onFailure: "block"` when the version is not known to
  predate it (`hook_failure = "auto"`; `"block"` always, `"off"` never; the old
  `[guard] hook_on_failure_block = true` still means `"block"`).

## What was found out

The spike for this page ran on Claude Code 2.1.323 (Windows, logged in with a
subscription), about 16 small calls: `--model haiku`, one to four turns, in a
scratch folder under the temp folder, nowhere near a project. Documentation was
fetched from code.claude.com and treated as data.

| Question | Answer | How known |
| --- | --- | --- |
| What does `-p` in `default` mode do with a call no hook decides? | A call that needs approval is **denied** ("this session can't prompt"), and appears in `permission_denials`. A built-in read-only command (`echo`) needs none and runs. With an allow rule that matches, the call runs, so a hook that prints nothing, or fails open, leaves it to the user's allow rules. | observed (`touch` with and without `Bash(touch *)`) |
| A hook that denies | stops the call even with an allow rule, and its reason is returned to the model | observed |
| A hook that exits 1 | the call runs; with `onFailure: "block"` it is blocked | observed |
| A hook that does not answer in time | blocked with `onFailure: "block"` (the model then retried once, and was blocked again) | observed |
| A hook that prints invalid JSON and exits 0 | **the call runs, even with `onFailure: "block"`** | observed |
| Do hooks see a subagent's calls? | yes, with `agent_id` and `agent_type` in the hook input; the call that starts the subagent is the tool `Agent` | observed |
| Do `--settings` hooks survive `allowManagedHooksOnly`? | not tested; the docs do not list them among the hooks that survive | docs only, not verified |
| Is `--setting-sources ""` accepted? `user`? | both accepted. With `""` the repository's `.claude/settings.json` (`env`, permission rules) did not apply; with `user` its `CLAUDE.md` was not loaded. (`user` was not tried against a `settings.json`.) | observed |
| Does a per-job `CLAUDE_CONFIG_DIR` keep the login? | **no**: an empty one gives "Not logged in", as the docs say credentials follow the folder. It is not used. | observed, docs |
| Can a repository's `env` set `ANTHROPIC_BASE_URL` under `-p`? | **yes**: with the repository's `.claude/settings.json` pointing it at a closed local port the call retried and timed out; with `--setting-sources ""` the same folder ran normally. (No credentials left the machine: the port was closed.) | observed |
| Are a repository's `permissions.allow` rules honoured under `-p`? | not in an untrusted folder: they are ignored with a warning on stderr. Its hooks and `env` are (the docs: "even in a folder you've never trusted"). | observed (allow, env), docs (hooks) |
| `system/init` | has `claude_code_version`, `capabilities` (18 strings on 2.1.323), `tools`, `mcp_servers` (name, status, source), `plugins`, `skills`, `agents`, `permissionMode`; nothing about hooks, settings sources or managed policy | observed, docs |
| Managed settings on this machine | none (no file, no registry key) | observed |

## Known limits

- The canary tests one hook behaviour, in a scratch folder, with one tool. It
  shows that the hook mechanism works under this profile and version; it does
  not show that every call is seen.
- The Guard is still a settings-file hook. A hook that prints invalid JSON, or a
  Claude Code that drops `--settings` hooks, defeats it; the canary and the
  host policy check catch the second, and the first is why the hook never
  prints anything else.
- A controlled job has no repository `CLAUDE.md`. That is the price of not
  loading anything a repository can steer the agent with.
