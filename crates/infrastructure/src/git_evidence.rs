//! What evidence needs of git: where a repository's work stands, how big
//! a change is, and clean copies of a commit to run checks in.
//!
//! git is started directly, with no shell and no console window.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicU32, Ordering};

use swarmux_application::{AppError, AppResult, ChangedFiles, Checkout, Checkouts, HeadFacts, Vcs};
use swarmux_domain::evidence::{ChangeKind, DiffFile, DiffSummary, diff_totals};
use swarmux_domain::ledger::{hex, sha256};

use crate::git::{NUMSTAT_ARGS, git_command, parse_changes};
use crate::worktree::remove_dir_retrying;

/// The empty tree of a SHA-1 repository: what a root commit changes from.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Where a branch usually left the main line from, in order of trust.
const MAIN_LINES: [&str; 5] = [
    "origin/HEAD",
    "origin/main",
    "origin/master",
    "main",
    "master",
];

/// Reads a repository with the `git` CLI.
pub struct GitVcs {
    bin: PathBuf,
}

impl GitVcs {
    pub fn new(bin: PathBuf) -> GitVcs {
        GitVcs { bin }
    }

    fn output(&self, cwd: &Path, args: &[&str]) -> AppResult<Output> {
        git_command(&self.bin, cwd)
            .args(["-c", "core.quotepath=off"])
            .args(args)
            .output()
            .map_err(|e| AppError::Workspace(format!("git: {e}")))
    }

