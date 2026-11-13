//! `CleanCheckout` adapter: clean copies of work in progress, made with git
//! in a folder swarmux owns.
//!
//! What the worktree holds, committed or not, is written into a commit of
//! its own through a throwaway index, so neither the worktree nor its index
//! nor its branch is touched. That commit is checked out as a detached
//! worktree under `<base>/<repo>/`; ignored files never get there. Copies of
//! one repository share a build folder: a `.cargo/config.toml` above them
//! points cargo at `<base>/<repo>/target`.
//!
//! git is started directly, with no shell and no console window.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicU32, Ordering};

use swarmux_application::{AppError, AppResult, CleanCheckout, CleanCopy};
use swarmux_domain::verifier::repo_key;

use crate::git::git_command;
use crate::holdout::quoted;
use crate::worktree::remove_dir_retrying;

/// Longest diff read for the tamper scan, in bytes.
const DIFF_MAX: usize = 8 << 20;
/// The name a repository without a usable folder name is kept under.
const UNNAMED: &str = "repo";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Makes clean copies with the `git` CLI.
pub struct GitCleanCheckout {
    bin: PathBuf,
    base: PathBuf,
}

impl GitCleanCheckout {
    /// Copies go below `base` (swarmux keeps it at `~/.swarmux/verify`).
    pub fn new(bin: PathBuf, base: PathBuf) -> GitCleanCheckout {
        GitCleanCheckout { bin, base }
    }
}

fn run(bin: &Path, cwd: &Path, args: &[&str], index: Option<&Path>) -> AppResult<Output> {
    let mut cmd = git_command(bin, cwd);
    cmd.args(["-c", "core.longpaths=true", "-c", "core.quotepath=off"])
        .args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    cmd.output()
        .map_err(|e| AppError::Workspace(format!("git: {e}")))
}

