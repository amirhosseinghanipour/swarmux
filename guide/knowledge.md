# Knowledge: rules, facts and decisions your agents keep to

Every project has things an agent can't see in the code: what the product
is, how its repositories fit together, and the exceptions that look like
bugs but aren't. Written into a long `CLAUDE.md`, they cost tokens on every
turn, go stale without anyone noticing, and are ignored as soon as the
context is compacted. swarmux keeps them as *knowledge items* instead:
short, scoped, tied to the code they talk about, and, for rules, enforced.

An item is a **rule**, a **fact** or a **decision**. Each has:

| Field | What it is |
| --- | --- |
| text | one to three sentences |
| why | one sentence, written when it was decided |
| scope | the whole workspace, some of its repositories, path globs, task classes (`work`, `review`, `mission`) |
| anchors | files (and optionally a symbol in them) the item is about, with a content hash |
| status | proposed → shadow → enforced or advisory → stale → retired |
| source | owner, correction, gate, extractor or agent-ask, and the ledger entries that recorded it |
| trust | verified or untrusted (what an agent wrote is untrusted until you say otherwise) |

Knowledge belongs to a workspace (see [workspaces](workspaces.md)) or, for
sessions on one repository, to that repository's folder. It is kept in
swarmux's database: every change is an event on the
[audit ledger](ledger-and-evidence.md), and the list of items is rebuilt
from those events with `swarmux knowledge rebuild`.

## The map: what every session starts with

```sh
swarmux knowledge init product
```

drafts a one-page map of the workspace from its scan alone (no model is
asked): its repositories, what each is built with, the checks they offer,
and how they seem to relate (a Flutter app and a Supabase backend: "app is
a client of backend"). It opens in `$VISUAL` or `$EDITOR` (Notepad or `vi`
otherwise). Correct it once: say what the product is, fix the guesses, add
the exceptions. Each line becomes a core fact. Run it again to edit the map;
lines you removed are retired. `--print` shows the draft, `--from file`
reads the page from a file.

The map is kept under 1,500 tokens. Research on context files is clear that
long, generated ones don't help and cost more; a short page the owner wrote
does.

## How agents get knowledge

swarmux puts very little in front of an agent, and only what is in scope:

- **When a session starts**, the map and the prompt rules in scope (up to
  500 tokens more) are added through Claude Code's `SessionStart` hook,
  before the handoff and the Board's digest. The block is ordered by id and
  has no times in it, so it is the same every time and stays in the prompt
  cache.
- **After a compaction** (`SessionStart` with source `compact`) the same
  block is given again, so rules don't fall out of the context with the rest.
- **When the agent reads or edits a file** (Read, Edit, Write, MultiEdit,
  NotebookEdit) that an item is scoped or anchored to, or that a guard rule
  watches, the `PreToolUse` hook adds those items (at most 300 tokens), each
  once per job. After a compaction they come again.

Every block is quoted, says it is project data and not a new task, and tags
each item with its id, kind, standing and source:

```text
- [R2 rule · enforced · owner] "Anonymous users are full users." — why: "the backend relies on it"
```

Stale, retired, proposed and untrusted items never reach an agent. Every
delivery is on the ledger with the items and their token count, so later
you can measure which ones help.

## Rules with teeth

A rule that is only text is a request an agent may ignore. Give it an
enforcer, the cheapest that works:

| Enforcer | What it does |
| --- | --- |
| `guard` | the [Guard](guard.md) checks every tool call: tools, paths it watches, a regular expression over the command or the text being written; then `deny` (refused, the rule is the reason), `ask` (held as a P1 approval that shows the rule) or `warn` (let through, the rule added to the agent's context) |
| `gate` | a hidden check the [done-gate](done-gate.md) runs for missions in scope; the agent only learns that a hidden check failed, never what it runs |
| `lint` | a visible command the done-gate runs with its checks (an analyzer, a linter) |
| `prompt` | text only, given to agents in scope |

Guard rules only ever forbid: they never allow what your `[guard]` rules
deny, and a `deny` in your settings still wins over a rule's `ask`. The
strictest matching rule wins. A rule that can't be evaluated (a broken
pattern) refuses the calls it watches rather than letting them through.
Calls a rule holds are never handed to JEV: they wait for you.