    /// Trimmed stdout of a git command that succeeded.
    fn ok(&self, cwd: &Path, args: &[&str]) -> Option<String> {
        let out = self.output(cwd, args).ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl Default for GitVcs {
    fn default() -> GitVcs {
        GitVcs::new("git".into())
    }
}

impl Vcs for GitVcs {
    fn head(&self, dir: &Path) -> AppResult<Option<HeadFacts>> {
        let Some(top) = self.ok(dir, &["rev-parse", "--show-toplevel"]) else {
            return Ok(None);
        };
        let Some(head) = self
            .ok(dir, &["rev-parse", "HEAD"])
            .filter(|h| !h.is_empty())
        else {
            return Ok(None);
        };
        // swarmux's own files are not the agent's work.
        let dirty = self
            .ok(
                dir,
                &["status", "--porcelain", "--", ".", ":(exclude).swarmux"],
            )
            .is_some_and(|s| !s.is_empty());
        let branch = self
            .ok(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
            .filter(|b| !b.is_empty() && b != "HEAD");
        let base = MAIN_LINES
            .iter()
            .find_map(|line| self.ok(dir, &["merge-base", "HEAD", line]))
            .filter(|b| !b.is_empty() && *b != head);
        Ok(Some(HeadFacts {
            head,
            dirty,
            branch,
            base,
            repo: PathBuf::from(top),
        }))
    }

    fn diff(&self, dir: &Path, base: Option<&str>, head: &str) -> AppResult<DiffSummary> {
        let parent = format!("{head}^");
        let from = match base {
            Some(b) => b.to_string(),
            None => self
                .ok(dir, &["rev-parse", "--verify", "--quiet", &parent])
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| EMPTY_TREE.to_string()),
        };
        let stat = self
            .ok(
                dir,
                &["diff", "--raw", "--numstat", "--no-renames", &from, head],
            )
            .unwrap_or_default();
        // Binary files show "-": no lines to count, marked as binary.
        let changes = parse_changes(&stat);
        let files = u32::try_from(changes.len()).unwrap_or(u32::MAX);
        let (insertions, deletions) = diff_totals(&changes);
        let text = self.output(dir, &["diff", "--no-color", "--no-ext-diff", &from, head])?;
        let hash = if text.status.success() && !text.stdout.is_empty() {
            hex(&sha256(&text.stdout))
        } else {
            String::new()
        };
        Ok(DiffSummary {
            files,
            insertions,
            deletions,
            hash,
            changes,
        })
    }
}

impl ChangedFiles for GitVcs {
    /// What the work in `dir` changed: committed since the branch left the
    /// main line, uncommitted, and new files, by repository-relative path.
    fn changed(&self, dir: &Path) -> AppResult<Vec<String>> {
        let Some(facts) = self.head(dir)? else {
            return Err(AppError::Workspace(format!(
                "{} is not in a git repository",
                dir.display()
            )));
        };
        let from = facts.base.unwrap_or(facts.head);
        let mut files: Vec<String> = Vec::new();
        for args in [
            vec!["diff", "--name-only", "--no-renames", &from],
            vec!["ls-files", "--others", "--exclude-standard"],
        ] {
            let out = self.output(dir, &args)?;
            if !out.status.success() {
                return Err(AppError::Workspace(format!(
                    "git {} failed",
                    args.first().copied().unwrap_or_default()
                )));
            }
            files.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string),
            );
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// What `changed` lists, with the lines git counted in each file:
    /// everything that differs from where the branch left the main line
    /// (committed or not), and new files, whose lines are counted by
    /// reading them (git has no diff for a file it does not track yet).
    fn changed_stats(&self, dir: &Path) -> AppResult<Vec<DiffFile>> {
        let Some(facts) = self.head(dir)? else {
            return Err(AppError::Workspace(format!(
                "{} is not in a git repository",
                dir.display()
            )));
        };
        let from = facts.base.unwrap_or(facts.head);
        let mut args = vec!["diff"];
        args.extend(NUMSTAT_ARGS);
        args.push(&from);
        let out = self.output(dir, &args)?;
        if !out.status.success() {
            return Err(AppError::Workspace("git diff failed".into()));
        }
        let mut files = parse_changes(&String::from_utf8_lossy(&out.stdout));
        let others = self.output(dir, &["ls-files", "--others", "--exclude-standard"])?;
        if !others.status.success() {
            return Err(AppError::Workspace("git ls-files failed".into()));
        }
        for name in String::from_utf8_lossy(&others.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            if files.iter().any(|f| f.path == name) {
                continue;
            }
            files.push(new_file(dir, name));
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }
}

/// The most of a new file read to count its lines.
const COUNT_CAP: u64 = 4 * 1024 * 1024;

/// An untracked file as an addition: its lines counted by reading it, or a
/// binary file (a NUL byte in its first kilobytes) with none.
fn new_file(dir: &Path, name: &str) -> DiffFile {
    use std::io::Read;
    let mut bytes = Vec::new();
    let read = std::fs::File::open(dir.join(name))
        .and_then(|f| f.take(COUNT_CAP).read_to_end(&mut bytes))
        .is_ok();
    let binary = read && bytes.iter().take(8000).any(|b| *b == 0);
    let lines = if !read || binary || bytes.is_empty() {
        0
    } else {
        let newlines = bytes.iter().filter(|b| **b == b'\n').count();
        newlines + usize::from(bytes.last() != Some(&b'\n'))
    };
    DiffFile {
        path: name.to_string(),
        insertions: u32::try_from(lines).unwrap_or(u32::MAX),
        deletions: 0,
        binary,
        kind: Some(ChangeKind::Added),
    }
}

/// Makes clean copies of commits as detached worktrees under the system's
/// temporary folder.
pub struct GitCheckouts {
    bin: PathBuf,
    base: PathBuf,
}

impl GitCheckouts {
    pub fn new(bin: PathBuf) -> GitCheckouts {
        GitCheckouts {
            bin,
            base: std::env::temp_dir(),
        }
    }

    /// Puts the copies under `base` instead of the temporary folder.
    pub fn under(mut self, base: PathBuf) -> GitCheckouts {
        self.base = base;
        self
    }
}

impl Default for GitCheckouts {
    fn default() -> GitCheckouts {
        GitCheckouts::new("git".into())
    }
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct GitCheckout {
    bin: PathBuf,
    repo: PathBuf,
    dir: PathBuf,
}

impl Checkout for GitCheckout {
    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for GitCheckout {
    fn drop(&mut self) {
        let removed = git_command(&self.bin, &self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.dir)
            .output()
            .is_ok_and(|o| o.status.success());
        if !removed {
            // A folder git no longer knows about, or could not remove.
            let _ = remove_dir_retrying(&self.dir);
            let _ = git_command(&self.bin, &self.repo)
                .args(["worktree", "prune"])
                .output();
        }
    }
}

impl Checkouts for GitCheckouts {
    fn holdout_key(&self, repo: &Path) -> Option<String> {
        crate::clean_checkout::holdout_key(&self.bin, repo)
    }

    fn checkout(&self, repo: &Path, sha: &str) -> AppResult<Box<dyn Checkout>> {
        let dir = self.base.join(format!(
            "swarmux-verify-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let out = git_command(&self.bin, repo)
            .args(["worktree", "add", "--detach", "--force"])
            .arg(&dir)
            .arg(sha)
            .output()
            .map_err(|e| AppError::Workspace(format!("git: {e}")))?;
        if !out.status.success() {
            return Err(AppError::Workspace(format!(
                "cannot check out {} of {}: {}",
                sha.get(..12).unwrap_or(sha),
                repo.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(Box::new(GitCheckout {
            bin: self.bin.clone(),
            repo: repo.to_path_buf(),
            dir,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

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

    /// A repository with a main line and a branch two files further on.
    fn repo() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().ok()?;
        let p = dir.path();
        if !git(p, &["init", "-q", "-b", "main"]) {
            eprintln!("git not available; skipping");
            return None;
        }
        std::fs::write(p.join("a.txt"), "one\n").ok()?;
        assert!(git(p, &["add", "a.txt"]));
        assert!(git(p, &["commit", "-q", "-m", "init"]));
        assert!(git(p, &["checkout", "-q", "-b", "work"]));
        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").ok()?;
        std::fs::write(p.join("b.txt"), "b\n").ok()?;
        assert!(git(p, &["add", "."]));
        assert!(git(p, &["commit", "-q", "-m", "work"]));
        Some(dir)
    }

    #[test]
    fn changed_files_are_committed_uncommitted_and_new() {
        let Some(dir) = repo() else { return };
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b2\n").unwrap();
        std::fs::write(p.join("new.txt"), "n\n").unwrap();
        let files = GitVcs::default().changed(p).unwrap();
        assert_eq!(files, ["a.txt", "b.txt", "new.txt"]);
        let outside = tempfile::tempdir().unwrap();
        assert!(GitVcs::default().changed(outside.path()).is_err());
    }

    #[test]
    fn head_branch_base_and_diff() {
        let Some(dir) = repo() else { return };
        let vcs = GitVcs::default();
        let facts = vcs.head(dir.path()).unwrap().unwrap();
        assert_eq!(facts.branch.as_deref(), Some("work"));
        assert!(!facts.dirty);
        assert_eq!(facts.head.len(), 40);
        let base = facts.base.clone().expect("the main line is the base");
        assert_ne!(base, facts.head);
        let d = vcs.diff(dir.path(), Some(&base), &facts.head).unwrap();
        assert_eq!((d.files, d.insertions, d.deletions), (2, 3, 0));
        assert_eq!(d.hash.len(), 64);
        // Without a base the change is the head commit's own.
        let alone = vcs.diff(dir.path(), None, &facts.head).unwrap();
        assert_eq!(alone, d);
    }

    #[test]
    fn the_diff_keeps_each_file_with_its_lines_and_kind() {
        let Some(dir) = repo() else { return };
        let vcs = GitVcs::default();
        let facts = vcs.head(dir.path()).unwrap().unwrap();
        let d = vcs
            .diff(dir.path(), facts.base.as_deref(), &facts.head)
            .unwrap();
        assert_eq!(
            d.changes,
            vec![
                DiffFile::text("a.txt", 2, 0).with_kind(ChangeKind::Modified),
                DiffFile::text("b.txt", 1, 0).with_kind(ChangeKind::Added),
            ]
        );
        // The totals are the list's totals.
        assert_eq!(d.files as usize, d.changes.len());
        assert_eq!((d.insertions, d.deletions), diff_totals(&d.changes));
    }

    #[test]
    fn changed_stats_count_committed_uncommitted_and_new_files() {
        let Some(dir) = repo() else { return };
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b2\nb3\n").unwrap();
        std::fs::write(p.join("new.txt"), "n1\nn2\nn3").unwrap();
        std::fs::write(p.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let files = GitVcs::default().changed_stats(p).unwrap();
        let find = |name: &str| files.iter().find(|f| f.path == name).unwrap().clone();
        // Committed on the branch.
        assert_eq!(
            find("a.txt"),
            DiffFile::text("a.txt", 2, 0).with_kind(ChangeKind::Modified)
        );
        // Committed, then changed again: counted against where the branch began.
        assert_eq!(
            find("b.txt"),
            DiffFile::text("b.txt", 2, 0).with_kind(ChangeKind::Added)
        );
        // New: lines counted by reading, the last line having no newline.
        assert_eq!(
            find("new.txt"),
            DiffFile::text("new.txt", 3, 0).with_kind(ChangeKind::Added)
        );
        let blob = find("blob.bin");
        assert!(blob.binary && blob.insertions == 0, "{blob:?}");
        // The names are the ones `changed` lists.
        let names: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(names, GitVcs::default().changed(p).unwrap());
        let outside = tempfile::tempdir().unwrap();
        assert!(GitVcs::default().changed_stats(outside.path()).is_err());
    }

    #[test]
    fn uncommitted_files_make_it_dirty_but_swarmux_files_do_not() {
        let Some(dir) = repo() else { return };
        let vcs = GitVcs::default();
        std::fs::create_dir_all(dir.path().join(".swarmux/missions")).unwrap();
        std::fs::write(dir.path().join(".swarmux/missions/x.toml"), "x").unwrap();
        assert!(!vcs.head(dir.path()).unwrap().unwrap().dirty);
        std::fs::write(dir.path().join("c.txt"), "new").unwrap();
        assert!(vcs.head(dir.path()).unwrap().unwrap().dirty);
    }

    #[test]
    fn outside_a_repository_there_is_no_head() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(GitVcs::default().head(dir.path()).unwrap(), None);
        let missing = GitVcs::new(dir.path().join("no-such-git"));
        assert!(missing.head(dir.path()).is_ok());
    }

    #[test]
    fn a_checkout_is_clean_and_goes_away() {
        let Some(dir) = repo() else { return };
        let vcs = GitVcs::default();
        let head = vcs.head(dir.path()).unwrap().unwrap().head;
        // Files the repository only has uncommitted or ignored stay out.
        std::fs::write(dir.path().join("scratch.txt"), "untracked").unwrap();
        let base = tempfile::tempdir().unwrap();
        let checkouts = GitCheckouts::default().under(base.path().to_path_buf());
        let held = checkouts.checkout(dir.path(), &head).unwrap();
        let path = held.path().to_path_buf();
        assert!(path.join("b.txt").is_file());
        assert!(!path.join("scratch.txt").exists());
        drop(held);
        assert!(!path.exists(), "removed on drop");
        let listed = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&listed.stdout).contains("swarmux-verify"));
    }

    #[test]
    fn an_unknown_commit_is_refused() {
        let Some(dir) = repo() else { return };
        let base = tempfile::tempdir().unwrap();
        let checkouts = GitCheckouts::default().under(base.path().to_path_buf());
        let bad = "0123456789abcdef0123456789abcdef01234567";
        let Err(e) = checkouts.checkout(dir.path(), bad) else {
            panic!("a commit that does not exist was checked out");
        };
        assert!(e.to_string().contains("cannot check out"), "{e}");
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }
}
