# Notifications

When something needs you and swarmux is in the background, it can tell you.
It only does so for the inbox's own priorities: a **P1** item always notifies
(unless the kill switch is off), a **P2** item notifies once, and only while
the terminal window is unfocused and outside quiet hours; **P3** never does.
More than three new items at once become a single summary. Items that are
held back by deferral are not sent until they are due.

Focus comes from the terminal's focus reports. A terminal that sends none
counts as unfocused after 30 seconds without a key.

```toml
[notify]
enabled = true            # kill switch: false silences everything, P1 included
toast = true              # native Windows toast (no PowerShell process); default on Windows
terminal = "osc777"       # "off", "osc777", "osc9" or "bell"; default off on Windows, osc777 elsewhere
quiet = "22:00-08:00"     # local time; P2 stays silent (and is not sent later). P1 ignores it
# ntfy = "https://ntfy.sh/my-private-topic"   # opt-in push to your phone
```

- `terminal` writes a notification escape sequence to the terminal:
  `osc777` for VTE, foot, WezTerm and Ghostty; `osc9` for iTerm2, ConEmu and
  Kitty; `bell` for the plain bell. Pick one, not both.
- `ntfy` is off until you set a topic URL. Only a short title (`swarmux P2`)
  and the kind of item (`an approval is waiting`) are sent: never a command,
  path, prompt or session name. Use a topic name nobody can guess.
- The toast uses the PowerShell application id so Windows shows it without
  an installed app; swarmux does not start PowerShell.
