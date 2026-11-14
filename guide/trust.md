# Repository trust

A repository can tell swarmux what to run: `.swarmux/gates.toml` lists checks
(`[[check]] run = ["cargo", "test"]`), and when it has no such file swarmux
guesses `npm test`, `pytest`, `cargo test` or `go test` from its files. Those
commands come from whoever wrote the repository, not from you. A repository you
just cloned can ask for anything.

**A repository runs none of the commands it ships until you grant it trust.**

## The rule

- **Nothing a repository ships runs before a grant.** Not its `[[check]]`
  commands, and not guessed ones when nobody is watching (a mission or a
  background job). Guessed commands still run when you are watching a session,
  as they always did, but the repository still asks to be trusted.
- **A grant is per repository and per set of files.** It records the
  repository's identity (a hash of its remote and root commit, so a second clone
  is the same repository and a different repository in a folder of the same name
  is not) and the hash of every file the grant covers (`.swarmux/*.toml`, and for
  guessed commands the command and, for `npm test`, the `scripts.test` line it
  runs). A file that changes asks again: there is nothing to do about a repository
  whose files an agent or a `git pull` just edited except look at it again.
- **A grant is a ledger entry** (`trust_granted`, with the repository and every
  file hash), so who trusted what, and when, is part of the tamper-evident record.
  `swarmux trust revoke` writes `trust_revoked`; the newest entry of a repository
  wins.
- **What only restricts never waits.** `gate.protected`, `verifier.substrate`,
  `verifier.revert_probe`, `verifier.dependencies = "block"` and
  `guard.protected_paths` in `.swarmux/config.toml` apply the moment swarmux sees
  the repository. A repository can add to its fence, not lower it. Until it is
  trusted, the keys that loosen the gate (`allow_test_changes`, `allow_substrate`,
  `env`, `test_edits = "allow"`, `clean = false`, `base_suite = false`,
  `worktree_checks = "allow"`, `evidence = false`, `max_blocks = 0`, a lower
  `level`) read as their defaults.
- **Until trusted the gate escalates.** With no checks run it cannot vouch for the
  work, so a mission's verdict is *escalate* with the reason ("not trusted yet: its
  checks did not run"), never *pass*.

The simplest safe rule for your own repositories: run
`swarmux trust grant <folder>` once. There is no implicit trust for "a repository
I started a mission in", because that is exactly what a freshly cloned
repository also looks like.

## What you see

The first time swarmux works in a repository that wants to run something, one
item appears in the inbox on the [Work screen](work.md) (and in
`swarmux doctor` for the current folder). It is not blocking: nothing stops, and
jobs go on. The item shows the exact commands, one per line, and the command to
grant:

```
shop wants to run commands: trust it?
  run cargo test --workspace  (gates.toml: unit)
  allow harness changes in Makefile
until you grant it, these do not run and the gate escalates. Its protected paths and denies apply now.
swarmux trust grant /home/me/code/shop
```

If a trusted repository's files change, the same item comes back, naming the
files that differ.

**Existing repositories that already have a `.swarmux/gates.toml` get this one
prompt after you update.** Until you grant them their checks do not run, which is
a change from before. Run `swarmux trust grant <folder>` for each repository you
use.

## Commands

```sh
swarmux trust list               # repositories you granted or revoked, and whether their files still match
swarmux trust show [folder]      # what a repository wants to run and the hash of each file a grant would cover
swarmux trust grant <folder>     # allow exactly what `show` lists
swarmux trust revoke <folder|id> # take it back
```

`swarmux gate`, `swarmux gate run` and `swarmux gate selftest` follow the same
rule, so typing a command in a cloned repository does not run its checks either
(`swarmux gate lint` only reads the file). An agent cannot grant trust: the
Guard denies an agent running `swarmux trust`.

## What a grant is not

A grant lets swarmux run the repository's check commands, and a test command
runs the repository's code: its tests, its `build.rs`, its `package.json` scripts.
Trusting a repository means trusting that code to run on your machine, with your
user's rights. It is a decision about the repository, not a sandbox
around it. See [SECURITY-MODEL.md](../SECURITY-MODEL.md).

## Project settings

`.swarmux/config.toml` is the repository's layer of the
[configuration](configuration.md#layers). Today only `guard.protected_paths` is
read from it (a restriction, honoured at once); keys that grant are refused there
with a reason, and the other settings are yours alone.