A new guard, gate or lint rule starts in **shadow**: it logs what it would
have done (a *would-deny*, *would-ask* or failed check on the ledger) and
counts it, but doesn't act. When its shadow hits look right, promote it
(`p` on the Knowledge screen, or `swarmux knowledge promote R3`).

`swarmux rules` prints the registry, for example
`4 rules · 2 enforced · 1 failing · 1 advisory-only`. *Failing* are
enforced rules whose last check failed; *advisory-only* are rules that are
still only text, a weakness to fix.

## Staleness

Anchors are hashed again when a workspace is rescanned, when a job starts
and after a mission is delivered. That costs nothing but reading the files.
When an anchored file (or the lines naming its symbol) changed, the item
turns **stale**: it is withheld from agents, and one P3 item in the inbox
lists the stale items of the workspace. Confirm one that still holds (`c`,
or `swarmux knowledge confirm F3`), or edit it; editing a stale item
confirms it too.

## Example: a business rule enforced three ways

*An illustration, not something swarmux ships.* A product's backend treats
anonymous (guest) users as full users, and an agent once "fixed" that and
broke checkout. The rule, enforced three ways:

```sh
# A hidden gate check: no policy may condition on is_anonymous, except an
# allow-list the owner keeps. The agent never sees the command.
swarmux knowledge add --workspace product --kind rule \
  --text "Anonymous users are full users." \
  --why "guests check out; an agent once broke that" \
  --enforcer gate --check "backend: psql -f checks/no_anon_conditions.sql"

# The Guard: an edit of a migration that mentions is_anonymous waits for you.
swarmux knowledge add --workspace product --kind rule \
  --text "Ask before conditioning on anonymous users." \
  --enforcer guard --tool Edit --watch "backend/supabase/migrations/**" \
  --match "is_anonymous" --action ask \
  --anchor "backend/supabase/migrations/0001_auth.sql#is_anonymous"

# One line of text, given only when the task touches auth.
swarmux knowledge add --workspace product --kind rule \
  --text "Guests are users: never treat is_anonymous as 'not signed in'." \
  --path "backend/supabase/**" --path "app/lib/auth/**"
```

The first two start in shadow; promote them once their logs look right.

## Commands

```sh
swarmux knowledge list [--workspace W | --repo DIR] [--all]
swarmux knowledge show R1
swarmux knowledge add --kind rule|fact|decision --text "..." [--why "..."]
    [--in-repo backend] [--path "glob"] [--class mission] [--anchor "file#symbol"] [--core]
    [--enforcer guard|gate|lint|prompt] [--tool Edit] [--watch "glob"] [--match "regex"]
    [--action deny|ask|warn] [--message "..."] [--check "repo: command"] [--status shadow]
swarmux knowledge edit R1 [the same flags; only those given change]
swarmux knowledge promote R1 [--to enforced]
swarmux knowledge confirm F3
swarmux knowledge retire D2
swarmux knowledge trust F4 [--untrusted]
swarmux knowledge recheck
swarmux knowledge rebuild
swarmux knowledge export [--out knowledge.toml]
swarmux knowledge import knowledge.toml
swarmux knowledge init <workspace> [--print | --from page.md]
swarmux rules [--workspace W | --repo DIR]
```

Without `--workspace` or `--repo`, the workspace whose root is the current
folder is meant, else the current folder as a repository.

`export` writes a TOML file you can keep in git (in a workspace's root, or
in the repository); it leaves out counters and times, so it only changes
when the knowledge does. `import` adds new items, puts changed ones and
leaves the rest alone.

Agents may not run `swarmux knowledge` or `swarmux rules` (the Guard
refuses it), so a hidden check stays hidden.

## The Knowledge screen

Press `8`. The registry is on top, then the items: stale ones first, then
proposed ones, rules in shadow (with how often they would have acted), the
other rules, facts and decisions. `↑↓` select, `a` adds one (type
`rule: ...`, `fact: ...` or `decision: ...`; it goes to the selected item's
home), `e` edits its text, `p` promotes, `c` confirms, `x` retires. Scope,
anchors and enforcers are set with `swarmux knowledge edit`.

## Not yet

- Contracts between repositories (tables, RPCs and their consumers) and
  lessons learned from corrections are reserved kinds; nothing creates them
  yet.
- Agents can't ask the project a question yet; their questions go through
  the [Board](board.md).
- Gate and lint rules' checks run in the job's worktree (or task folder),
  not in a clean copy.
