//! Working tree state through the `git` CLI.

use std::path::Path;
use std::time::Duration;

use swarmux_application::{Commit, DiffStat, GitStatus};

use super::cli::{self, CliError};

const LIMIT: Duration = Duration::from_secs(10);
const COMMITS: &str = "-5";

/// Branch, upstream, change counts, diff size and recent commits of the
/// repository at `cwd`.
pub fn status(git: &Path, cwd: &Path) -> Result<GitStatus, CliError> {
    let run = |args: &[&str]| {
        // Never take the index lock: this runs in the background.
        let mut all = vec!["--no-optional-locks", "-C"];
        let dir = cwd.to_string_lossy();
        all.push(&dir);
        all.extend_from_slice(args);
        cli::run(git, &all, None, LIMIT)
    };
    let porcelain = run(&["status", "--porcelain=v2", "--branch"])?;
    let mut s = parse_status(&porcelain);
    s.cwd = cwd.display().to_string();
    s.diff = if s.head.is_some() {
        parse_numstat(&run(&[
            "diff",
            "HEAD",
            "--numstat",
            "--no-ext-diff",
            "--no-color",
        ])?)
    } else {
        // No commit yet: everything staged plus what changed since.
        let staged = parse_numstat(&run(&["diff", "--cached", "--numstat", "--no-color"])?);
        let unstaged = parse_numstat(&run(&["diff", "--numstat", "--no-color"])?);
        DiffStat {
            files: staged.files + unstaged.files,
            insertions: staged.insertions + unstaged.insertions,
            deletions: staged.deletions + unstaged.deletions,
        }
    };
    if s.head.is_some() {
        s.commits = parse_log(&run(&["log", COMMITS, "--format=%h%x1f%ct%x1f%s"])?);
    }
    Ok(s)
}

/// `git status --porcelain=v2 --branch` output.
pub fn parse_status(text: &str) -> GitStatus {
    let mut s = GitStatus::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            s.head = (rest != "(initial)").then(|| rest.chars().take(7).collect());
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            s.branch = (rest != "(detached)").then(|| rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                let n = |p: &str| p[1..].parse::<u32>().unwrap_or(0);
                match part.as_bytes().first() {
                    Some(b'+') => s.ahead = n(part),
                    Some(b'-') => s.behind = n(part),
                    _ => {}
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let xy = line.as_bytes().get(2..4).unwrap_or(b"..");
            s.staged += u32::from(xy[0] != b'.');
            s.unstaged += u32::from(xy[1] != b'.');
        } else if line.starts_with("u ") {
            s.conflicted += 1;
        } else if line.starts_with("? ") {
            s.untracked += 1;
        }
    }
    s
}

/// `git diff --numstat` output; binary files count as files only.
pub fn parse_numstat(text: &str) -> DiffStat {
    let mut d = DiffStat::default();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split('\t');
        let add = parts
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);
        let del = parts
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);
        d.files += 1;
        d.insertions += add;
        d.deletions += del;
    }
    d
}

/// `git log --format=%h%x1f%ct%x1f%s` output.
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\u{1f}');
            let hash = parts.next()?.trim().to_string();
            let secs: i64 = parts.next()?.trim().parse().ok()?;
            let subject = parts.next().unwrap_or("").to_string();
            (!hash.is_empty()).then_some(Commit {
                hash,
                subject,
                at: secs * 1000,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn porcelain_v2_counts() {
        let text = "# branch.oid 0123456789abcdef\n# branch.head main\n\
                    # branch.upstream origin/main\n# branch.ab +2 -1\n\
                    1 M. N... 100644 100644 100644 a b src/a.rs\n\
                    1 .M N... 100644 100644 100644 a b src/b.rs\n\
                    1 MM N... 100644 100644 100644 a b src/c.rs\n\
                    2 R. N... 100644 100644 100644 a b R100 new.rs\told.rs\n\
                    u UU N... 100644 100644 100644 100644 a b c x.rs\n\
                    ? notes.txt\n? tmp/\n";
        let s = parse_status(text);
        assert_eq!(s.head.as_deref(), Some("0123456"));
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicted),
            (3, 2, 2, 1)
        );
        let fresh = parse_status("# branch.oid (initial)\n# branch.head (detached)\n");
        assert_eq!((fresh.head, fresh.branch), (None, None));
    }

    #[test]
    fn numstat_and_log() {
        let d = parse_numstat("3\t1\tsrc/a.rs\n-\t-\timg.png\n10\t0\tb.rs\n");
        assert_eq!(
            d,
            DiffStat {
                files: 3,
                insertions: 13,
                deletions: 1
            }
        );
        let log = parse_log("abc1234\u{1f}1700000000\u{1f}feat: x \u{1f} y\nbad\n");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "feat: x \u{1f} y");
        assert_eq!(log[0].at, 1_700_000_000_000);
    }

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
    fn status_of_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        if !git(dir.path(), &["init", "-q", "-b", "main"]) {
            return eprintln!("git not available; skipping");
        }
        let bin = Path::new("git");
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let s = status(bin, dir.path()).unwrap();
        assert_eq!((s.head, s.untracked, s.commits.len()), (None, 1, 0));
        assert!(git(dir.path(), &["add", "a.txt"]));
        let s = status(bin, dir.path()).unwrap();
        assert_eq!((s.staged, s.diff.insertions), (1, 1));
        assert!(git(dir.path(), &["commit", "-q", "-m", "first commit"]));
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        let s = status(bin, dir.path()).unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert!(s.head.is_some());
        assert_eq!((s.staged, s.unstaged, s.untracked), (0, 1, 1));
        assert_eq!(
            s.diff,
            DiffStat {
                files: 1,
                insertions: 2,
                deletions: 0
            }
        );
        assert_eq!(s.commits.len(), 1);
        assert_eq!(s.commits[0].subject, "first commit");
        assert_eq!(s.upstream, None);
        let outside = tempfile::tempdir().unwrap();
        assert!(matches!(
            status(bin, outside.path()),
            Err(CliError::Failed(_))
        ));
    }
}
