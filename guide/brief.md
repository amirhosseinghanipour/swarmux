# The Return brief (`g b`)

The Return brief says what happened while you were away and what waits for
you now. It opens by itself when you attach after a gap, and any time with `g
b` (a chord: `g`, then `b`). It reads what swarmux already keeps: the same read
model as [Work](work.md) and the [Deck](deck.md) for what stands now, and the
[ledger](ledger-and-evidence.md) for what happened in the window. It shows
nothing it has no data for: a figure with nothing behind it says it is
unknown, never zero. There is no boot screen: the brief is the first screen
only when you were away.

## The window

From the moment you were last here (the newest ledger entry before this run)
to the moment you came back, with how long that was. The window is fixed when
the brief is first built, so it keeps saying what happened while you were
away however long you stay. Decisions and what is ready to land are read live.

On a first run there is no earlier session, so the brief says that and shows
only what waits now. When nothing happened and nothing waits it says so.

## What it shows

| Section | From |
| --- | --- |
| Counts: landed with proof, verified and ready to land, decisions waiting, stopped, spent | the console snapshot and the ledger |
| Claim to verified (median, with n), first-run pass rate (with the counts) | the ledger's gate verdicts in the window ([metrics](ledger-and-evidence.md)) |
| **Decisions waiting**, ranked as in Work, each with its auto-deny countdown or what waits on it | the snapshot |
| **Landed**: each with its evidence bundle (first 12 characters of its hash) and its change | the snapshot's landed group, in the window |
| **Verified, ready to land** | the snapshot |
| **Stopped**: escalated, refused, failed or interrupted in the window, with why, and whether a decision waits | the snapshot |
| **Guard while away**: routine allows as one number; held and refused calls with their rule or reason; calls that fell back because swarmux was not reachable (`guard_fallback`); always-allow rules added | the ledger's `guard_decision` and `policy` entries |
| **Spend and checks**: what the jobs that ended in the window cost, today across all sessions, hidden checks that did not run (with the reason), jobs whose checks ran in their own worktree | the ledger's `job_finished` and gate entries, the usage telemetry |
| **What lands where**: by repository, from landing data | the snapshot's landings |
| **Timeline**: a short list of what happened, oldest first | the ledger |

What it does not show, because nothing backs it:

- **Minutes of your time.** swarmux does not record how long a decision takes,
  so there is no estimate.
- **A day budget.** There is no day budget setting; the line says `day budget:
  not tracked`. Today's spend across your sessions is shown when swarmux could
  read it.
- **Dispatch held by a WIP limit** and **sampled items waiting.** These belong to
  review back pressure, which is not built yet; the brief has a place for them
  and draws nothing until it does.

The ledger's event for an added always-allow rule is `added` (the plan called
it `guard_rule_added`).

## Keys

| Key | Does |
| --- | --- |
| `Enter` | start with the first decision: opens it in Work. `Esc` there comes back here |
| `Esc` | skip past the brief, to Work by default or the Deck (`[brief] esc_goes_to`) |
| `w`, `d` | skip to Work, skip to the Deck |
| `Tab` | at a small size, the sections that fold |
| `g b` | the brief, from anywhere |

The keys are actions of the `brief` context (`brief.to_work`, `brief.to_deck`,
`brief.more`, `list.open`) and rebindable under `[keys.brief]`, like every
other key: see [configuration](configuration.md).

## Small windows

At 180×50 every section is on screen. Below 140 columns or 40 rows the
sections fold, in this order of priority (kept first): decisions, landed and
ready, stopped, the Guard, spend and checks. The timeline and what lands where
are behind `Tab`. Below 80×16 it asks for a bigger window.

## Customizing {#customizing-brief}

```toml
[brief]
away_after_minutes = 30    # how long away counts, 1 to 10080; default 30
show_on_attach = true      # open on the brief after that gap; default true
esc_goes_to = "work"       # where Esc skips to: "work" (default) or "deck"
```

A mistake never stops swarmux from starting: a value that cannot be used is
ignored and the default applies, and one line at the bottom says so.
`swarmux return-brief lint` prints every problem and exits with 1 if there is
one. (`swarmux brief` is the older text brief of the last hours.)

## Try it

`swarmux --demo --demo-away 60` plays the scripted night first, then attaches
as if you had been away for an hour, so the brief reads the demo's ledger and
snapshot with the same code a real run uses.

## For clients

The brief is also plain data: `BriefView` in `swarmux_application::console`
(schema 1, the same rules as the console snapshot). Adding a field does not
change the schema; the golden files `brief.json` and `brief_first_run.json`
fail until a change to its shape is made on purpose.
