# Ledger (screen 5) and evidence bundles

A mission is delivered only with a bundle. When the gate passes, swarmux
seals `.swarmux/evidence/<mission>.json` (and keeps the same JSON in its
database): the contract, the commit the work ended at (`head_sha`, with the
branch point `base_sha`), the diff's size and hash, the checks the passing
verdict ran (command, exit code, time, the first failing line on a failure),
every gate verdict with the digest of its ledger entry, the checkpoints, the
reviewer's verdict, the cost by phase and by model, and the stretch of the
ledger that tells the story with the hash it ended at. The bundle's digest is
the SHA-256 of its JSON without the signature, and the sealing is itself an
entry in the audit ledger. The signature is only that hash for now: it shows
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
checks passed and the bundle's digest, the models paid, the label). The
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