/// Trimmed stdout of a git command that succeeded.
fn ok(bin: &Path, cwd: &Path, args: &[&str]) -> Option<String> {
    let out = run(bin, cwd, args, None).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Stdout of a git command that must succeed, or why it failed.
fn must(bin: &Path, cwd: &Path, args: &[&str], index: Option<&Path>) -> AppResult<String> {
    let out = run(bin, cwd, args, index)?;
    if !out.status.success() {
        return Err(AppError::Workspace(format!(
            "git {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl GitCleanCheckout {
    /// Points cargo builds of the repository's copies at one shared
    /// folder, unless that was done already.
    fn share_target(&self, folder: &Path) {
        let config = folder.join(".cargo").join("config.toml");
        if config.exists() {
            return;
        }
        let target = folder.join("target").to_string_lossy().replace('\\', "/");
        let text = format!(
            "# Written by swarmux: clean copies of this repository share one build folder.\n[build]\ntarget-dir = {}\n",
            quoted(&target)
        );
        // Without it each copy builds from scratch: slower, still right.
        let _ = fs::create_dir_all(folder.join(".cargo")).and_then(|()| fs::write(&config, text));
    }

    /// A commit holding everything the worktree at `top` holds now,
    /// committed or not, apart from what git ignores.
    fn snapshot_commit(&self, top: &Path, folder: &Path, head: Option<&str>) -> AppResult<String> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let index = folder.join(format!("index-{}-{n}", std::process::id()));
        // Starting from the real index keeps git from hashing every file
        // again; it is copied, never written. The copy keeps the original's
        // modification time: git trusts a file whose size and time match its
        // index entry unless the entry is as new as the index file ("racily
        // clean"), and a copy stamped with the time of copying would make
        // every such entry look old, so a same-size edit made in the same
        // clock tick as the last commit went missing from the snapshot.
        if let Some(real) = ok(
            &self.bin,
            top,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        ) && fs::copy(&real, &index).is_ok()
        {
            let stamp = fs::metadata(&real).and_then(|m| m.modified());
            if let Ok(at) = stamp {
                let _ = fs::OpenOptions::new()
                    .write(true)
                    .open(&index)
                    .and_then(|f| f.set_modified(at));
            }
        }
        let result = (|| {
            must(&self.bin, top, &["add", "-A", "--", "."], Some(&index))?;
            let tree = must(&self.bin, top, &["write-tree"], Some(&index))?;
            let mut args = vec![
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=swarmux",
                "-c",
                "user.email=swarmux@localhost",
                "commit-tree",
                tree.as_str(),
                "-m",
                "swarmux: clean copy for verification",
            ];
            if let Some(h) = head {
                args.extend(["-p", h]);
            }
            must(&self.bin, top, &args, None)
        })();
        let _ = fs::remove_file(&index);
        result
    }
}

impl CleanCheckout for GitCleanCheckout {
    fn head(&self, dir: &Path) -> Option<String> {
        ok(
            &self.bin,
            dir,
            &["rev-parse", "--verify", "--quiet", "HEAD"],
        )
    }

    fn repo(&self, dir: &Path) -> Option<String> {
        let common = ok(
            &self.bin,
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let common = PathBuf::from(common);
        // `<main>/.git` for a checkout and its worktrees; a bare or
        // separate git folder names itself.
        let main = if common.file_name().is_some_and(|n| n == ".git") {
            common.parent().map(Path::to_path_buf).unwrap_or(common)
        } else {
            common
        };
        let name = main.to_string_lossy();
        let name = name.trim_end_matches(".git");
        Some(repo_key(name).unwrap_or_else(|| UNNAMED.to_string()))
    }

    fn snapshot(&self, dir: &Path) -> AppResult<Box<dyn CleanCopy>> {
        let top = ok(&self.bin, dir, &["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
            .ok_or_else(|| {
                AppError::Workspace(format!("{} is not in a git repository", dir.display()))
            })?;
        let repo = self.repo(&top).unwrap_or_else(|| UNNAMED.to_string());
        let folder = self.base.join(&repo);
        fs::create_dir_all(&folder)
            .map_err(|e| AppError::Workspace(format!("cannot create {}: {e}", folder.display())))?;
        self.share_target(&folder);
        let head = self.head(&top);
        let commit = self.snapshot_commit(&top, &folder, head.as_deref())?;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = folder.join(format!("{}-{n}", std::process::id()));
        let path_text = path.to_string_lossy().into_owned();
        must(
            &self.bin,
            &top,
            &[
                "worktree",
                "add",
                "--detach",
                "--force",
                path_text.as_str(),
                commit.as_str(),
            ],
            None,
        )?;
        Ok(Box::new(GitCleanCopy {
            bin: self.bin.clone(),
            top,
            path,
            repo,
            commit,
            head,
        }))
    }

    fn root(&self) -> PathBuf {
        self.base.clone()
    }
}

/// One copy; removed with its worktree entry when dropped.
struct GitCleanCopy {
    bin: PathBuf,
    /// The repository the copy was made from.
    top: PathBuf,
    path: PathBuf,
    repo: String,
    /// The snapshot commit the copy is at.
    commit: String,
    /// The worktree's own commit when the copy was made.
    head: Option<String>,
}

impl CleanCopy for GitCleanCopy {
    fn path(&self) -> &Path {
        &self.path
    }

    fn repo(&self) -> &str {
        &self.repo
    }

    fn diff_from(&self, base: Option<&str>) -> AppResult<String> {
        let from = match base {
            Some(b) => b.to_string(),
            // Without a known start only the work not yet committed is
            // looked at: commits on the branch may be the user's own.
            None => self
                .head
                .clone()
                .ok_or_else(|| AppError::Workspace("no commit to compare with".into()))?,
        };
        let out = run(
            &self.bin,
            &self.top,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--find-renames",
                from.as_str(),
                self.commit.as_str(),
            ],
            None,
        )?;
        if !out.status.success() {
            return Err(AppError::Workspace(format!(
                "git diff: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let mut bytes = out.stdout;
        bytes.truncate(DIFF_MAX);
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn restore(&self, base: &str, paths: &[String]) -> AppResult<()> {
        // The copy is a worktree of the repository, so `base` is there.
        // `--` and literal pathspecs keep a file name from being read as
        // an option or a pattern.
        for chunk in paths.chunks(100) {
            let mut args: Vec<String> = vec!["checkout".into(), base.into(), "--".into()];
            args.extend(chunk.iter().map(|p| format!(":(literal){p}")));
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            must(&self.bin, &self.path, &args, None)?;
        }
        Ok(())
    }
}

impl Drop for GitCleanCopy {
    fn drop(&mut self) {
        let removed = git_command(&self.bin, &self.top)
            .args(["worktree", "remove", "--force", "--force"])
            .arg(&self.path)
            .output()
            .is_ok_and(|o| o.status.success());
        if !removed {
            let _ = remove_dir_retrying(&self.path);
            let _ = git_command(&self.bin, &self.top)
                .args(["worktree", "prune"])
                .output();
        }
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

    /// A repository `Shop` with one commit and an ignored folder.
    fn repo() -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let p = dir.path().join("Shop");
        fs::create_dir_all(&p).ok()?;
        if !git(&p, &["init", "-q", "-b", "main"]) {
            eprintln!("git not available; skipping");
            return None;
        }
        fs::write(p.join(".gitignore"), "target/\n").ok()?;
        fs::create_dir_all(p.join("tests")).ok()?;
        fs::write(p.join("tests/a.rs"), "assert_eq!(f(), 1);\n").ok()?;
        assert!(git(&p, &["add", "."]));
        assert!(git(&p, &["commit", "-q", "-m", "init"]));
        Some((dir, p))
    }

    #[test]
    fn a_copy_holds_uncommitted_work_but_nothing_ignored_and_goes_away() {
        let Some((dir, p)) = repo() else { return };
        let base = dir.path().join("verify");
        let checkouts = GitCleanCheckout::new("git".into(), base.clone());
        let head = checkouts.head(&p).expect("a head");
        assert_eq!(checkouts.repo(&p).as_deref(), Some("shop"));

        fs::write(p.join("new.rs"), "fn new() {}\n").unwrap();
        fs::write(p.join("tests/a.rs"), "assert_eq!(f(), 2);\n").unwrap();
        fs::create_dir_all(p.join("target")).unwrap();
        fs::write(p.join("target/planted.txt"), "ok").unwrap();
        let status_before = Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["status", "--porcelain"])
            .output()
            .unwrap()
            .stdout;

        let copy = checkouts.snapshot(&p).unwrap();
        let path = copy.path().to_path_buf();
        assert!(path.starts_with(base.join("shop")), "{}", path.display());
        // Line endings are what a fresh checkout gives (core.autocrlf).
        let new = fs::read_to_string(path.join("new.rs")).unwrap();
        assert_eq!(new.trim_end(), "fn new() {}");
        assert!(
            fs::read_to_string(path.join("tests/a.rs"))
                .unwrap()
                .contains("2)"),
            "uncommitted edits are in the copy"
        );
        assert!(
            !path.join("target/planted.txt").exists(),
            "ignored files stay out"
        );
        assert_eq!(copy.repo(), "shop");
        let diff = copy.diff_from(Some(&head)).unwrap();
        assert!(diff.contains("-assert_eq!(f(), 1);"), "{diff}");
        assert!(diff.contains("+fn new() {}"), "{diff}");
        // Builds of the copies share one folder.
        let config = fs::read_to_string(base.join("shop/.cargo/config.toml")).unwrap();
        assert!(config.contains("target-dir"), "{config}");

        // The repository itself was not touched.
        let status_after = Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["status", "--porcelain"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(status_before, status_after);
        assert_eq!(checkouts.head(&p).as_deref(), Some(head.as_str()));

        drop(copy);
        assert!(!path.exists(), "removed on drop");
        let listed = Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&listed.stdout).contains("verify"));
    }

    #[test]
    fn a_worktree_is_named_after_its_main_repository() {
        let Some((dir, p)) = repo() else { return };
        let wt = dir.path().join("elsewhere-7");
        assert!(git(
            &p,
            &["worktree", "add", "-q", "-b", "job", wt.to_str().unwrap()]
        ));
        let checkouts = GitCleanCheckout::new("git".into(), dir.path().join("verify"));
        assert_eq!(checkouts.repo(&wt).as_deref(), Some("shop"));
        let copy = checkouts.snapshot(&wt).unwrap();
        assert_eq!(copy.repo(), "shop");
        assert!(copy.path().join("tests/a.rs").is_file());
    }

    #[test]
    fn outside_git_there_is_nothing_to_copy() {
        let dir = tempfile::tempdir().unwrap();
        let checkouts = GitCleanCheckout::new("git".into(), dir.path().join("verify"));
        assert_eq!(checkouts.head(dir.path()), None);
        assert_eq!(checkouts.repo(dir.path()), None);
        assert!(checkouts.snapshot(dir.path()).is_err());
    }
}
