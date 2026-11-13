//! What an agent's tool calls may not reach: swarmux's own command line, its
//! state and configuration files, and the files that steer agents.
//!
//! All of it is text matching on the call, not a sandbox: an agent runs as
//! the same user and a determined one finds another spelling. The point is
//! that the plain ways are refused, and that every refusal is a decision in
//! the ledger.

use super::glob::{absolute, glob_match, is_windows, normalize};
use super::shell::{split_commands, strip_wrappers};
use crate::gate::{protected_write, words_of};

/// The swarmux commands an agent's shell may run: its own hook, the board's
/// MCP server and the status line. Everything else is the owner's.
pub const AGENT_SUBCOMMANDS: [&str; 3] = ["hook", "board-mcp", "statusline"];

/// Files that steer agents or run code on their own. Writes to them are
/// held for the owner (denied for unattended jobs). The owner may add more.
pub const AGENT_CONFIG_PROTECTED: [&str; 10] = [
    ".claude/settings*.json",
    ".claude/hooks/**",
    ".mcp.json",
    "**/CLAUDE.md",
    "**/CLAUDE.local.md",
    "**/AGENTS.md",
    ".git/hooks/**",
    ".swarmux/**",
    ".claude/commands/**",
    ".claude/agents/**",
];

/// Programs that run the rest of their arguments as a command.
const PASSTHROUGH: [&str; 10] = [
    "xargs", "timeout", "watch", "setsid", "nohup", "env", "command", "time", "exec", "stdbuf",
];
/// Shells whose `-c` argument is a command line.
const SHELLS: [&str; 8] = ["sh", "bash", "zsh", "dash", "fish", "pwsh", "powershell", "cmd"];

fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let base = base.to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
}

fn is_shell_tool(tool: &str) -> bool {
    matches!(tool, "Bash" | "PowerShell" | "Shell")
}

/// A refusal when a shell line runs swarmux with a subcommand other than
/// [`AGENT_SUBCOMMANDS`]. `exe` is the path of the running binary, so a
/// copy under another name is still seen. Looks through `sh -c '…'`,
/// substitutions, `sudo`, `env`, `xargs` and the like.
pub fn agent_cli_refusal(tool: &str, subject: &str, exe: Option<&str>) -> Option<String> {
    if !is_shell_tool(tool) {
        return None;
    }
    scan_line(subject, exe, 0)
}

fn is_swarmux(word: &str, exe: Option<&str>) -> bool {
    if program_name(word) == "swarmux" {
        return true;
    }
    exe.is_some_and(|exe| {
        let (a, b) = (
            normalize(word).unwrap_or_default(),
            normalize(exe).unwrap_or_default(),
        );
        !a.is_empty() && (a == b || (is_windows(&a) && a.eq_ignore_ascii_case(&b)))
    })
}

fn scan_line(line: &str, exe: Option<&str>, depth: usize) -> Option<String> {
    if depth > 3 {
        return None;
    }
    for command in split_commands(line) {
        let words = words_of(strip_wrappers(&command));
        if let Some(why) = scan_words(&words, exe, depth) {
            return Some(why);
        }
    }
    None
}

fn scan_words(words: &[String], exe: Option<&str>, depth: usize) -> Option<String> {
    // The program that really runs: past xargs, timeout 5, nohup and so on.
    let mut at = 0;
    while let Some(w) = words.get(at) {
        let p = program_name(w);
        if PASSTHROUGH.contains(&p.as_str()) {
            at += 1;
            while words
                .get(at)
                .is_some_and(|w| w.starts_with('-') || w.chars().all(|c| c.is_ascii_digit() || c == '.'))
            {
                at += 1;
            }
        } else {
            break;
        }
    }
    let program = words.get(at)?;
    let rest = &words[at + 1..];
    if SHELLS.contains(&program_name(program).as_str()) {
        let inner = rest.iter().position(|w| {
            let w = w.to_ascii_lowercase();
            w == "/c"
                || w == "-command"
                || w == "-c"
                || (w.starts_with('-') && !w.starts_with("--") && w.ends_with('c'))
        })?;
        return rest.get(inner + 1).and_then(|l| scan_line(l, exe, depth + 1));
    }
    if !is_swarmux(program, exe) {
        return None;
    }
    let sub = rest.iter().find(|w| !w.starts_with('-'));
    match sub {
        Some(s) if AGENT_SUBCOMMANDS.contains(&s.as_str()) => None,
        Some(s) => Some(format!(
            "`swarmux {s}` is the owner's; a job may run only swarmux {}",
            AGENT_SUBCOMMANDS.join(", ")
        )),
        None if rest.iter().all(|w| matches!(w.as_str(), "--version" | "-V" | "--help" | "-h")) && !rest.is_empty() => None,
        None => Some(format!(
            "swarmux without one of {} is the owner's",
            AGENT_SUBCOMMANDS.join(", ")
        )),
    }
}

