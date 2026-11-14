# Security model

What swarmux defends, what it does not, and what happens when something
fails. It describes what the code does; where a guarantee has a hole, the hole
is listed. To report a vulnerability, see [SECURITY.md](SECURITY.md).

## What swarmux is, and is not

swarmux is a control plane that sits beside Claude Code on your machine. It
**is not a security boundary.**

- **Agents run as your operating-system user.** An agent can do whatever you
  can do, by any route swarmux does not see. swarmux watches the tool calls
  Claude Code reports to its hook, and the work an agent leaves behind. It does
  not sandbox the processes the agent starts, and it cannot stop an agent that
  works outside the hook (a program the agent builds and runs, a script that
  does a thing the hook only sees as "Bash").
- **The Guard matches text.** A rule such as `Bash(rm -rf *)` is matched
  against the command string after shell parsing. It catches what it matches
  and nothing else. Obfuscation, a script the agent wrote a minute ago, a
  tool Claude Code does not report: none of that is the Guard's to see.
- **The ledger is tamper-*evident*, not tamper-proof.** Every entry commits to
  the one before. Changing or removing a stored entry breaks the chain and
  `swarmux audit verify` says where. Someone who can write the database *and*
  the signing key can rewrite history and re-sign it; the key is kept in the
  OS keychain when there is one and in a file readable by your user when not.
  Checkpoints make a rewrite visible to anyone who kept an earlier one.
- **The organisation's file is a client-side control.** `managed.toml` can lock
  keys against every other layer of the configuration, including you at the
  command line. The file and swarmux's binary are both yours to edit if you are the
  user. It stops mistakes and repository files, not a person with the rights to
  remove it.
- **Repository trust is not a sandbox.** Granting trust lets swarmux run the
  repository's check commands, and a test command runs the repository's code.

What it *does* give you: policy on the calls the hook sees, an independent
re-run of checks the agent cannot edit, a record you can check, and a refusal
to proceed when it cannot vouch for something.

## Trust tiers

Who may say what, from least to most trusted. A higher layer wins for ordinary
keys; keys that *grant* need the owner; keys that *restrict* are honoured from
any layer at once. See [Configuration](guide/configuration.md#layers).

| Tier | Source | May grant | May restrict |
| --- | --- | --- | --- |
| core | compiled in, including the invariants below | n/a | yes, and the invariants cannot be configured away |
| org | `managed.toml` (keys it did not lock) | yes | yes |
| user | your `config.toml` | yes | yes |
| workspace | your workspace file | yes | yes |
| project | the repository's `.swarmux/*.toml` | **only with your trust record** (per repository and file hash) | yes, at once |
| pack | an installed pack (placeholder: none exist yet) | no | yes |
| mission | a mission's settings | no | yes |
| run | command-line flags; `SWARMUX_*` variables | flags: what you type; variables: only narrowing, never a granting key | yes |
| managed lock | `managed.toml` `locked = [...]` | yes | yes; wins over every tier above |

What counts as *granting*: allow rules, commands that run (`[[check]]`), changes
the verifier would otherwise block, endpoints and notification targets
(`notify.ntfy`), standing orders. What counts as *restricting*: deny rules,
protected paths, stricter limits and locks.

## What happens when something fails

The rule everywhere: **unattended work fails closed.** A mission or a
background job nobody watches never turns a failure into a pass.

| What fails | What happens | Recorded |
| --- | --- | --- |
| The hook cannot reach swarmux (app closed, crashed, pipe gone) | `guard.on_unreachable = "auto"` (default): unattended jobs are **denied**, other jobs are left to Claude Code's own rules. `deny`, `ask` and `pass` force one answer for all jobs. | each fallback is spooled by the hook and written to the ledger as `guard_fallback` at the next start |
| A tool call no rule matches | `guard.unmatched = "ask"` (default): you are asked; with `"pass"` Claude Code's own rules decide | decision entry |
| An irreversible call (R4) | held; only a human typing the target's name allows it. No rule, timer, config key or automated resolver can | decision entry with the resolver |
| An approval nobody answers | denied after `guard.approval_timeout_secs` | decision entry (`timeout`) |
| A write to a protected path, to swarmux's own state or config, or to files that steer agents | refused (unattended jobs: denied, others: asked for the steering files) | decision entry |
| An agent runs a `swarmux` subcommand other than `hook`, `board-mcp`, `statusline` | denied | decision entry |
| `.swarmux/gates.toml` cannot be read or has an unknown key | the gate **escalates** with the reason; it never passes | gate verdict |
| The repository is not trusted | its checks do not run; the gate escalates for work it cannot vouch for; what only restricts still applies | gate verdict (`problem`), inbox item |
| Hidden checks exist and could not run | a mission's verdict is **escalate**: "hidden checks: not run (reason)" | gate verdict, bundle |
| The checks could not run in a clean copy | they run in the worktree, the bundle says `ran_in = "worktree"` and a mission **escalates** (`verifier.worktree_checks = "allow"` opts out) | gate verdict, bundle |
| A check cannot start or times out | the check fails; `on_error` has one value, `fail` | gate verdict |
| A line of any configuration file is wrong | that line is dropped, the key falls back to its default, or for a restrictive key to its restrictive reading; the app starts | warning in the app, `doctor`, `config lint` |
| The organisation's file has an allowlist that cannot be read | enforced as empty | `config lint` |
| `swarmux self-update` cannot verify a signature | it installs nothing | the message |

Holes, said plainly: the Guard fails *open* for **attended** jobs when swarmux
is unreachable (by default; `guard.on_unreachable = "deny"` closes it); and a decision in
flight when the process is killed can be missing from the ledger.

## The integrity invariants

These are the guarantees the other features rest on. Each is enforced in code and
covered by a test, and none is a setting.

1. **Only a human's typed confirmation allows an irreversible call.** Enforced in
   `GuardService::resolve` through `may_resolve`, not in the interface; no
   model, rule, timer or config key resolves such a call to allow. A domain-level
   test proves no automated path can.
2. **The ledger is append-only and hash-chained.** The adapter only inserts; the
   schema refuses updates and deletes of stored rows; each entry commits to its
   predecessor; the hash format is frozen. Removing or editing an entry is
   detected, not prevented.
3. **Hidden checks run in a restored copy.** They are kept outside the
   repository and out of the agents' tools, run on a copy made again from what the
   agent left with the test harness and changed tests put back from the commit the
   job started from, and are shown as counts only. When this cannot be done the
   verdict says so and a mission escalates.
4. **Unattended jobs fail closed.** See the table above: denied when swarmux is
   unreachable, no write to protected or steering files, no guessed commands
   before trust, escalate when the gate cannot vouch.
5. **Trust is granted only by you, in the ledger.** `trust_granted` entries come
   from `swarmux trust grant`, which the Guard denies to agents; a repository file
   cannot grant itself anything, and a changed file asks again.
6. **Every decision names the policy that made it.** Guard decision entries carry
   `policy_sha256` (the hash of the rules in force) and, when one rule decided,
   its tier and file (`rule_tier`, `rule_source`); gate verdicts carry
   `policy_sha256`, `policy_tier` and `policy_source`; every mission start writes
   `config_loaded` with the hash of the effective configuration and its layers.

## What leaves your machine

See [SECURITY.md](SECURITY.md#what-leaves-your-machine). Configuration cannot add an endpoint from a repository: `notify.ntfy`
is a granting key and is read from your own files only.
