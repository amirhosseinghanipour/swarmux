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

