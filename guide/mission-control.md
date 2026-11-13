# Mission control

Screen 1 is where swarmux starts. It answers three questions: does anything
need me, who is doing what, and will I hit a limit.

- **Needs you** (left): held tool calls, failed jobs, done-gates that gave
  up, reviews with concerns, finished results and stalled jobs, most urgent
  first. Only things that need
  a human decision or a look are here; everything else goes to the change log.
- **Wall**: one card per session in a fixed order (by creation), with model,
  permission mode, task, context, activity, todo progress, done-gate chips
  (`✓ cargo test`, `✗ cargo test`, `gate 1/3`, `evidence missing`) and cost.
  Cards never move when a state changes.
- **Top strip**: sessions by state, queue, burn, machine load, the 5-hour and
  weekly limits with a marker for how far through the window you are, and a
  note on any data source that has gone stale: usage, system, git, GitHub,
  `claude agents`, and the sessions' status lines while a job runs.
- **Changes**: what happened, newest first.

The screen fills the window it gets. Up to about 120x36 it shows the strip, the
inbox and a session list. Around 160x48 it adds the wall, the change log and
the token flow. From 220x60 it adds burn, model mix, agents, tools, files, git,
pull requests, MCP servers, system load and a 12-week activity calendar. From
300x80 it adds a peek at the agent the inbox selection points to.

These are the default keys; [`[keys]`](configuration.md#keys) in the config
changes them, `?` lists the ones in use and `swarmux keys list` prints them all.

| Key | Does |
| --- | --- |
| `Up` / `Down` | select an item (or a wall card, after `Tab`) |
| `Tab` | switch the arrow keys between the inbox and the wall |
| `Enter` | open the selected item's or card's job in Focus |
| `y` / `n` / `a` | approve, deny or always allow the selected held tool call (from any other screen they answer the oldest) |
| `x` | dismiss an item (held tool calls stay until answered) |
| `A` | approve every ask of the selected one's signature at once (same tool, pattern and tier) |
| `s` | shelve the selected item for `1` 15 min, `2` 60 min or `3` until its job ends |
| `u` | bring every shelved item back |
| `F` | focus mode on or off |
| `r` / `R` | take or turn down an offered rule |

### Moving between screens

| Key | Does |
| --- | --- |
| `1`-`9`, `0` | show a screen: `1` mission control, `2` missions, `3` prompts, `4` queue, `5` ledger, `6` sessions, `0` focus, `7` board, `8` knowledge, `9` follow-ups |
| `g` then `m`, `p`, `k` or `w` | the same, by letter: missions, prompts, knowledge, [work](work.md) |
| `g` then `d` or `r` | reserved for screens that are not built yet; until they are, each goes to the nearest screen and says so in the status bar |
| `Esc` | back to the screen you came from (it first closes what is open: the constellation, a highlighted panel); it never quits |
| `q` / `Ctrl+C` | quit (a window attached to the background service lets go and the work goes on) |

After `g` the status bar shows the waiting chord; `Esc` or two seconds
drops it, and any key that is not a destination says it is not bound. While
you type (a prompt, a session name, the mission composer) every key is text,
including `g`.

### The inbox as an alarm panel

The inbox follows alarm-management practice, so that it stays small enough
to act on.

- **Risk.** Every held call is scored from how hard it is to undo
  (R0 read, R1 worktree, R2 repository, R3 outside the machine, R4
  irreversible), how far it reaches, how new the pattern is here, how the
  agent's earlier jobs went and, when JEV is on, what it made of the call.
  The score gives a tier: routine, elevated or destructive.
- **Priorities.** P1 is an irreversible call or a secret on its way out;
  P2 is anything that blocks an agent or has broken; P3 is the rest. The
  header counts them, and `swarmux stats alarms` shows the mix against the
  5 / 15 / 80 % target, the rate of new P1 and P2 items against one per ten
  minutes, the busiest ten minutes and how long items took to be looked at.
- **Irreversible calls** are never one key. `y` opens a dialog that arms
  for 1.5 seconds, then wants the target (the branch or path) typed, or
  picked with `Tab` and a digit from a list whose order changes every time.
  They are never batched, never always-allowed and never shelved.
- **Batches.** Asks with one signature (tool, pattern, tier) show `×N`;
  `A` approves them together. Never across tiers, never P1.
- **Floods.** Ten items inside ten minutes is a flood: items are folded by
  cause, the first one in stays and the rest are held out of the list, with
  a banner. P1 is never folded.
- **Focus mode** (`F`) shows `‖ 3 waiting · next ≤1:40` instead of hiding
  things: P1 shows at once, P2 waits for a breakpoint (a prompt sent, a diff
  closed, Focus closed, another card, ten quiet seconds) and at most two
  minutes, P3 until focus ends and at most 45 minutes. Nothing is
  auto-approved.
- **Shelving** hides a P2 or P3 item for a while. It is logged and comes
  back by itself, and the header shows what is shelved.
- **Rules.** After ten identical approvals with no refusal, the ask offers
  `r make it a rule?` (an allow rule such as `Bash(cargo test:*)`).
- **Load.** The header shows how much of the last 15 minutes you spent in
  the inbox. Above 70 % new jobs wait, and the queue says
  `held: you're at 78% load`; reviews keep running. Set the line with
  `load_hold_at` in `[governor]`, or `1.0` to turn the hold off.

### Where an outcome stands

The console's read model (the data the Work and Deck screens are built on)
puts every outcome, a mission or a job without one, in one of four groups,
using the inbox's own priorities and nothing new:

1. **Decide.** Something needs you: a held call, a question, a failed job, a
   gate that gave up, a recovery that did not work, a plan to approve, a merge
   that cannot happen, a reviewer's concerns, a build that strayed from its
   plan, or a mission that was escalated (always at least P2). Ordered by
   priority, then by the earliest auto-deny deadline (an ask without one
   last), then by the oldest ask.
2. **Ready to land.** The gate passed and the work has not landed. Waiting
   longest first.
3. **Running.** A job is out for it, or it waits for a slot. Verifying first,
   then running, then queued; oldest first.
4. **Landed.** Delivered with proof. Newest first.

A result to read, a quiet job, a follow-up and a knowledge item never move an
outcome out of its group; a finished job nobody asks about is on no queue.
Ties are broken by the outcome's id, so the order never depends on the order
things were read in.

