# One screenshot of the scripted demo per theme, for the README's theme
# strip. Render it from the repository root with:
#
#   vhs assets/tapes/themes.tape
#
# That writes assets/theme-<name>.png. Join them side by side afterwards,
# for example with ImageMagick:
#
#   magick montage assets/theme-*.png -tile 4x1 -geometry +4+4 assets/themes.png
#
# The demo is a script, not a real run; its status bar says so.

Require swarmux

Set Shell "bash"
Set FontFamily "Cascadia Code NF, Cascadia Code, JetBrains Mono, Consolas, monospace"
Set FontSize 16
Set LineHeight 1.0
Set Width 1640
Set Height 1000
Set Padding 8
Set TypingSpeed 0
Set Theme { "name": "black", "background": "#000000", "foreground": "#d4d4d4", "cursor": "#d4d4d4" }

Hide
Type "swarmux --demo --theme reference"
Enter
Sleep 14s
Screenshot assets/theme-reference.png
Type "q"
Sleep 1s

Type "swarmux --demo --theme catppuccin-mocha"
Enter
Sleep 14s
Screenshot assets/theme-catppuccin-mocha.png
Type "q"
Sleep 1s

Type "swarmux --demo --theme tokyo-night"
Enter
Sleep 14s
Screenshot assets/theme-tokyo-night.png
Type "q"
Sleep 1s

Type "swarmux --demo --theme gruvbox-dark"
Enter
Sleep 14s
Screenshot assets/theme-gruvbox-dark.png
Type "q"
Sleep 1s
