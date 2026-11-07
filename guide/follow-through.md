# Follow-through

When an agent finishes, the most useful part of what it says is often not the
result but the loose ends: "I noticed X", "should I also do Y?", "this needs a
migration first", "I couldn't decide between A and B". Normally these die in
that agent's last message. Follow-through collects them from every agent in
every repository and brings you only what needs you, in a few batches a day.

## What is collected

- The Board's `schedule_followup` and `ask_human` entries. A follow-up with
  `after_minutes` is held back until that time has passed.
- One small-model call for each finished job. It reads the job's goal, its
  final message and the done-gate's verdict, with no tools and no files, and
  lists the follow-ups in it: nothing is invented, and an empty list is a
  fine answer. It costs a fraction of a cent and the cost is in the ledger.
  Turn it off with `extract = false`.

Each follow-up has a kind: **question** (only you can answer), **decision**
(a choice the agent could not make), **task** (extra work worth doing),
**risk** or **blocker**. The same follow-up found by two jobs, or in two
repositories (same kind, same files, same words), is kept once with both
sources. One you declined does not come back when another agent finds it again.

## Why no AI decides for you

Agents that supervise other agents tend to approve what they are shown, and
people shown many approvals approve by reflex. So swarmux uses a model for one
thing only, reading what an agent wrote, and everything after that is plain
code. Whether a follow-up is approved is decided by you, or by a rule you
wrote in advance.

## Standing orders

Standing orders are your playbook, checked in the order you write them; the
first one that matches decides.

```toml
[followups]
brief_times = ["09:00", "13:00", "18:00"]   # local time; [] turns the scheduled brief off
extract = true
extract_model = "haiku"
max_cards = 5          # cards in one brief; the rest are counted
defer_hours = 24       # how long "later" is
protected = ["infra/**"]   # paths no order may approve, on top of the built-in ones

# Typo and documentation fixes in any repository can go straight through.
[[followups.order]]
id = "docs"
kinds = ["task"]
paths = ["docs/**", "**/*.md"]
action = "auto-approve"

# Anything that mentions a migration always comes to you, even if a later
# order would approve it.
[[followups.order]]
id = "migrations"
keywords = ["migration", "schema"]
action = "always-ask"

# Park ideas that are not for now: they stay listed but never reach a brief.
[[followups.order]]
id = "someday"
keywords = ["someday", "nice to have"]
action = "park"
```

An order lists what it matches (`kinds`, `repos`, `paths`, `keywords`,
`max_cost_usd`); every condition you set must hold, and for `paths` every file
the follow-up names must match. `max_cost_usd` only matches when the cost is
known. An `auto-approve` order with no conditions is refused.

Fences you cannot loosen by accident:

- Decisions, risks and blockers are never auto-approved unless the order says
  `allow_risky = true`. Questions are never auto-approved.
- Urgent items are never auto-approved.
- A follow-up that names a protected path (`.env`, `.git`, CI workflows, keys,
  `.claude`, your `protected` list) is never auto-approved, whatever the order.
- Every automatic decision is a ledger entry naming the order.

`swarmux followup orders` lists them, and the Follow-ups screen shows them
under the cards.

## The brief

At each time in `brief_times` (and whenever you ask: `swarmux brief` or
`swarmux followup brief`), swarmux prepares a short brief: "3 tasks finished,
2 met their checks; 2 decisions need you", then one card for each:

```
1) #14 [risk] Migration may lock the users table
   why: the table has 40M rows and the migration adds an index
   from: job 21 · mission payments
   you might say no because: it may be a false alarm; check the evidence first
```

Every card ends with a reason you might say no. The agent that found the
item may supply one; otherwise a fixed one for the kind is used. The point is
that approving should be a decision, not a reflex. A toast or ntfy push says
only how many follow-ups need you, never what they are.

Three batches a day are enough: more interruptions do not help, and none at
all leaves things unattended. Only two kinds of item interrupt you: a risk
the agent marked P1, and a blocker on work that is still running. These
appear in Mission control's inbox, and nothing else from follow-through does.

## Answering

Press `9` for the Follow-ups screen. `↑↓` select, then one key per card:
`a` approve, `d` decline, `l` later, `o` open the job it came from, `m`
approve and draft a mission. From a shell: `swarmux followup approve|decline|defer <id>`,
`swarmux followup list [--all]`.

Approving leaves an **intent**: "approved, not yet a mission". `m` fills the
mission composer on the Missions screen with its text and files so you can
check it and save; `swarmux followup intents` lists those still waiting. A
later release adds a backlog that turns intents into missions.

## The rubber-stamp check

If you approve 95% or more of one kind over your last 20 or more decisions
of it, the brief says so and asks you to open the evidence of one approved
item and check it still holds. It asks at most once a week
(`spot_check_days`) and writes that to the ledger. Approvals made by your
standing orders do not count as yours.

## Turning it off

```toml
[followups]
enabled = false   # nothing is collected; no brief, no screen content
```

## How it works

Follow-ups, intents and a few counters are kept in the database (schema v21);
the CLI and the running swarmux share them, so a decision made in a shell
shows up in the TUI. The background service runs the schedule, so briefs
arrive with no window open. Delivery and replies are behind ports, so a chat
adapter can be added without touching the rules.
