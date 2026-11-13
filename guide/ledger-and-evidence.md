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
the review, how it finished) and every guard, loop-breaker, JEV and routing
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
itself. The strong guarantee comes from a copy outside the machine's reach:
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

## Changes to the policy and gaps in it

Entries of kind `policy` record what changes what the [Guard](guard.md)
allows, and where it could not decide:

- `added`, `removed` and `expired`: an "always allow" or an accepted rule offer
  was added, removed by `swarmux guard rules remove`, or pruned after its expiry
  (`rule`, `scope`, `root`, `source`, `created_at`, `expires_at` and the SHA-256
  of the rule as kept; the actor is `operator`).
- `guard_fallback`: a hook call that fell back because swarmux could not be
  reached (when, job, tool, the SHA-256 of the hook input, why, what the hook
  printed, and the hash of the setting that decided it; the actor is
  `guard-hook`). These reach the ledger when swarmux next starts, from a spool
  file the hook writes, so a fallback is on record only after that restart.

Guard decisions (kind `guard_decision`) also carry the SHA-256 of the rule that
decided them, who resolved a held call (`human-typed`, `human-key`, `rule`,
`model`, `timeout`) and, for a subagent's call, its id and type; JEV's carry the
provider, the model, the hash of what was sent and whether the summary was cut.
