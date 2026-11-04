# Install

| Platform | Command |
|---|---|
| Windows | `winget install AmirhosseinGhanipour.Swarmux` or `scoop install swarmux` |
| macOS, Linux | `brew install amirhosseinghanipour/swarmux/swarmux` |
| Any (Rust 1.88+) | `cargo install swarmux` |
| By hand | download the archive for your platform from the GitHub releases page, unpack it and run `swarmux install` |

`swarmux install` copies the binary to a per-user folder (`%LOCALAPPDATA%\Programs\swarmux\bin`
on Windows, `~/.local/bin` elsewhere), puts it on your PATH on Windows, and creates
`~/.swarmux`. Add `--statusline` to print the `statusLine` setting for Claude Code; swarmux
never edits your Claude Code settings. `swarmux uninstall` removes what `install` set up and
lists each thing it removed; your data in `~/.swarmux` stays unless you pass `--purge`.

`swarmux self-update` downloads the newest release for your platform and installs it only
if its minisign signature verifies against the key built into the binary
(`--check` only looks). Release archives are signed; the Windows executable itself is not
code-signed, so SmartScreen may ask for confirmation on first run. Maintainers: see
[RELEASING.md](../RELEASING.md).

## The scripted demo

`swarmux --demo` opens the real interface on a made-up "overnight org": five
agents at work, a Guard denial, a run of routine approvals waiting as one row, a
mission the done-gate sends back once before it passes, and the delivered
mission with its evidence on the Ledger screen (`5`). It plays for about half
a minute and then stays open so you can look around; `q` quits.

It is a script, not a recording of a real run, and every number in it is made
up. The status bar says `DEMO · scripted` the whole time. It works on a plain
install: it does not start `claude`, and it keeps its data in a temporary
folder that is deleted when you quit. Your `~/.swarmux` folder, your config and
Claude Code's settings are not read or changed.

`swarmux --demo --theme NAME` picks one of the built-in themes (see
`swarmux theme list`). The GIF and theme strip in this repository come from the
VHS tapes in `assets/tapes/`.
