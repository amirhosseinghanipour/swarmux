# Configuration

Optional `config.toml` (these are the defaults):

```toml
schema_version = 1       # the format of this file; leave it out and it is 1
max_concurrent = 3       # jobs running at the same time
min_free_ram_mb = 1536   # queued jobs wait while free memory is lower
default_model = "sonnet" # model for new sessions
```

Every setting is optional, and every one can be changed. `swarmux config list`
shows them all with their values and where each value came from;
`swarmux config explain guard.unmatched` shows one in full.

## Where the files are

Configuration (what you edit) and state (what swarmux writes: the database, the
ledger, the audit key, hidden checks, the verifier's copies) are two folders.

| | Linux and other Unix | macOS | Windows |
| --- | --- | --- | --- |
| config | `$XDG_CONFIG_HOME/swarmux`, else `~/.config/swarmux` | `~/Library/Application Support/swarmux` (`~/.config/swarmux` is accepted if it exists) | `%APPDATA%\swarmux` |
| state | `$XDG_STATE_HOME/swarmux`, else `~/.local/state/swarmux` | `~/Library/Application Support/swarmux/state` | `%LOCALAPPDATA%\swarmux` |
| organisation's file | `/etc/swarmux/managed.toml` | `/etc/swarmux/managed.toml` | `%ProgramData%\swarmux\managed.toml` |

The order of choice, for each folder: the environment variable
`SWARMUX_CONFIG_DIR` or `SWARMUX_STATE_DIR`; then **`~/.swarmux`, if it exists**;
then the platform's folder above. **An install that already has `~/.swarmux`
keeps working exactly as it did: nothing moves and nothing needs doing.** Only
a home that never had one gets the new folders. `swarmux config paths` prints
every location and why it was chosen. `swarmux config migrate-paths` moves an
old install to the new folders if you want that (it shows the plan first, needs
the background service stopped, leaves worktrees alone, and writes a ledger
entry); it is optional.

## Layers

A value can come from several places. From lowest to highest:

1. **core**: swarmux's defaults.
2. **org**: the organisation's `managed.toml`, for keys it did not lock (a
   default you may change).
3. **user**: your `config.toml`.
4. **workspace**: `<config folder>/workspaces/<name>.toml`, read when you
   start with `SWARMUX_WORKSPACE=<name>` (or pass `--workspace <name>` to
   `swarmux config`).
5. **project**: the repository's `.swarmux/config.toml`. See
   [what a repository may do](trust.md).
6. **pack**: reserved for practice packs.
7. **mission**: a mission's own settings.
8. **run**: `SWARMUX_<SECTION>__<KEY>` environment variables
   (`SWARMUX_GUARD__ON_UNREACHABLE=deny`; a key outside any section is
   `SWARMUX__THEME`) and `--set key=value` on the command line.

Keys the organisation's file **locks** (`locked = ["guard.unmatched"]`) take the
organisation's value whatever any other layer says, the command line included.
That is a client-side control on your own machine: it stops mistakes and
repository files, not you (it is not a security boundary, see
[SECURITY-MODEL.md](../SECURITY-MODEL.md)).

Each key says how its values from several layers become one:

- **scalar**: the nearest layer wins (`max_concurrent`, `theme`).
- **list**: the lists of every layer are joined without repeats
  (`guard.deny`, `guard.protected_paths`).
- **lock**: the strictest wins (`guard.unmatched`: `ask` beats `pass`;
  `guard.on_unreachable`, `missions.landing`, `guard.always_expiry_days`,
  `governor.max_job_usd`). A repository or a workspace can make these
  stricter, never looser. What you type with `--set` wins for that run; an
  environment variable can only narrow.
- **allowlist**: the whole value of the highest layer that sets it
  (`watchdog.rungs`, `watchdog.escalate_to`).

Keys that **grant** something (`guard.allow`, `notify.ntfy`, `followups.order`)
can only be set in your own files: not from the environment, a pack or a
mission, and not from a repository without your trust. Keys that **restrict**
count from every layer at once. `swarmux config explain <key>` shows the merge
rule, the layers in order, what was overridden and any lock.

## Mistakes

Every file is read line by line. A line that is wrong costs only itself, and the
app starts anyway:

- A value that cannot be used is reported (file, line, why) and the key falls
  back to its default. A key that restricts reads as its restrictive answer
  (`guard.on_unreachable` that cannot be read is `"deny"`, `missions.landing`
  is `"manual"`). A list with one bad entry keeps the good ones. An organisation
  allowlist that cannot be read is enforced as empty.
- A key swarmux does not know is a warning in the app and in `swarmux doctor`,
  and an error under `swarmux config lint --strict`.
- A line that is not TOML is dropped and named; the rest is read. A header that
  cannot be read takes its block with it, so keys never end up in the wrong
  section.

`swarmux config lint` lists every problem and exits 1 on an error (`--strict`:
on a warning too), so it can run in CI. The same rules apply to the other layers.
Two files are stricter on purpose, because a typo there would loosen a
policy: an unknown key or a value that cannot be used in a repository's
`gates.toml` makes the gate escalate (it never passes on a file it cannot
read), and a mission contract with one is refused.

## Commands

```sh
swarmux config list [--changed] [--tier user]   # every setting, its value, tier and file
swarmux config get guard.unmatched              # one value, as TOML
swarmux config explain guard.unmatched          # value, tier, file, merge rule, what it overrode, locks
swarmux config lint [--strict]                  # what is wrong; exit 1 on an error
swarmux config export [--tier user]             # the effective config (or one tier's) as TOML
swarmux config schema --out schema              # the JSON Schemas
swarmux config paths                            # where everything is, and why
swarmux config migrate-paths [--yes]            # optional: move ~/.swarmux to the platform folders
```

`--repo <folder>` and `--workspace <name>` add those layers; without `--repo`
the current folder's repository is used when it has a `.swarmux/config.toml`.
`--set key=value` (any command) sets a key for that run. The commands print
plain text, so `NO_COLOR` changes nothing.

## Schemas

`schema/` in the repository holds a JSON Schema for each file (`config`,
`managed`, `gates`, `mission`, `knowledge`, `keys`, `theme`), generated from the
same table of keys the loader uses (`swarmux config schema --out <folder>`
writes them). Editors pick them up through the `#:schema` directive that taplo
and Even Better TOML understand, as the first line of the file:

```toml
#:schema https://raw.githubusercontent.com/amirhosseinghanipour/swarmux/main/schema/config.schema.json
```

(use `gates.schema.json`, `mission.schema.json` and so on for the other files,
or a local path). Each file carries `schema_version`; a number newer than the
swarmux reading it is a warning, and keys it does not know are ignored.

The other settings live next to the feature they control:

| Section | Settings | Guide |
| --- | --- | --- |
| `[telemetry]`, `[pricing]` | which sources run, how often, model prices | [Telemetry](telemetry.md) |
| `theme`, `layout`, `icons`, `motion`, `[themes]`, `[[layouts]]` | look and feel | [Themes](themes.md) |
| `[guard]` | permission rules, the loop breaker, what happens when swarmux can't be reached, "always allow" scope and expiry, protected paths | [Guard](guard.md) |
| `gates.toml` in a repository | the done-gate's checks and protected paths | [Done-gate](done-gate.md) |
| `[verifier]`, `[gate] level`, `[[check]] required/level/env` | clean copies, hidden checks, the harness list, check levels and environment, the revert probe | [Verifier](verifier.md#configuration) |
| `[worktree]` | where worktrees go, port blocks | [Worktrees](worktrees.md) |
| `[missions]` | plan-first, drift, and `landing` | [Missions](missions.md), [Landing](#landing) below |
| `[deck]` | which panels the Deck shows, their order, the graph's direction, the sessions strip | [The Deck](deck.md#customizing-deck) |
| `[console]` | the Work screen's verification window, suggested notes and messages to running agents | [Console](#console) below |
| `[governor]` | budgets, forecasts, auto-resume | [Usage governor](governor.md) |
| `[notify]` | toasts, bell and `ntfy` | [Notifications](notifications.md) |
| `[keys]` | hotkeys | [Keys](#keys) below |

## Keys

Every key in swarmux does a named action, and `[keys]` in `config.toml`
changes which keys do what. Without a `[keys]` section the keys are the ones
this guide lists everywhere else.

```toml
[keys.global]
quit = "ctrl+q"                       # the default `q` no longer quits
"look.next_theme" = ["t", "f2"]       # a list: any of them works
"nav.go_work" = "w"                   # was `g w`
"nav.go_record" = "g r"               # a chord: two keys, one after the other

[keys.queue]
"queue.cancel" = "x"
"queue.retry" = []                    # an empty list: no key at all

[keys."mission.wall"]                 # a context with a dot is quoted
"list.next" = ["down", "j", "right"]
```

- **Where.** A table per *context*: `global` (every screen, after the screen's
  own keys), a screen (`queue`, `ledger`, `sessions`, ...), or a part of one
  (`mission.wall`, `mission.inbox`, `focus.reading`, `knowledge.items`).
  Names with a dot are quoted, as above (`[keys.mission.wall]` also works:
  TOML reads it as a table inside a table and swarmux follows it). Each line
  is an action id and one key or a list of keys.
- **Writing keys.** A character is itself (`x`, `D`, `+`, `?`, and any other
  character your layout types, `é` included). Named keys: `enter`, `tab`,
  `shift+tab`, `space`, `backspace`, `up`, `down`, `left`, `right`, `home`,
  `end`, `pageup`, `pagedown`, `delete`, `insert`, `f1` to `f24`. Modifiers:
  `ctrl+x`, `alt+x`, `shift+up`, and together (`ctrl+alt+f5`); `shift+d` is
  `D`. Keys pressed one after the other are a chord, separated by a space:
  `"g w"` (at most three keys; the pending chord waits two seconds, and `Esc`
  drops it). A chord can't start with a key that is also bound alone in the
  same context.
- **What a line replaces.** The default keys of that action in that context.
  Every action you don't mention keeps its keys. A key an action has no use
  for any more is free for another action in the same context: moving two
  actions onto each other's keys works.
- **Unbinding.** An empty list (`"queue.retry" = []`) leaves the action without
  a key; it still shows in `swarmux keys list` and the key list (`?`) as
  `(none)`, and its hint leaves the footer. The action is still reachable by
  binding it again.
- **Not rebindable.** `Esc` (always goes back, and drops a half-typed chord),
  `Ctrl+C` (always quits) and the typed-confirm challenge for irreversible
  calls (you type the target's name, there is no key to move). The answers to
  an approval, `decide.yes`, `decide.no`, `decide.always` and
  `decide.cancel_plan` (`y`, `n`, `a`, `c`), are fixed in this version too,
  so what answers a held call is predictable on every screen and in every
  config; `swarmux keys lint` says so if you try. While you type (a prompt, a
  session name, the mission composer, search) every key is text and the
  keymap is not consulted.
- **Overlaps.** An action in a screen's context hides the same key in
  `global`; that is how `t` is the theme everywhere and something else on
  Focus. `swarmux keys lint` warns when your config creates a new overlap
  (it is allowed, you may mean it); the overlaps swarmux has by design are
  not warned about.

### Mistakes never leave you without keys

A line with an unknown context or action, a key that can't be read, `Esc` or
`Ctrl+C`, a fixed action, or a key already used by another action in that
context (or a chord that starts another) is ignored; that action keeps its
default keys and the rest of your `[keys]` still applies. If even the default
keys of an action can't be kept next to what you asked for, the whole context
goes back to its defaults. The app starts anyway and says in the status bar,
for a few seconds, how many lines were ignored and to run `swarmux keys lint`
(the key list, `?`, says it as well). A config that is not valid TOML or has a wrong value elsewhere no longer
stops swarmux from starting either: see [Mistakes](#mistakes).

### Commands

```sh
swarmux keys list     # every action: context, keys now, default keys; changed and fixed marked
swarmux keys lint     # every problem and overlap in your [keys], one per line; exit 1 if any error
swarmux keys export   # the keys in use, defaults included, as TOML to paste into config.toml
```

`swarmux keys export > keys.toml` and pasting that into `config.toml` changes
nothing, which is the starting point for editing: change a line and run
`swarmux keys lint`. The output is plain text, so `NO_COLOR` makes no difference.

### On screen

The footer of each screen, the tab numbers, the shelve prompt and the hints in
messages ("press 6 then n") are written from the keys in use, so a rebound key
shows as what it is now, and an action with no key is left out. A footer that
is wider than the room drops hints from the end and never prints half of one;
every footer ends with `? help` and `q quit`, which are the last to go (the
help hint is left out only if you unbind `help.toggle`). `?` opens a list of every key that works on the
current screen and the global ones, with each action's id (what to write in
`[keys]`) and the table it goes in; `?` or `Esc` closes it, the list keys
scroll it. Fixed keys are marked `(fixed)`.

### Windows, other layouts and what can't be bound

Keys are matched by the character they type, so a non-US layout works as
typed: `[` on a German keyboard is `[` (Windows reports AltGr as
`Ctrl+Alt`; swarmux ignores those two when a character comes with them). Some
combinations never reach a terminal program, or arrive as something else, and
can't be bound reliably:

- Legacy terminals send `Ctrl+I` as `Tab`, `Ctrl+M` as `Enter`, `Ctrl+H` as
  `Backspace` and `Ctrl+[` as `Esc`; `Ctrl+Enter`, `Shift+Enter`, `Ctrl+digit`
  and `Ctrl+Shift+letter` are not told apart from their plain keys.
- Windows Terminal keeps some keys for itself by default: `Alt+Enter` and `F11`
  (full screen), `Alt+F4`, `Ctrl+Shift+P`, `Ctrl+Tab`, `Ctrl+,` and whatever
  you bound in its settings. The same goes for your multiplexer's prefix key.
- `Esc` and `Ctrl+C`, above.

## Landing

`landing` in the `[missions]` section says when a workspace mission lands: its
repositories' branches merged into `swarmux/<mission>/integration`, producers
first, all or none (see [Workspaces](workspaces.md)). Only workspace
missions land anything; a mission worked in one folder is delivered when its
gate passes, whatever this says.

```toml
[missions]
landing = "auto"      # "auto" (the default) or "manual"
```

- `"auto"`: the moment the gate passes, swarmux lands the mission and delivers
  it. This is how it has always worked; nothing changes if you leave the
  setting out.
- `"manual"`: the gate passing does not land. The mission waits, verified, under
  READY TO LAND on the [Work screen](work.md) until you land it with `l`. Landing
  then does exactly what `"auto"` does: each repository is merged, producers
  first; if one cannot take its merge, the ones merged before it are put back,
  nothing stays landed, and the mission is escalated to your inbox. When every
  repository landed the mission is delivered and its evidence sealed, so a
  mission waiting to land has no evidence bundle yet. Delivered work is landed
  once; asking again is refused.

Any other word is reported with its line and read as `"manual"`, the cautious
answer.

## Console

The `[console]` section sets what the [Work screen](work.md) shows. Every key
is optional.

```toml
[console]
metrics_hours = 24
send_back_notes = [
  "Read the first failing line of the failed check and fix that before anything else.",
  "Keep the change smaller: touch only what the task needs.",
]
```

- `metrics_hours` (1 to 720, default 24): how many hours back the verification
  figures under the queue (and `swarmux stats verification`) look: first-run
  gate passes, claim to verified, verified an hour, checks run, bundles sealed.
  A value outside the range is reported with its line and the default is used.
- `send_back_notes` (default: three general starting points): the notes the
  send-back line offers when you press `s` on Work; `Tab` puts the next one on
  the line, you edit it or send it as it is. Up to 9 notes of at most 400
  characters each. An empty list (`send_back_notes = []`) turns the
  suggestions off.
- `intervene_mode` (`"queue"` or `"steer"`, default `"queue"`): the mode the
  message line opens in when you press `m` on Work (`Tab` switches it for one
  message, `M` for the ones after). `queue` delivers when the agent's turn
  ends; `steer` stops the turn and continues the session with the message.
  See [Message a running agent](work.md#message-a-running-agent).
- `intervene_max_chars` (1 to 20000, default 2000): the longest message, in
  characters. A longer one is refused with its length and the cap.
- `intervene_confirm_steer` (`true` or `false`, default `true`): steering asks
  for a second `Enter` before it stops the agent's turn. Turning it off makes
  one `Enter` enough; queueing never asks.

The three `intervene_*` keys are tolerant: a value that cannot be used (an
unknown mode, a number out of range, a string where a flag is expected) does
not keep swarmux from starting. It is reported as a configuration warning,
with the key and its line, and the default is used.