/// Tools whose subject is a path they read or write.
const PATH_TOOLS: [&str; 9] = [
    "Read",
    "Edit",
    "Write",
    "MultiEdit",
    "NotebookEdit",
    "NotebookRead",
    "Glob",
    "Grep",
    "LS",
];

/// `home` for `~` and the spellings of the home variable.
fn expand_home(word: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if let Some(rest) = word.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with(['/', '\\']))
    {
        return format!("{home}{rest}");
    }
    for var in [
        "${HOME}",
        "$HOME",
        "${USERPROFILE}",
        "$USERPROFILE",
        "%USERPROFILE%",
        "%HOME%",
        "$env:USERPROFILE",
        "$env:HOME",
        "$HOMEPATH",
    ] {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with(['/', '\\']))
        {
            return format!("{home}{rest}");
        }
    }
    word.to_string()
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// What a call names that could be a path.
fn path_candidates(tool: &str, subject: &str) -> Vec<String> {
    if PATH_TOOLS.contains(&tool) {
        return vec![subject.trim().to_string()];
    }
    if !is_shell_tool(tool) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for command in split_commands(subject) {
        for word in words_of(strip_wrappers(&command)) {
            let word = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '>' || c == '<');
            if word.is_empty() {
                continue;
            }
            out.push(word.to_string());
            // --file=path, KEY=path
            if let Some((_, v)) = word.split_once('=')
                && !v.is_empty()
            {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// The entry of `fenced` a call touches (reads or writes), if any. `fenced`
/// are absolute paths: a file, or a folder with everything below it. Paths
/// are expanded (`~`, `$HOME`), made absolute against `cwd`, resolved
/// lexically (`..`) and compared by component, not by substring; `resolve`
/// may follow symlinks the way the file system does.
pub fn touches_fenced<'a>(
    fenced: &'a [String],
    tool: &str,
    subject: &str,
    cwd: &str,
    home: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Option<&'a str> {
    let norm = |p: &str| normalize(p).map(|n| n.trim_end_matches('/').to_string());
    let entries: Vec<(&str, String)> = fenced
        .iter()
        .filter_map(|f| norm(f).map(|n| (f.as_str(), n)))
        .collect();
    let eq = |a: &str, b: &str| {
        if is_windows(a) || is_windows(b) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let inside = |path: &str, entry: &str| {
        let path = path.trim_end_matches('/');
        eq(path, entry)
            || (path.len() > entry.len()
                && eq(&path[..entry.len()], entry)
                && path.as_bytes()[entry.len()] == b'/')
    };
    let hit = |path: &str| {
        entries
            .iter()
            .find(|(_, e)| inside(path, e))
            .map(|(orig, _)| *orig)
    };
    if let Some(found) = norm(cwd).and_then(|c| hit(&c)) {
        return Some(found);
    }
    for cand in path_candidates(tool, subject) {
        let expanded = expand_home(&cand, home);
        let Some(abs) = absolute(&expanded, cwd) else {
            continue;
        };
        if has_glob(&abs) {
            let fold = is_windows(&abs);
            let found = entries.iter().find(|(_, e)| {
                glob_match(&abs, e, fold) || glob_match(&abs, &format!("{e}/x"), fold)
            });
            if let Some((orig, _)) = found {
                return Some(orig);
            }
            continue;
        }
        if let Some(found) = hit(&abs) {
            return Some(found);
        }
        if let Some(real) = resolve(&abs).and_then(|r| norm(&r))
            && let Some(found) = hit(&real)
        {
            return Some(found);
        }
    }
    None
}

/// The glob of `globs` a write by this call would touch in the repository
/// at `root`.
pub fn agent_config_write(
    globs: &[String],
    root: &str,
    tool: &str,
    subject: &str,
    cwd: &str,
) -> Option<String> {
    protected_write(globs, root, tool, subject, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(line: &str) -> Option<String> {
        agent_cli_refusal("Bash", line, Some("/opt/bin/swarmux"))
    }

    #[test]
    fn the_hook_the_board_and_the_status_line_are_allowed() {
        for line in [
            "swarmux hook pre-tool",
            "swarmux statusline",
            "swarmux board-mcp",
            "/usr/local/bin/swarmux statusline",
            "swarmux --version",
            "cargo test -p swarmux",
            "grep -r swarmux src",
            "git log --grep swarmux",
            "echo swarmux audit",
            "ls",
        ] {
            assert_eq!(refused(line), None, "{line}");
        }
    }

    #[test]
    fn every_other_subcommand_is_refused() {
        for line in [
            "swarmux audit checkpoint",
            "swarmux daemon stop",
            "swarmux gate run",
            "swarmux verify x",
            "swarmux keys list",
            "swarmux theme list",
            "swarmux uninstall --purge",
            "swarmux self-update",
            "swarmux guard rules remove abcd1234",
            "swarmux",
            "swarmux --flag",
            "swarmux.exe audit checkpoint",
            r"C:\tools\Swarmux.exe audit checkpoint",
            "/opt/bin/swarmux audit",
            "cd x && swarmux audit export-checkpoints",
            "sudo swarmux uninstall",
            "FOO=1 swarmux audit",
            "env swarmux audit",
            "nohup swarmux audit &",
            "timeout 5 swarmux audit checkpoint",
            "xargs swarmux audit",
            "echo $(swarmux audit checkpoint)",
            "bash -c 'swarmux audit checkpoint'",
            "sh -c \"cd x; swarmux gate\"",
            "bash -lc 'swarmux keys list'",
            "pwsh -Command \"swarmux audit\"",
            "cmd /c swarmux audit",
            "bash -c \"sh -c 'swarmux audit'\"",
        ] {
            assert!(refused(line).is_some(), "{line}");
        }
    }

    #[test]
    fn a_renamed_copy_of_the_running_binary_is_seen() {
        let line = "/home/me/.cache/tool audit checkpoint";
        assert!(agent_cli_refusal("Bash", line, Some("/home/me/.cache/tool")).is_some());
        assert_eq!(agent_cli_refusal("Bash", line, Some("/other/path")), None);
        assert_eq!(agent_cli_refusal("Bash", line, None), None);
    }

    #[test]
    fn only_shell_tools_are_looked_at() {
        assert_eq!(
            agent_cli_refusal("Read", "swarmux audit", None),
            None,
            "a file named like a command"
        );
        assert!(agent_cli_refusal("PowerShell", "swarmux audit", None).is_some());
    }

    fn no_links(_: &str) -> Option<String> {
        None
    }

    fn fence() -> Vec<String> {
        vec![
            "/home/u/.swarmux/swarmux.db".into(),
            "/home/u/.swarmux/audit.key".into(),
            "/home/u/.swarmux/config.toml".into(),
            "/home/u/.swarmux/holdout".into(),
        ]
    }

    fn touched(tool: &str, subject: &str) -> Option<String> {
        touches_fenced(&fence(), tool, subject, "/w/app", "/home/u", &no_links)
            .map(str::to_string)
    }

    #[test]
    fn reads_and_writes_of_state_files_are_seen_by_path() {
        for (tool, subject) in [
            ("Read", "/home/u/.swarmux/swarmux.db"),
            ("Read", "~/.swarmux/audit.key"),
            ("Edit", "/home/u/.swarmux/config.toml"),
            ("Read", "/home/u/.swarmux/holdout/abc/tests.py"),
            ("Read", "/home/u/./.swarmux/../.swarmux/audit.key"),
            ("Bash", "cat ~/.swarmux/audit.key"),
            ("Bash", "sqlite3 $HOME/.swarmux/swarmux.db .dump"),
            ("Bash", "cat ${HOME}/.swarmux/config.toml"),
            ("Bash", "echo x >> ~/.swarmux/config.toml"),
            ("Bash", "tee /home/u/.swarmux/config.toml < x"),
            ("Bash", "cp --file=/home/u/.swarmux/audit.key /tmp/k"),
            ("Bash", "cat /home/u/.swarmux/*.key"),
            ("Bash", "cat /home/u/.swarmux/au*"),
            ("Grep", "/home/u/.swarmux/holdout"),
        ] {
            assert!(touched(tool, subject).is_some(), "{tool}: {subject}");
        }
    }

    #[test]
    fn near_misses_are_not_touches() {
        for (tool, subject) in [
            ("Read", "/home/u/.swarmux/worktrees/1/src/a.rs"),
            ("Read", "/home/u/.swarmux/swarmux.dbx"),
            ("Read", "/home/u/.swarmux-other/swarmux.db"),
            ("Bash", "cargo test"),
            ("Bash", "echo swarmux.db"),
            ("Read", "/w/app/audit.key"),
        ] {
            assert_eq!(touched(tool, subject), None, "{tool}: {subject}");
        }
    }

    #[test]
    fn a_session_inside_a_fenced_folder_touches_it() {
        let fenced = fence();
        let found = touches_fenced(
            &fenced,
            "Bash",
            "ls",
            "/home/u/.swarmux/holdout/x",
            "/home/u",
            &no_links,
        );
        assert_eq!(found, Some("/home/u/.swarmux/holdout"));
    }

    #[test]
    fn symlinks_are_followed_when_the_caller_can() {
        let resolve = |p: &str| (p == "/w/app/link").then(|| "/home/u/.swarmux/audit.key".to_string());
        let fenced = fence();
        let found = touches_fenced(&fenced, "Read", "/w/app/link", "/w/app", "/home/u", &resolve);
        assert_eq!(found, Some("/home/u/.swarmux/audit.key"));
    }

    #[test]
    fn windows_paths_compare_without_case() {
        let fenced = vec![r"C:\Users\Me\.swarmux\audit.key".to_string()];
        let found = touches_fenced(
            &fenced,
            "Bash",
            r"type %USERPROFILE%\.swarmux\AUDIT.KEY",
            r"C:\w",
            r"C:\Users\Me",
            &no_links,
        );
        assert!(found.is_some());
    }

    #[test]
    fn agent_config_files_are_matched_as_data() {
        let globs: Vec<String> = AGENT_CONFIG_PROTECTED.iter().map(|s| s.to_string()).collect();
        for (tool, subject) in [
            ("Edit", "/w/.claude/settings.json"),
            ("Write", "/w/.claude/settings.local.json"),
            ("Write", "/w/.claude/hooks/pre.sh"),
            ("Write", "/w/.mcp.json"),
            ("Edit", "/w/CLAUDE.md"),
            ("Edit", "/w/sub/dir/AGENTS.md"),
            ("Write", "/w/.git/hooks/pre-commit"),
            ("Bash", "echo x > .claude/settings.json"),
        ] {
            assert!(
                agent_config_write(&globs, "/w", tool, subject, "/w").is_some(),
                "{tool}: {subject}"
            );
        }
        for (tool, subject) in [
            ("Read", "/w/.claude/settings.json"),
            ("Edit", "/w/src/main.rs"),
            ("Edit", "/w/docs/claude.md.txt"),
        ] {
            assert!(
                agent_config_write(&globs, "/w", tool, subject, "/w").is_none(),
                "{tool}: {subject}"
            );
        }
    }
}
