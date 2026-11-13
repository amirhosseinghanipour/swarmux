//! `Workspace` adapter backed by the `git` CLI.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use swarmux_application::ports::Workspace;
use swarmux_application::{AppError, AppResult};
use swarmux_domain::evidence::{ChangeKind, DiffFile};

use crate::process::hide_std;

/// Largest diff handed to a prompt, in bytes.
pub const DIFF_CAP: usize = 64 * 1024;

pub struct GitCli {
    bin: PathBuf,
}

impl GitCli {
    pub fn new(bin: PathBuf) -> GitCli {
        GitCli { bin }
    }
}

impl Default for GitCli {
    fn default() -> GitCli {
        GitCli::new("git".into())
    }
}

/// A `git -C cwd` invocation: no shell, no console window, no stdin, and
/// paths past 260 characters allowed.
pub(crate) fn git_command(bin: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.arg("-C")
        .arg(cwd)
        .args(["-c", "core.longpaths=true"])
        .stdin(Stdio::null());
    hide_std(&mut cmd);
    cmd
}

impl Workspace for GitCli {
    fn git_diff(&self, cwd: &Path) -> AppResult<String> {
        let mut cmd = git_command(&self.bin, cwd);
        cmd.args(["diff", "--no-color", "--no-ext-diff"])
            .stderr(Stdio::null());
        let output = match cmd.output() {
            Ok(output) => output,
            // No git installed: same as no repository.
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(AppError::Workspace(format!("git: {e}"))),
        };
        if !output.status.success() {
            return Ok(String::new());
        }
        Ok(cap(String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    fn git_numstat(&self, cwd: &Path) -> AppResult<Vec<DiffFile>> {
        let mut cmd = git_command(&self.bin, cwd);
        // The same basis as `git_diff`, so the counts and the text agree.
        cmd.args(["-c", "core.quotepath=off", "diff"])
            .args(NUMSTAT_ARGS)
            .stderr(Stdio::null());
        let output = match cmd.output() {
            Ok(output) => output,
            // No git installed: same as no repository.
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AppError::Workspace(format!("git: {e}"))),
        };
        if !output.status.success() {
            return Ok(Vec::new());
        }
        Ok(parse_changes(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// What makes `git diff` list files with the lines counted in each, and how
/// each changed (`--raw`), with a rename shown as a deletion and an
/// addition so every line names one path.
pub(crate) const NUMSTAT_ARGS: [&str; 5] = [
    "--raw",
    "--numstat",
    "--no-renames",
    "--no-color",
    "--no-ext-diff",
];

/// Reads the output of `git diff` run with [`NUMSTAT_ARGS`]: the `--raw`
/// lines say how each path changed, the `--numstat` lines count its lines.
/// A binary file has no lines to count and is marked as one.
pub(crate) fn parse_changes(text: &str) -> Vec<DiffFile> {
    let mut kinds: HashMap<&str, ChangeKind> = HashMap::new();
    let mut out: Vec<DiffFile> = Vec::new();
    for line in text.lines() {
        if let Some(raw) = line.strip_prefix(':') {
            // `:100644 100644 <sha> <sha> M<TAB>path`
            let Some((head, path)) = raw.split_once('\t') else {
                continue;
            };
            if let Some(kind) = head
                .split_whitespace()
                .last()
                .and_then(|s| s.chars().next())
                .and_then(ChangeKind::from_status)
            {
                kinds.insert(path, kind);
            }
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let (Some(ins), Some(del), Some(path)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        let binary = ins == "-" && del == "-";
        let count = |s: &str| s.parse::<u32>().ok();
        let (Some(insertions), Some(deletions)) = (
            if binary { Some(0) } else { count(ins) },
            if binary { Some(0) } else { count(del) },
        ) else {
            continue;
        };
        out.push(DiffFile {
            path: path.to_string(),
            insertions,
            deletions,
            binary,
            kind: None,
        });
    }
    for f in &mut out {
        f.kind = kinds.get(f.path.as_str()).copied();
    }
    out
}

fn cap(mut text: String) -> String {
    if text.len() > DIFF_CAP {
        let mut end = DIFF_CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn diff_of_a_modified_repo() {
        let dir = tempfile::tempdir().unwrap();
        if !git(dir.path(), &["init", "-q"]) {
            eprintln!("git not available; skipping");
            return;
        }
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one\n").unwrap();
        assert!(git(dir.path(), &["add", "a.txt"]));
        assert!(git(dir.path(), &["commit", "-q", "-m", "init"]));
        assert_eq!(GitCli::default().git_diff(dir.path()).unwrap(), "");

        std::fs::write(&file, "one\ntwo é\n").unwrap();
        let diff = GitCli::default().git_diff(dir.path()).unwrap();
        assert!(diff.contains("+two é"), "{diff}");
    }

    #[test]
    fn counts_are_read_per_file_with_how_each_changed() {
        let text = "\
:100644 100644 1111111 2222222 M\tsrc/a.rs
:000000 100644 0000000 3333333 A\tsrc/new file.rs
:100644 000000 4444444 0000000 D\told.txt
:100644 100644 5555555 6666666 M\tlogo.png
12\t3\tsrc/a.rs
7\t0\tsrc/new file.rs
0\t9\told.txt
-\t-\tlogo.png
";
        let files = parse_changes(text);
        assert_eq!(
            files,
            vec![
                DiffFile::text("src/a.rs", 12, 3).with_kind(ChangeKind::Modified),
                DiffFile::text("src/new file.rs", 7, 0).with_kind(ChangeKind::Added),
                DiffFile::text("old.txt", 0, 9).with_kind(ChangeKind::Deleted),
                DiffFile {
                    path: "logo.png".into(),
                    insertions: 0,
                    deletions: 0,
                    binary: true,
                    kind: Some(ChangeKind::Modified),
                },
            ]
        );
        // No raw lines: the counts stand, the kind is not guessed. Lines that
        // are not counts are skipped.
        let bare = parse_changes("3\t1\tx.rs\nnot a line\nq\tr\ts\n");
        assert_eq!(bare, vec![DiffFile::text("x.rs", 3, 1)]);
        assert!(parse_changes("").is_empty());
    }

    #[test]
    fn numstat_of_a_repo_counts_each_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        if !git(dir.path(), &["init", "-q"]) {
            eprintln!("git not available; skipping");
            return;
        }
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(p.join("b.txt"), "x\ny\nz\n").unwrap();
        assert!(git(p, &["add", "."]));
        assert!(git(p, &["commit", "-q", "-m", "init"]));
        assert!(GitCli::default().git_numstat(p).unwrap().is_empty());

        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        std::fs::remove_file(p.join("b.txt")).unwrap();
        let files = GitCli::default().git_numstat(p).unwrap();
        assert_eq!(
            files,
            vec![
                DiffFile::text("a.txt", 2, 0).with_kind(ChangeKind::Modified),
                DiffFile::text("b.txt", 0, 3).with_kind(ChangeKind::Deleted),
            ]
        );
    }

    #[test]
    fn not_a_repo_gives_no_counts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            GitCli::default()
                .git_numstat(dir.path())
                .unwrap()
                .is_empty()
        );
        let missing = GitCli::new(dir.path().join("no-such-git"));
        assert!(missing.git_numstat(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn not_a_repo_or_no_git_gives_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(GitCli::default().git_diff(dir.path()).unwrap(), "");
        let missing = GitCli::new(dir.path().join("no-such-git"));
        assert_eq!(missing.git_diff(dir.path()).unwrap(), "");
    }

    #[test]
    fn cap_cuts_on_a_char_boundary() {
        let text = "é".repeat(DIFF_CAP);
        let capped = cap(text);
        assert!(capped.len() <= DIFF_CAP);
        assert!(capped.chars().all(|c| c == 'é'));
        assert_eq!(cap("short".into()), "short");
    }
}
