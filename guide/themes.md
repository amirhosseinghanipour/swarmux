# Themes & layouts

swarmux paints its own background (the default `reference` theme is OLED
black, with orange for Opus/Sonnet and the lead, blue for Haiku and green for
the Guard), so your terminal's colour scheme never shows through (unless you set
`bg = "none"`, below).

| Key | Does |
| --- | --- |
| `t` | next theme (on Focus `t` opens the session in a terminal tab instead): `reference` (alias `oled`), `catppuccin-mocha`, `tokyo-night`, `gruvbox-dark`, `rose-pine`, `nord`, `dracula`, `nocturne`, `catppuccin-latte`, `okabe-ito`, `kanagawa`, `everforest` |
| `L` | next layout: `command` (the dashboard above), `btop` (a dense grid of small panels), `minimal` (lead and log), then yours |
| `Tab` / `Shift+Tab` | move the focus highlight between panels (`Esc` clears it) |

```toml
theme = "tokyo-night"   # any name above
layout = "btop"         # command, btop, minimal or one of [[layouts]]
icons = "unicode"       # "unicode" (default), "ascii", "nerd", or "auto" (see Themes)
gaps = 1                # blank cells between panels, 0-4
fps = 10                # frames per second, 1-30
low_power = false       # true: 4 fps, no easing or pulses (same as motion = "reduced")
motion = "auto"         # "auto", "full", "reduced" or "off" (see Showcase)
splash = true           # the start-up screen (or run with --no-splash)

[[layouts]]             # your own: rows top to bottom
name = "focus"
rows = [
  { panels = ["steps"], height = 1 },
  { panels = ["lead:2", "guard", "usage"], height = 8 },
  { panels = ["timeline:3", "who:2"], weight = 1 },
  { panels = ["log"], weight = 2 },
]
```

A row is `height` rows tall, or shares what is left by `weight`; `"id:n"`
gives a panel `n` shares of its row. Panel ids: `legend`, `command`, `steps`,
`target`, `reviewer`, `lead`, `guard`, `delegate`, `subagents`, `back`,
`timeline`, `who`, `log`, `mix` (tokens by model as a ring), and the
telemetry panels `usage`, `burn`, `tokens`, `context`, `agents`, `todos`,
`tools`, `files`, `git`, `ci`, `system`, `wall`, `calendar`, `env`, which
show "waiting for data" until their feed is connected. A panel too small for
its full view collapses to one line; when even that doesn't fit, the least
important panels are hidden first.

### Themes

Every theme keeps text at 4.5:1 or better against its background and every
coloured mark at 3:1 or better; a test checks all of them. `reference` is
pure `#000000` and stays the default (`oled` is another name for it).
`nocturne` is near-black blue-grey with a periwinkle accent,
`catppuccin-latte` is the light one, and `okabe-ito` uses the eight
colour-blind-safe Okabe-Ito hues, so warnings, errors and "ok" stay apart for
deuteranopes.

```toml
theme = "nocturne"
bg = "none"             # don't paint a background: panels and gaps show your terminal's
icons = "auto"          # Nerd Font glyphs when SWARMUX_ICONS=nerd, NERD_FONT=1 or `font` says so
font = "JetBrainsMono NF"   # a hint for icons = "auto"; "Nerd" or an NF/NFM/NFP suffix counts

[themes.mine]           # your own; unset roles come from `base` (default: reference)
base = "nord"
lead = "#ff9e64"
error = "#ff5c6c"
gauge = ["#7fd99a", "#e6c37a", "#ff6b7a"]   # low, mid, high
```

With `bg = "none"` the screen background, panels and the gaps between them are
left to the terminal, which is how a transparent or blurred terminal shows
through. Choose a theme whose text suits your terminal's background.

Take colours from the rest of your desktop; swarmux reads the file again
within a second of it changing:

| `theme =` | Reads |
| --- | --- |
| `"wal"` | pywal, `~/.cache/wal/colors.json` |
| `"base16:~/schemes/gruvbox.yaml"` | a base16 scheme (flat or `palette:` YAML) |
| `"matugen:~/.cache/matugen/colors.json"` | matugen's JSON (the `dark` scheme) |

Share a theme, or give your terminal the same palette:

```
swarmux theme list
swarmux theme export nocturne                      # a [themes.nocturne] block for config.toml
swarmux theme export nord --format base16          # also: kitty, alacritty, wezterm
swarmux theme export wal --format kitty > wal.conf # imports can be exported too
```

Colours are truecolor when `COLORTERM` is `truecolor` or `24bit` (or inside
Windows Terminal) and fall back to the 256-colour palette otherwise.

## Showcase

**`swarmux fetch`** prints a neofetch-style card: the logo in your theme's
gradient, version, theme, sessions, agents running, today's spend and burn,
the 5-hour and 7-day limits, missions delivered, the ledger head, the Claude
Code version and OS, a stacked bar of spend by model, and the terminal's 16
colours. It reads only and creates nothing.

```
swarmux fetch                      # the theme from your config
swarmux fetch --theme nocturne     # any theme, or wal / base16:<file> / matugen:<file>
swarmux fetch --color never        # also: NO_COLOR=1, or piping the output
```

Colour is truecolor, 256 or 16 colours by what the terminal says, and off
under `NO_COLOR` or when the output is not a terminal (`--color always` forces
it, except against `NO_COLOR`).

Two screens for the screenshot, from Mission control, Focus or Sessions:

- **`z` Constellation.** Agents orbit a core on a braille canvas: the orbit's
  radius is the agent's context fill, the star's brightness its tokens per
  second, a comet tail follows tool calls, a star that needs an approval
  flares, and a finished agent goes nova and settles as a dim check.
- **`S` Spectrum.** Cava-style bars per agent: prompt tokens per minute rise
  above the line, output tokens hang below it, caps hold the recent peaks.

`z` / `S` switch between them, `esc` leaves, `t` still changes the theme, and
`bg = "none"` shows your wallpaper through both. When a P1 or P2 item reaches
the inbox the animation stops, everything but the star that needs you dims,
and a one-line banner says what is waiting; `y` / `n` still answer a held
call, `1` goes to Mission control.

`motion` sets how much moves. `full` runs every effect, `reduced` keeps data
updates and spinners at 4 fps and holds the showcase still, `off` keeps only
data updates. The default `auto` is `full` and drops to `reduced` on battery,
over SSH (`SSH_CONNECTION` or `SSH_TTY`) and when drawing a frame takes more
than 8 ms; an explicit `full` is never lowered.

