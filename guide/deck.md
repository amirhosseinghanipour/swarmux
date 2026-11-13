# The Deck (`g d`)

The Deck is the picture of the work: a task graph in the middle, and around it
what needs you, how verification is going, what the limits allow, what the
Guard held, and what happened lately. Go to it with `g d` (a chord: `g`, then
`d`). It reads what swarmux already keeps (the same read model as
[Work](work.md)) and shows nothing it has no data for: a figure with nothing
behind it says it is unknown, never zero.

## The task graph

Every job, and every task of a plan built as a task graph, is a card.

```text
┌ api-gateway ─────────── Sonnet ┐
│ □  #7 VERIFYING        $9.14   │   state shape, number, state, spend
│   ■ ■ · ■ ■ · · ·  3/9         │   one dot per configured check, passed of total
│   Invoice totals round half up │   title
└────────────────────────────────┘
```

- **State shape**, readable without colour: `□` working (waiting, queued,
  running, being verified), `■` verified or done, `▌▐` a tool call is held
  for you, `▚` blocked or escalated.
- **Dots**: one for each check the gate runs for the job: `■` passed, `×`
  failed, `·` not yet. A job with no configured checks says
  `no checks configured`; swarmux does not invent a dot.
- **Lines** are drawn only from recorded relationships, left to right from
  producer to consumer, ending in `▶` at the card that waits:

  | Line | Meaning | Colour |
  | --- | --- | --- |
  | `──` | a plan task depends on another | green met, label-coloured pending, red blocked |
  | `┈┈` | a repository provides what another uses (a contract) | same |
  | `╌╌` | two cards claim overlapping files; the one holding them is at the left | same |

  The lines of the selected card are bold, and a line under it in words lists
  all its relations, including the ones the picture leaves out (a cycle that was
  broken, two cards that could not be put in order).
- Cards with no relation to another are on a **shelf** headed `NO
  RELATIONSHIPS`, beside the columns when there is room, else below them.

**Keys** (rebind any of them under `[keys.deck]`, see [Keys](configuration.md#keys)):

| Keys | Action id | What it does |
| --- | --- | --- |
| `j` `k` / `↓` `↑` | `list.next`, `list.prev` | the next or previous card (column by column, top to bottom), or the session row while the strip has the keys |
| `h` `l` / `←` `→` | `deck.producer`, `deck.consumer` | go to a card this one waits for, or one that waits for it |
| `Enter` | `list.open` | open the card's outcome in [Work](work.md), with the Deck behind it: `Esc` comes back. An outcome on no queue (nothing to decide) opens its job in Focus |
| `s` | `deck.sessions` | give the keys to the sessions strip, or back to the graph |
| `Tab` | `deck.more` | at a small size, bring the next folded panel in place of the graph |

### Size

At 140×40 and over, the graph and all panels show. Under that the Deck folds:
the graph stays, with the needs, the limits and the Guard; the sessions strip,
verification and the timeline take the graph's place one at a time with `Tab`
(`s` brings the sessions). The graph is a list, two rows a card (the card, then
its title and relations), when its columns do not fit across the pane
(each column needs 26 cells and the gaps for the lines) or, in a pane under 100
cells wide, when it would not fit down it. At any size the selected card is in
view: the graph scrolls (`▲` `▼` at its edge, and `n more out of view`).

Below 80×16 the Deck asks for a bigger window.

### How the layout is decided

Columns are the producer-to-consumer depth the read model gives each card
(file claims have none: the card that holds the files goes left of the one
waiting for them, unless that would close a cycle). A line over a column passes
through a row in it. Order within a column is improved by sweeps that remove
line crossings, and rows follow the neighbours' rows. It is the same every
time. A cycle is broken, its closing edge not drawn and said in words;
more edges in a gap than it has lanes share the last lane (the picture may then
join lines that are not joined); with a hundred cards the columns scroll and
the isolated ones wrap on the shelf.

## The panels

| Panel | Shows |
| --- | --- |
| `sessions` | one row per session: its state, repository, model, context, spend, age and the last line of its stream. `Enter` opens the job |
| `needs` | the decisions waiting for you (count and the top ones), each with why (`auto-deny in 8:19`, `blocks t3 (task dependency)`), and the way to Work |
| `verification` | jobs verified, first-run pass (always with how many jobs it is of, and `small n` under five), claim to verified (median, p90, n), checks run, bundles sealed, open escalations. The window is `metrics_hours` of [`[console]`](configuration.md#console) |
| `limits` | the 5-hour and 7-day windows with the forecast line, spend today and this week, spend this run, and each mission's spend against its own budget |
| `guard` | calls held for you or refused, with the rule that decided each; the routine allows are one number (of the last 50 decisions of this run), not rows |
| `timeline` | the change log: what started, finished, was asked, denied, gated |
| `graph` | the task graph above |

Panels with nothing to show say so in a line: `no jobs yet`, `nothing waits
for you`, `no call is held or refused`.

## Customizing: `[deck]`

```toml
[deck]
# Which panels show, and in which order. These are all of them.
panels = ["graph", "sessions", "verification", "limits", "guard", "timeline", "needs"]
# Which way the graph runs: "lr" (producers at the left). The only direction yet.
direction = "lr"
# How many sessions the strip lists, 1 to 30.
session_rows = 7
```

Every key is optional; without `[deck]` the Deck is as above. The panels fall
in three zones, and the order of `panels` is the order within a zone:

| Zone | Panels | Where |
| --- | --- | --- |
| main | `graph`, `sessions` | the wide column at the left, top to bottom |
| side | `needs`, `verification`, `limits` | the narrow column at the right, top to bottom |
| foot | `guard`, `timeline` | along the bottom, left to right |

A panel you leave out is not drawn, and a zone with no panel gives its room to
the others (`panels = ["graph"]` is the graph alone).

A mistake never stops swarmux. An unknown panel name, a panel listed twice, a
wrong type, a `direction` other than `"lr"`, a `session_rows` out of range or an
unknown key is ignored (the default applies in its place) and the Deck says so
in a line above its panels for as long as it is wrong. `swarmux deck lint`
prints every problem and exits with 1 if there is one; `swarmux deck show`
prints the panels as the Deck draws them:

```text
$ swarmux deck lint
error: [deck] panels: unknown panel "weather" (the panels are graph, sessions, verification, limits, guard, timeline, needs); ignored
deck: 1 problem in /home/you/.swarmux/config.toml
      swarmux ignores each entry above and uses the default for it; the deck says so above its panels.
```

## What the Deck leaves out, for want of data

Nothing here is drawn from a guess. Left out for now: a card's branch (the
read model does not carry one), overlap between two jobs' real changes (only
their claims are compared, never what they changed), a usage-history chart,
spend by day, the number of always-allow rules made today, and a reviewer's
concerns count. Each comes back when the data does.
