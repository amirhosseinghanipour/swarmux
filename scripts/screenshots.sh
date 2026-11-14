#!/bin/sh
# Regenerates the README's screenshots: assets/screens/*.png.
#
#   scripts/screenshots.sh
#
# It plays `swarmux --demo` (a script; nothing in it is a real run) in a tmux
# pane of the size each image needs, waits until the scenario has played,
# presses the keys that reach each screen, captures the pane with its colours
# (`tmux capture-pane -p -e`) and draws it to a PNG with scripts/render_ansi.py.
#
# Needs: tmux, python3 with Pillow (`pip install pillow`; fontTools is
# optional), a monospace font (JetBrains Mono, else DejaVu Sans Mono; or set
# SWX_FONT and SWX_FONT_BOLD to TTF files) and Rust, unless SWARMUX_BIN names
# a built swarmux. Linux and macOS; the demo needs about 40 seconds a pane.
#
# Environment:
#   SWARMUX_BIN  the swarmux to run (default: builds target/release/swarmux)
#   OUT          where the PNGs go (default: assets/screens)
#   THEME        a built-in theme name (default: the app's default theme)
set -eu

cd "$(dirname "$0")/.."
OUT=${OUT:-assets/screens}
THEME=${THEME:-}
mkdir -p "$OUT"

if [ -z "${SWARMUX_BIN:-}" ]; then
  cargo build --release --locked
  SWARMUX_BIN=$PWD/target/release/swarmux
fi

work=$(mktemp -d)
sock=swarmux-shots-$$
session=s
cleanup() {
  tmux -L "$sock" kill-server 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

cat >"$work/tmux.conf" <<'EOF'
set -g status off
set -g default-terminal "xterm-256color"
set -ga terminal-overrides ",*:RGB"
set -g history-limit 100
EOF

tm() { tmux -L "$sock" -f "$work/tmux.conf" "$@"; }

# start COLS ROWS [demo args...]: a pane running the demo, with a throwaway
# home so no config of yours is read.
start() {
  cols=$1
  rows=$2
  shift 2
  theme=
  [ -n "$THEME" ] && theme="--theme $THEME"
  mkdir -p "$work/home"
  tm kill-server 2>/dev/null || true
  tm new-session -d -x "$cols" -y "$rows" -s "$session" \
    "env HOME='$work/home' USERPROFILE='$work/home' TERM=xterm-256color COLORTERM=truecolor '$SWARMUX_BIN' --demo $theme $*"
}

# wait_for TEXT: until TEXT is on screen (150 s at most).
wait_for() {
  n=0
  while ! tm capture-pane -p -t "$session" | grep -qF -- "$1"; do
    n=$((n + 1))
    if [ "$n" -gt 600 ]; then
      echo "screenshots.sh: '$1' never showed on screen:" >&2
      tm capture-pane -p -t "$session" >&2
      exit 1
    fi
    sleep 0.25
  done
}

# played: open the Deck and wait for the scenario to end: the Deck's "this
# run" line reaches $9.27 when the last job has finished. (The status bar
# cannot say: the held tool calls keep jobs counted as running.)
# The limit gauges climb with the demo's clock, so shoot at once.
played() {
  wait_for "g d deck"
  keys g d
  wait_for 'jobs $9.27'
  sleep 1
}

# keys KEY...: one tmux key after the other, with a beat between.
keys() {
  for k in "$@"; do
    tm send-keys -t "$session" "$k"
    sleep 0.3
  done
  sleep 1.2
}

# shot NAME COLS ROWS: capture the pane and render it.
shot() {
  tm capture-pane -p -e -t "$session" >"$work/$1.ans"
  python3 scripts/render_ansi.py "$work/$1.ans" "$OUT/$1.png" "$2" "$3"
}

# The 180x50 panes: the scenario plays on Mission control and takes about 35
# seconds; once the last job has finished the held tool calls stay held. Then
# visit the screens.
start 180 50
played
shot deck 180 50
keys g w
shot work 180 50
keys 5 Down
shot ledger 180 50

# The Return brief: the demo plays the night first, then attaches on it.
start 180 50 --demo-away 60
wait_for "WHILE YOU WERE AWAY"
sleep 2
shot brief 180 50

# The compact Deck.
start 120 36
played
shot deck-compact 120 36

echo "wrote $OUT:"
ls -l "$OUT"
