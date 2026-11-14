# What leaves your machine

swarmux sends no usage data, telemetry or crash reports. It opens a network
connection itself in the cases below, and everything else that leaves is
traffic of programs it starts. This page lists each one, what it sends, and
whether it is on by default. It describes the code as it is; if you find an
outbound call that is not here, that is a bug in this page, and
[SECURITY.md](../SECURITY.md) says how to report it.

## What swarmux sends itself

| What | When | What is sent | Default |
| --- | --- | --- | --- |
| `swarmux self-update` | only when you run it (`--check` asks without installing) | an HTTPS request to the GitHub API and release downloads of the swarmux repository, with a `swarmux/<version>` user agent; the release is installed only when its minisign signature verifies against the key built into the binary (`crates/infrastructure/src/dist/source.rs`) | never runs by itself |
| ntfy notifications | when `[notify] ntfy` is set to a topic URL | one HTTPS POST per notification to that URL: a fixed title (`swarmux P2`), a priority, a tag and the kind of item (`an approval is waiting`); never a command, path, prompt or session name (`crates/infrastructure/src/notify.rs`, [Notifications](notifications.md)) | off |

That is all swarmux's own HTTP client does: the code has one client for
releases and one for ntfy. The ntfy server, and anyone who can read your topic,
sees those messages; use a topic name nobody can guess.

## What programs swarmux starts send

**`gh` (the GitHub CLI), when it is installed.** swarmux runs `gh` and `gh`
talks to GitHub with your login; what it sends is `gh`'s own.

- *Pull requests and CI in the telemetry.* `gh repo view`, `gh pr list` and
  `gh run list`, in the selected session's folder, every `github_secs` (60 by
  default). This is **on by default** (`[telemetry] github = true`), and does
  nothing when `gh` is missing or not logged in. Turn it off with
  `github = false` ([Telemetry](telemetry.md)).
- *Outcome labels.* Asking what became of a delivered branch (its pull
  request, a revert) runs only when you press `l` on the Ledger screen or run
  `swarmux labels` ([Ledger](ledger-and-evidence.md)).

**Claude Code.** swarmux starts `claude` for your jobs and for its helpers.
Claude Code sends what it is given to Anthropic, under your own Claude Code
login or API key; swarmux holds no key of its own and does not see Claude
Code's other traffic. The calls swarmux starts:

- your jobs, mission jobs and their continuations (send back, resume, retry);
- the review step, when it is on ([Verifier](verifier.md#review-step));
- small Haiku calls: routing a prompt, diagnosing a stuck agent, reading what
  a finished job left, and linting and planning missions
  ([Sessions](sessions.md), [Watchdog](watchdog.md),
  [Follow-through](follow-through.md), [Missions](missions.md)).

The text each of these is given (a prompt, a diff, an excerpt of a transcript)
goes to Anthropic as part of that Claude Code run. No hosted model decides your
tool calls: the Guard is code and your rules ([Guard](guard.md)).

**Your checks and your agents' code.** The gate runs the commands in your
`gates.toml` (or the ones it guesses, such as `cargo test`) as child processes,
with a reduced environment ([Verifier](verifier.md#what-a-check-can-see)).
Those commands, and the code the agent wrote, can use the network as any
program run as you can; swarmux does not restrict it. A tool call an agent
makes (`WebFetch`, `curl`) is a Claude Code call, held or allowed by your
Guard rules.

## What stays local

The hook between Claude Code and swarmux, the background service and its
clients use a local socket or named pipe, not a network port. Claude Code's
transcripts, git history, the telemetry readings, the ledger, the evidence
bundles and the hidden checks are read and written on your machine. `git` is
used for local operations (worktrees, copies, diffs, merges); swarmux does not
run `git push`, `fetch` or `clone` itself. `swarmux --demo` starts no
`claude` and reads none of your data ([Install](install.md#the-scripted-demo)).
