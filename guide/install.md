# Install

| Platform | Command |
|---|---|
| Windows | `winget install AmirhosseinGhanipour.Swarmux` or `scoop install swarmux` |
| macOS, Linux | `brew install amirhosseinghanipour/swarmux/swarmux` |
| Any (Rust 1.88+) | `cargo install swarmux` |
| By hand | download the archive for your platform from the GitHub releases page, unpack it and run `swarmux install` |

`swarmux install` copies the binary to a per-user folder (`%LOCALAPPDATA%\Programs\swarmux\bin`
on Windows, `~/.local/bin` elsewhere), puts it on your PATH on Windows, and creates
its data folder (`~/.swarmux` where that exists, else the platform's state folder, see [Configuration](configuration.md#where-the-files-are)). Add `--statusline` to print the `statusLine` setting for Claude Code; swarmux
never edits your Claude Code settings. `swarmux uninstall` removes what `install` set up and
lists each thing it removed; your data stays unless you pass `--purge`. It
stops the [background service](daemon.md) first when it runs.

`swarmux self-update` downloads the newest release for your platform and installs it only
if its minisign signature verifies against the key built into the binary
(`--check` only looks). Release archives are signed; the Windows executable itself is not
code-signed, so SmartScreen may ask for confirmation on first run. A background service started
before the update keeps running the old version; the next `swarmux` says so and offers to
restart it when nothing is running. Maintainers: see
[RELEASING.md](../RELEASING.md).

## The scripted demo

`swarmux --demo` opens the real interface on a made-up "overnight org": five
agents at work, routine `cargo check` calls the Guard lets through, a Guard
denial, a push held for you, a docs agent whose first "done" the done-gate sends
back and that then waits on two held calls, a mission the gate sends back once
before it passes, and the delivered mission with its evidence on the Ledger
screen (`5`). It plays on its own and then stays open so you can look
around; `q` quits. The checks run in a clean copy, as they do for real work,
and the demo's clock reads a day in November 2026.

It is a script, not a recording of a real run, and every number in it is made
up. The status bar says `DEMO · scripted` the whole time. It works on a plain
install: it does not start `claude`, and it keeps its data in a temporary
folder that is deleted when you quit. Your `~/.swarmux` folder, your config and
Claude Code's settings are not read or changed.

`swarmux --demo --snapshot DIR` plays the same script without a terminal and
writes its key moments into `DIR` as JSON frames (rows of styled text, colours
as hex), one file per moment. Add `--theme NAME` for another theme.

`swarmux --demo --theme NAME` picks one of the built-in themes (see
`swarmux theme list`). `swarmux --demo --demo-away MINUTES` starts as if you had
been away that long, on the [Return brief](brief.md). The screenshots in the
README are captured from the demo by `scripts/screenshots.sh` (see
[Contributing](../CONTRIBUTING.md#screenshots)).
