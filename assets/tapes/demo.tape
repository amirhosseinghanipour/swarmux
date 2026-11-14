# The README's demo GIF: `swarmux --demo`, a scripted scenario (nothing in it
# is a real run). Render it from the repository root with:
#
#   vhs assets/tapes/demo.tape
#
# The scenario takes about 32 seconds; the GIF plays it at 4x, so it lasts
# about 8 seconds. The window is about 160 columns by 48 rows.

Output assets/demo.gif

Require swarmux

Set Shell "bash"
Set FontFamily "Cascadia Code NF, Cascadia Code, JetBrains Mono, Consolas, monospace"
Set FontSize 16
Set LineHeight 1.0
Set Width 1640
Set Height 1000
Set Padding 8
Set TypingSpeed 0
Set PlaybackSpeed 4
# The "reference" theme draws on black.
Set Theme { "name": "black", "background": "#000000", "foreground": "#d4d4d4", "cursor": "#d4d4d4" }

# Start the demo without showing the typing.
Hide
Type "swarmux --demo"
Enter
Show

# Mission control: agents at work, a Guard denial, a held push, a gate that
# sends a mission back once and then passes.
Sleep 16s

# The Ledger: the delivered mission and its evidence card.
Type "5"
Sleep 8s

# Back to Mission control for the last jobs finishing.
Type "1"
Sleep 8s

Hide
Type "q"
