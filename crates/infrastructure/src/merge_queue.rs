//! `MergeQueue` adapter backed by the `git` CLI.
//!
//! The integration branch of a mission gets a worktree of its own under
//! swarmux's folder; task branches are merged into it there with
//! `--no-ff`, and a conflicting merge is aborted before anything else
//! happens. Only swarmux's own branches are written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use swarmux_application::{AppError, AppResult, MergeOutcome, MergeQueue};
use swarmux_domain::ledger::{hex, sha256};

use crate::git::git_command;
use crate::paths::long;

/// Identity for merge commits when git has none configured.
const FALLBACK_NAME: &str = "swarmux";
const FALLBACK_EMAIL: &str = "swarmux@localhost";

pub struct GitMergeQueue {
    bin: PathBuf,
    /// Where integration worktrees are made.
    root: PathBuf,
}

impl GitMergeQueue {
    pub fn new(bin: PathBuf, root: PathBuf) -> GitMergeQueue {
        GitMergeQueue { bin, root }
    }

    /// Integration worktrees under `root`, with the `git` on the path.
    pub fn under(root: PathBuf) -> GitMergeQueue {
        GitMergeQueue::new("git".into(), root)
    }

    fn output(&self, cwd: &Path, args: &[&str]) -> AppResult<Output> {
        git_command(&self.bin, cwd)
            .args(["-c", "core.quotepath=off"])
            .args(args)
            .output()
            .map_err(|e| AppError::Workspace(format!("git: {e}")))
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> AppResult<String> {
        let out = self.output(cwd, args)?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
        Err(AppError::Workspace(format!(
            "git {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }

    fn succeeds(&self, cwd: &Path, args: &[&str]) -> bool {
        self.output(cwd, args).is_ok_and(|o| o.status.success())
    }

    fn identity(&self, cwd: &Path) -> Vec<String> {
        let has = |key: &str| {
            self.run(cwd, &["config", "--get", key])
                .is_ok_and(|v| !v.is_empty())
        };
        let mut args = vec!["-c".to_string(), "commit.gpgsign=false".to_string()];
        if !has("user.name") {
            args.extend(["-c".into(), format!("user.name={FALLBACK_NAME}")]);
        }
        if !has("user.email") {
            args.extend(["-c".into(), format!("user.email={FALLBACK_EMAIL}")]);
        }
        args
    }

    fn folder_for(&self, repo: &Path, branch: &str) -> PathBuf {
        let repo_text = repo.to_string_lossy().to_lowercase().replace('\\', "/");
        let digest = hex(&sha256(repo_text.as_bytes()));
        let name: String = branch
            .trim_start_matches("swarmux/")
            .replace('/', "-")
            .chars()
            .take(40)
            .collect();
        self.root.join(&digest[..8]).join(name)
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

impl MergeQueue for GitMergeQueue {
    fn open(&self, repo: &Path, branch: &str) -> AppResult<PathBuf> {
        let path = self.folder_for(repo, branch);
        let branch_ref = format!("refs/heads/{branch}");
        if !self.succeeds(repo, &["show-ref", "--verify", "--quiet", &branch_ref]) {
            self.run(repo, &["branch", branch, "HEAD"])?;
        }
        if !path.is_dir() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(long(parent))
                    .map_err(|e| AppError::Workspace(format!("{}: {e}", parent.display())))?;
            }
            // A registration whose folder vanished would block the branch.
            let _ = self.run(repo, &["worktree", "prune"]);
            let target = path.to_string_lossy().into_owned();
            self.run(repo, &["worktree", "add", &target, branch])?;
        }
        Ok(path)
    }

    fn changed(&self, workdir: &Path, branch: &str) -> AppResult<Vec<String>> {
        let range = format!("HEAD...{branch}");
        self.run(workdir, &["diff", "--name-only", "--no-renames", &range])
            .map(|t| lines(&t))
    }

    fn merge(&self, workdir: &Path, branch: &str, message: &str) -> AppResult<MergeOutcome> {
        let identity = self.identity(workdir);
        let mut args: Vec<&str> = identity.iter().map(String::as_str).collect();
        args.extend(["merge", "--no-ff", "--no-edit", "-m", message, branch]);
        let out = self.output(workdir, &args)?;
        if out.status.success() {
            let commit = self.run(workdir, &["rev-parse", "HEAD"])?;
            return Ok(MergeOutcome::Merged { commit });
        }
        let files = self
            .run(workdir, &["diff", "--name-only", "--diff-filter=U"])
            .map(|t| lines(&t))
            .unwrap_or_default();
        // Whatever went wrong, the integration branch is left as it was.
        let _ = self.run(workdir, &["merge", "--abort"]);
        if files.is_empty() {
            return Err(AppError::Workspace(format!(
                "git merge {branch}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(MergeOutcome::Conflict { files })
    }

    fn fast_forward(&self, worktree: &Path, branch: &str) -> AppResult<()> {
        self.run(worktree, &["merge", "--ff-only", branch])
            .map(drop)
    }
}
