# Ledger (screen 5) and evidence bundles

A mission is delivered only with a bundle. When the gate passes, swarmux
seals `.swarmux/evidence/<mission>.json` (and keeps the same JSON in its
database): the contract, the commit the work ended at (`head_sha`, with the
branch point `base_sha`), the diff's size and hash (and, in bundles sealed
since per-file counts were kept, the files behind them with the lines git
counted in each: `diff.changes`, written only when there are some, so a
bundle sealed earlier keeps its digest and shows its totals alone), the checks the passing
verdict ran (command, exit code, time, the first failing line on a failure),
every gate verdict with the digest of its ledger entry, the checkpoints, the
reviewer's verdict, the cost by phase and by model, and the stretch of the
ledger that tells the story with the hash it ended at. The bundle's digest is
the SHA-256 of its JSON without the signature, and the sealing is itself an
entry in the audit ledger. When sealing, swarmux also signs a checkpoint of
the ledger (see [Anchoring](#anchoring-the-ledger)) and keeps it in the
bundle; `swarmux verify` checks it. The signature is only that hash for now: it shows
a bundle was edited, it does not say who wrote it. A mission that cannot be
sealed (no passing verdict on the ledger, no git repository) is escalated
instead of delivered, and missions delivered before bundles existed get one
the next time swarmux starts.

### What a bundle says about how it was checked (schema 2)

A tick says the gate passed; it does not say how. A bundle of schema 2 has a
`verification` object, read from the passing verdict's ledger entry, so it
never shows more certainty than the run had:

| Key | Says |
| --- | --- |
| `ran_in` | `clean_copy`, or `worktree` when the checks ran next to what the agent left (a mission is only delivered like that when its `gates.toml` says `worktree_checks = "allow"`) |
| `planned`, `left_out` | the checks the gate meant to run, and those it left out with why (a level above the gate's) |
| `hidden_not_run` | how many hidden checks exist and did not run, and why: `hidden checks: not run (reason)`. The `hidden` count beside it is only for checks that did run |
| `base_suite` | whether the original tests passed with the job's code, and which files were put back |
| `tampering`, `substrate`, `dependencies` | tests weakened, harness files created or changed, dependencies added or changed: what the gate found and let by |
| `revert_probe` | whether the tests proved the change, when the probe ran |
| `incomplete` | why the gate could not vouch, if it could not |
| `policy_sha256` | the hash of the gate configuration that judged |

`swarmux verify` prints what it says as findings that do not mismatch (`checks
ran in: the worktree, next to what the agent left`, `hidden checks: not run
(...)`, `planned check <name>: not run`): they are what the bundle does not
establish. A bundle sealed before schema 2 prints `how it was checked: not
recorded`.

**Schema versions.** The bundle format is `schema` 2. Bundles of schema 1 are
still read and verify as they always did: their digest covers the bytes they
were sealed with, no new key is written into them when they are read back,
and nothing is added to the digest of a bundle that already has one. Anything
other than 1 or 2 is refused with its number. A fixture in the repository
(`crates/domain/src/evidence/fixtures/bundle_v1.json`) pins the version 1
bytes and its signature, which was also recomputed with `sha256sum`.

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
checks passed and the bundle's digest, the models paid, the cache-read
share, the label). Focus shows the same share in its header. It is the part
of the job's input that came from the prompt cache, `—` when the job did not
report it. `swarmux stats cache [--days N]` totals it per model. The
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


## Audit ledger

Everything swarmux stores about a job (queued, started, each agent event,
the review, how it finished) and every guard, loop-breaker and routing
decision is also appended to a ledger in `~/.swarmux/swarmux.db`. Each entry
holds the SHA-256 of the previous entry plus its own content, so editing,
removing or reordering anything already recorded breaks the chain. Only
swarmux writes it; the agent runner never gets a handle on it, and the table
refuses changes and deletions. The chain alone has no secret key: someone who
can write the end of it can rewrite every entry after an edit and get a valid
chain with a different head. That is what anchoring is for.

```sh
swarmux audit verify      # walks the chain and every checkpoint; exits 1 on a break
swarmux audit tail -n 20  # the newest entries
swarmux doctor            # includes the ledger head and whether it verifies
```

### Anchoring the ledger

A checkpoint is a signed statement: "at this time the ledger had this many
entries and its last entry hashed to this". It is signed with an ed25519 key
(minisign format) that lives outside the database, in the OS keychain when
there is one.

```sh
swarmux audit checkpoint                   # sign the head now (also done whenever a mission is delivered)
swarmux audit verify                       # the chain, and the ledger against every checkpoint
swarmux audit export-checkpoints > cp.txt  # one line each: keep a copy where an agent cannot write
swarmux audit verify --checkpoints cp.txt  # also check against that copy
```

`verify` reports **cut short** when the ledger is shorter than a checkpoint
says it was, and **rewritten** when the entry at a checkpoint's position
hashes differently now, even if the rewritten chain is internally valid.
Entries after the newest checkpoint are not anchored yet; `verify` says how
many.

What this protects, honestly: the ledger is *tamper-evident, anchored*, not
tamper-proof. Agents run as your own OS user, so anything that can write the
database can in principle also ask the keychain for the key (or read it from
`~/.swarmux/audit.key` on a machine without a keychain, which `audit
checkpoint` says), delete the checkpoints and sign new ones. Checkpoints
raise the bar from "edit a file" to "edit a file and use the key" and make
the earlier history committed to something the database does not hold by
itself. A stronger protection comes from a copy outside the machine's reach:
export the checkpoints now and then (a gist, an email to yourself) and verify
against them. On Linux the keychain may not survive a reboot; then a new key
is made, and earlier checkpoints still verify against their own recorded key
but show as made by another key.

Long texts (prompts, answers, tool output) are kept up to 8 KB per field,
with the SHA-256 and length of the full text.


## What you told an agent

When you [send work back with a note](work.md#send-back), or deny a held call
with one, the note is recorded as an entry of kind `operator` (the actor is
`operator`): the note, the job, where it stood (`escalated`, `verifying`,
`failed`, `held`, ...), the job the agent carries on as, and for a call the
call itself. It is chained like every other entry, so verifying the ledger
covers it, and a mission's bundle range includes it when it falls inside.

A [message to a running agent](work.md#message-a-running-agent) is an
`operator` entry too, with `action` `intervene`. It is written before anything
is sent, and a message that cannot be written is not sent. The first entry
(`outcome` `queued`, or `stopping` for a steer) carries the `mode`, the text
(cut at 8 KiB like a note, with its SHA-256 and length when cut), the `job`, the
`mission` if there is one, and the message's number in the run (`message`). A
second entry says how it ended: `delivered` (with `resumed_as`, the job that
carries it, and `delivered_at`, when the agent's turn with it was seen to start)
or `undelivered` (with the `reason`, and `queued_seq` pointing at the first
entry). A message refused before it was taken (empty, too long, the job not
running) is not recorded: nothing was sent.

## What a run was launched with, and what it reported

The `job_started` entry carries a `runtime` object when swarmux launched the
job through an agent adapter (Claude Code): `runtime` (`claude-code`),
`profile` (`inherit` or `controlled`) and `profile_reason`, `version` as
found at start and `version_standing` (`tested`, `above_tested`,
`below_tested`, `below_floor`, `unknown`), `sources` (the settings files it
loads: `default`, a list, or `none`), `strict_mcp`, `on_failure_block`,
`settings_hash` (SHA-256 of the `--settings` JSON swarmux generated),
`host_policy` (`sources`, `locks`, `unreadable` and `blocks` of Claude Code's
managed settings as read), `policy_sha256` (the `[runtime]` settings that
decided it) and `canary_seq`, the entry of the canary that
cleared it (null for a job that needed none). An entry written before this
existed has no `runtime`, and verifies as before.

Entries of kind `runtime` are written next to it:

- `canary`: the checks run before an unattended mission's job started
  (`version`, `host_policy`, `deny`), each with its `status` (`pass`, `fail`,
  `note`, `inconclusive`, `skipped`), whether it was `required` and what was
  seen, `passed`, and `policy_sha256`, the `[runtime]` settings that decided
  what to do with it. A refused job has one of these and no `job_started`.
- `observed`: what the agent said about itself in `system/init` once it was
  running: `version`, `permission_mode`, the number of `tools` and `plugins`,
  the `mcp_servers` as `name=status`, `capabilities`, and `mismatches`, the places
  where that differs from what was asked (another version than the one found at
  start; an MCP server in a controlled run that is not swarmux's).

## Changes to the policy and gaps in it

Entries of kind `policy` record what changes what the [Guard](guard.md)
allows, and where it could not decide:

- `added`, `removed` and `expired`: an "always allow" or an accepted rule offer
  was added, removed by `swarmux guard rules remove`, or pruned after its expiry
  (`rule`, `scope`, `root`, `source`, `created_at`, `expires_at` and the SHA-256
  of the rule as kept; the actor is `operator`).
- `gate_selftest`: `swarmux gate selftest` ran. Each known-bad change it made
  in a scratch copy and whether the gate refused it (`probes`), the probes it
  could not make (`skipped`), the outcome (`pass` only when at least one was
  made and all were refused), the repository, and the SHA-256 of the gate
  configuration that judged (`policy_sha256`; the actor is `operator`).
- `holdout_migrated`: hidden checks kept under a repository's folder name moved
  to its identity key (`from`, `to`; the actor is `operator`). See [the
  verifier](verifier.md#how-hidden-checks-are-kept-per-repository).
- `guard_fallback`: a hook call that fell back because swarmux could not be
  reached (when, job, tool, the SHA-256 of the hook input, why, what the hook
  printed, and the hash of the setting that decided it; the actor is
  `guard-hook`). These reach the ledger when swarmux next starts, from a spool
  file the hook writes, so a fallback is on record only after that restart.

Guard decisions (kind `guard_decision`) also carry the SHA-256 of the rule that
decided them, who resolved a held call (`human-typed`, `human-key`, `rule`,
`timeout`) and, for a subagent's call, its id and type. Entries written while
the optional JEV model existed (source `jev`, resolver `model`, with the
provider, model and hash of what was sent) stay in the ledger unchanged and
verify as before; swarmux no longer writes any.

## What a gate verdict records

An entry of kind `gate_verdict` is the gate's decision, and carries what it
decided on: the checks (command, exit code, time, whether it flaked, the first
failing line), the protected files that changed, the claims nothing backed,
the tests weakened, and, since the verifier checks the harness:

- `outcome`: `pass`, `block`, `escalate` for a stop; for a run nobody waited
  for (a mission's verification, `swarmux gate run`) `pass`, `fail`, or
  `escalate` when nothing was wrong with the change and the gate could not
  vouch for it. It is never `pass` when the gate could not read its file,
  hidden checks that exist did not run in a mission, or the checks ran in the
  worktree where a mission does not allow that.
- `substrate`, `dependencies`, `ran_in`, `planned`, `left_out`,
  `hidden_not_run`, `base_suite`, `revert_probe`, `incomplete`, `warnings`,
  `unattended`: what [the bundle](#what-a-bundle-says-about-how-it-was-checked-schema-2)
  repeats.
- `policy_sha256`: the SHA-256 of the gate configuration that judged (the
  checks with their levels, `required`, environment names and timeouts, the
  limits, and every `[verifier]` key). Change anything that decides a verdict
  and the hash changes, so an entry says which rules judged it.

Hidden checks are counted, never named, here as everywhere. Entries written
before these keys existed read as having none of them.
