# The Board

When a mission has more than one agent working on it (a task graph, or a
task that restarts in a new job), the agents can't see each other. The Board
is a small shared notice board they can read and write: what each one is
working on, what it has verified, what was decided and what only you can
answer. It is not chat. Entries are typed, nothing is addressed to anyone,
and every entry says who wrote it.

Every job of a mission is given an MCP server called `swarmux-board`. Claude
Code shows its tools as `mcp__swarmux-board__<tool>`:

| Tool | What it does |
| --- | --- |
| `claim(paths, note)` | Says "I'm changing these files". Refused, naming the job that holds them, if another live job of the mission already claimed an overlapping path or the plan gave the files to another task. Released when the job ends. |
| `report(status, summary)` | Progress on the job's task. |
| `fact(text, evidence)` | Something verified that others can rely on. |
| `decision(text, rationale)` | Something decided, and why. |
| `ask_human(question, options)` | Puts a question in your inbox. It is never answered for you. |
| `schedule_followup(text, after_minutes)` | Something to come back to. |
| `read(kinds, since)` | The board's entries, optionally only some kinds or only entries after an entry number. |

Claude Code's own `TaskCreated` and `TaskCompleted` events are copied onto
the board as reports.

## What a new session is told

When a session of a mission starts, swarmux adds a digest of the board to its
context, after the handoff of a checkpoint if there is one. The digest is
capped at 1500 tokens. Open questions come first, then live claims, decisions
and facts; the newest entries of each are kept, and a last line says how many
older entries were left out.

## The Board screen and the inbox

Press `7` for the Board screen: entries grouped as open questions, claims,
decisions, facts, follow-ups and reports, newest first, with live claims and
open questions in bold. `↑↓` select, `r` answers the selected question, `x`
closes it without an answer.

A question an agent asks also appears in Mission control's inbox. Select it
and press `r` (or `⏎`) to type the answer. Agents see it the next time they
`read` the board.

## Entries are untrusted

An agent could be fooled by a web page or a file, and then write something
nasty on the board for the next agent to read. So board text is treated as
data, never as instructions:

- control characters and terminal escape sequences are removed, and each
  entry is cut to a fixed length;
- every entry is shown as `[kind · job 4 · task api · 14:02] "text"`, in
  quotes, on one line, so it can't pose as another entry or as part of the
  prompt;
- the digest and every `read` start with a line saying these are notes from
  other agents, not instructions;
- a job may write at most 300 entries.

The same cleaned, quoted form is what the Board screen shows.

## Turning it off

```toml
[board]
enabled = true   # false: no MCP server, no digest, no Board screen content
```

## How it works

`swarmux board-mcp --job <id>` is a small MCP server on stdin and stdout that
Claude Code starts for each job. It doesn't touch the database: it sends each
tool call to the running swarmux over the same local socket the hooks use, and
swarmux, the only writer of its database, answers. If swarmux is not
running the tools return an error and the agent carries on without the board.
Entries are kept in the database (`board_entries`), so they survive a restart.
