# Configuration

Optional `~/.swarmux/config.toml` (these are the defaults):

```toml
max_concurrent = 3       # jobs running at the same time
min_free_ram_mb = 1536   # queued jobs wait while free memory is lower
default_model = "sonnet" # model for new sessions
```

The other settings live next to the feature they control:

| Section | Settings | Guide |
| --- | --- | --- |
| `[telemetry]`, `[pricing]` | which sources run, how often, model prices | [Telemetry](telemetry.md) |
| `theme`, `layout`, `icons`, `motion`, `[themes]`, `[[layouts]]` | look and feel | [Themes](themes.md) |
| `[guard]` | permission rules and the loop breaker | [Guard](guard.md) |
| `[jev]` | the optional hosted decision model | [JEV](jev.md) |
| `gates.toml` in a repository | the done-gate's checks and protected paths | [Done-gate](done-gate.md) |
| `[verifier]` | clean copies and hidden checks | [Verifier](verifier.md) |
| `[worktree]` | where worktrees go, port blocks | [Worktrees](worktrees.md) |
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
(the key list, `?`, says it as well). A config that is not valid TOML, or
has a wrong value elsewhere, still stops swarmux from reading the file, as
before; `[keys]` itself never does.

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
