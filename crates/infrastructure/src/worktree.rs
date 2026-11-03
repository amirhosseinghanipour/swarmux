//! `Worktrees` adapter backed by the `git` CLI.

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::Output;
use std::thread::sleep;
use std::time::Duration;

use swarmux_application::{AddWorktree, Added, AppError, AppResult, Removed, Worktrees};
use swarmux_domain::Handoff;

use crate::git::git_command;
use crate::paths::long;

/// Where swarmux keeps its files inside a worktree's own git directory.
const META_DIR: &str = "swarmux";
/// Paths copied into the worktree, one per line; snapshots leave them out.
const COPIED_FILE: &str = "copied.txt";
const HANDOFF_FILE: &str = "handoff.json";
/// Tries for operations that git or Windows may briefly refuse.
const TRIES: u32 = 6;
/// Identity for snapshot commits when git has none configured.
const FALLBACK_NAME: &str = "swarmux";
const FALLBACK_EMAIL: &str = "swarmux@localhost";

pub struct GitWorktrees {
    bin: PathBuf,
}

impl GitWorktrees {
    pub fn new(bin: PathBuf) -> GitWorktrees {
        GitWorktrees { bin }
    }

    fn output(&self, cwd: &Path, args: &[&str]) -> AppResult<Output> {
        git_command(&self.bin, cwd)
            .args(["-c", "core.quotepath=off"])
            .args(args)
            .output()
            .map_err(|e| AppError::Workspace(format!("git: {e}")))
    }

    /// Runs git and returns its trimmed stdout; a failure carries stderr.
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

    /// Like [`GitWorktrees::run`], and tries again while another git process
    /// holds a lock.
    fn run_locked(&self, cwd: &Path, args: &[&str]) -> AppResult<String> {
        let mut last = None;
        for attempt in 0..TRIES {
            match self.run(cwd, args) {
                Ok(text) => return Ok(text),
                Err(AppError::Workspace(e)) if e.contains(".lock") || e.contains("lock file") => {
                    last = Some(AppError::Workspace(e));
                    sleep(Duration::from_millis(60 * u64::from(attempt + 1)));
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| AppError::Workspace("git stayed locked".into())))
    }

    fn succeeds(&self, cwd: &Path, args: &[&str]) -> bool {
        self.output(cwd, args).is_ok_and(|o| o.status.success())
    }

    fn git_dir(&self, worktree: &Path) -> AppResult<PathBuf> {
        self.run(worktree, &["rev-parse", "--absolute-git-dir"])
            .map(PathBuf::from)
    }

    fn meta_dir(&self, worktree: &Path) -> AppResult<PathBuf> {
        let dir = self.git_dir(worktree)?.join(META_DIR);
        fs::create_dir_all(&dir)
            .map_err(|e| AppError::Workspace(format!("{}: {e}", dir.display())))?;
        Ok(dir)
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

    fn copied(&self, worktree: &Path) -> Vec<String> {
        let Ok(dir) = self.git_dir(worktree) else {
            return Vec::new();
        };
        fs::read_to_string(dir.join(META_DIR).join(COPIED_FILE))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn dirty(&self, worktree: &Path) -> AppResult<bool> {
        Ok(!self.run(worktree, &["status", "--porcelain"])?.is_empty())
    }

    fn head(&self, worktree: &Path) -> AppResult<String> {
        self.run(worktree, &["rev-parse", "HEAD"])
    }
}

impl Default for GitWorktrees {
    fn default() -> GitWorktrees {
        GitWorktrees::new("git".into())
    }
}

impl Worktrees for GitWorktrees {
    fn repository(&self, dir: &Path) -> AppResult<Option<PathBuf>> {
        let out = match git_command(&self.bin, dir)
            .args(["rev-parse", "--show-toplevel"])
            .output()
        {
            Ok(out) => out,
            // No git installed: same as no repository.
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AppError::Workspace(format!("git: {e}"))),
        };
        if !out.status.success() {
            return Ok(None);
        }
        let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((!top.is_empty()).then(|| PathBuf::from(top)))
    }

    fn add(&self, req: &AddWorktree) -> AppResult<Added> {
        if let Some(parent) = req.path.parent() {
            fs::create_dir_all(long(parent))
                .map_err(|e| AppError::Workspace(format!("{}: {e}", parent.display())))?;
        }
        // Registrations of folders that vanished would block their branches.
        let _ = self.run_locked(&req.repo, &["worktree", "prune"]);
        let path = req.path.to_string_lossy().into_owned();
        let branch_ref = format!("refs/heads/{}", req.branch);
        let exists = self.succeeds(&req.repo, &["show-ref", "--verify", "--quiet", &branch_ref]);
        if exists {
            self.run_locked(&req.repo, &["worktree", "add", &path, &req.branch])?;
        } else {
            self.run_locked(&req.repo, &["worktree", "add", "-b", &req.branch, &path])?;
        }
        let copied = copy_listed(&req.repo, &req.path, &req.copy);
        if !copied.is_empty() {
            let meta = self.meta_dir(&req.path)?;
            let mut known = self.copied(&req.path);
            known.extend(copied.iter().cloned());
            known.sort();
            known.dedup();
            fs::write(long(&meta.join(COPIED_FILE)), known.join("\n"))
                .map_err(|e| AppError::Workspace(format!("{}: {e}", meta.display())))?;
        }
        Ok(Added {
            created_branch: !exists,
            copied,
        })
    }

    fn snapshot(&self, path: &Path, message: &str) -> AppResult<String> {
        if !self.dirty(path)? {
            return self.head(path);
        }
        self.run_locked(path, &["add", "-A", "--", "."])?;
        // Files swarmux copied in stay out, even when git does not ignore them.
        let copied = self.copied(path);
        if !copied.is_empty() {
            let mut reset: Vec<String> = vec!["reset".into(), "--quiet".into(), "--".into()];
            reset.extend(copied.iter().map(|p| format!(":(literal){p}")));
            let reset_refs: Vec<&str> = reset.iter().map(String::as_str).collect();
            self.run_locked(path, &reset_refs)?;
        }
        if self.succeeds(path, &["diff", "--cached", "--quiet"]) {
            return self.head(path);
        }
        let identity = self.identity(path);
        let mut args: Vec<&str> = identity.iter().map(String::as_str).collect();
        args.extend(["commit", "--no-verify", "--quiet", "-m", message]);
        self.run_locked(path, &args)?;
        self.head(path)
    }

    fn remove(&self, repo: &Path, path: &Path, branch: &str) -> AppResult<Removed> {
        let mut removed = Removed::default();
        if path.is_dir() {
            // Nothing is thrown away: the files are committed first, and if
            // that cannot be done the folder stays.
            if self.dirty(path).unwrap_or(true) {
                removed.snapshot =
                    Some(self.snapshot(path, "swarmux: files left in a reclaimed worktree")?);
            }
            let target = path.to_string_lossy().into_owned();
            let mut gone = false;
            for attempt in 0..TRIES {
                if self
                    .run(repo, &["worktree", "remove", "--force", &target])
                    .is_ok()
                    || !path.exists()
                {
                    gone = true;
                    break;
                }
                // Windows keeps folders locked for a moment after a process
                // that ran inside them exits.
                sleep(Duration::from_millis(200 * u64::from(attempt + 1)));
            }
            if !gone {
                remove_dir_retrying(path)?;
            }
        }
        let _ = self.run_locked(repo, &["worktree", "prune"]);
        removed.branch_deleted = self.succeeds(repo, &["branch", "-d", branch]);
        Ok(removed)
    }

    fn present(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn write_handoff(&self, path: &Path, handoff: &Handoff) -> AppResult<PathBuf> {
        let file = self.meta_dir(path)?.join(HANDOFF_FILE);
        fs::write(long(&file), handoff.to_json())
            .map_err(|e| AppError::Workspace(format!("{}: {e}", file.display())))?;
        Ok(file)
    }
}

pub(crate) fn remove_dir_retrying(path: &Path) -> AppResult<()> {
    let mut last = None;
    for attempt in 0..TRIES {
        match fs::remove_dir_all(long(path)) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                // Read-only files (git objects) block deletion on Windows.
                clear_readonly(path);
                last = Some(e);
                sleep(Duration::from_millis(150 * u64::from(attempt + 1)));
            }
        }
    }
    Err(AppError::Workspace(format!(
        "cannot remove {}: {}",
        path.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

fn clear_readonly(dir: &Path) {
    let Ok(entries) = fs::read_dir(long(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(meta) = fs::symlink_metadata(long(&path)) {
            if meta.is_dir() {
                clear_readonly(&path);
            } else if meta.permissions().readonly() {
                let mut perms = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(long(&path), perms);
            }
        }
    }
}

/// A relative path that stays inside the repository.
fn safe_relative(rel: &str) -> Option<PathBuf> {
    let path = Path::new(rel.trim());
    if rel.trim().is_empty() || path.is_absolute() {
        return None;
    }
    path.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        .then(|| path.to_path_buf())
}

/// `*` and `?` in a file name.
fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

/// Copies what exists and is missing in `to`; returns the relative paths
/// (with `/`) that were copied. Links are never followed.
fn copy_listed(from: &Path, to: &Path, listed: &[String]) -> Vec<String> {
    let mut copied = Vec::new();
    for entry in listed {
        let Some(rel) = safe_relative(entry) else {
            continue;
        };
        let name = rel.file_name().map(|n| n.to_string_lossy().into_owned());
        let parent = rel.parent().map(Path::to_path_buf).unwrap_or_default();
        match name {
            Some(name) if name.contains(['*', '?']) => {
                let Ok(dir) = fs::read_dir(long(&from.join(&parent))) else {
                    continue;
                };
                let mut names: Vec<String> = dir
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n != ".git" && wildcard(&name, n))
                    .collect();
                names.sort();
                for n in names {
                    copy_one(from, to, &parent.join(n), &mut copied);
                }
            }
            _ => copy_one(from, to, &rel, &mut copied),
        }
    }
    copied
}

fn copy_one(from: &Path, to: &Path, rel: &Path, copied: &mut Vec<String>) {
    let src = from.join(rel);
    let Ok(meta) = fs::symlink_metadata(long(&src)) else {
        return;
    };
    let dst = to.join(rel);
    let rel_text = rel.to_string_lossy().replace('\\', "/");
    if meta.is_file() {
        if long(&dst).exists() {
            return;
        }
        if let Some(dir) = dst.parent() {
            let _ = fs::create_dir_all(long(dir));
        }
        if fs::copy(long(&src), long(&dst)).is_ok() {
            copied.push(rel_text);
        }
    } else if meta.is_dir() {
        let Ok(entries) = fs::read_dir(long(&src)) else {
            return;
        };
        for e in entries.flatten() {
            if e.file_name() != ".git" {
                copy_one(from, to, &rel.join(e.file_name()), copied);
            }
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
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// A repository with one commit, or `None` without git.
    fn repo() -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        if !git(&root, &["init", "-q", "-b", "main"]) {
            eprintln!("git not available; skipping");
            return None;
        }
        fs::write(root.join("a.txt"), "one\n").unwrap();
        fs::write(root.join(".gitignore"), ".env\n").unwrap();
        assert!(git(&root, &["add", "."]));
        assert!(git(&root, &["commit", "-q", "-m", "init"]));
        Some((dir, root))
    }

    fn request(root: &Path, dir: &Path, n: u32, copy: &[&str]) -> AddWorktree {
        AddWorktree {
            repo: root.to_path_buf(),
            path: dir.join("wt").join(format!("job-{n}")),
            branch: format!("swarmux/demo/{n}"),
            copy: copy.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn repository_is_found_from_a_subfolder_and_missing_elsewhere() {
        let Some((dir, root)) = repo() else { return };
        let git = GitWorktrees::default();
        fs::create_dir_all(root.join("sub")).unwrap();
        let top = git.repository(&root.join("sub")).unwrap().unwrap();
        assert_eq!(top.canonicalize().unwrap(), root.canonicalize().unwrap());
        let outside = tempfile::tempdir().unwrap();
        assert!(git.repository(outside.path()).unwrap().is_none());
        assert!(
            git.repository(&dir.path().join("missing"))
                .unwrap()
                .is_none()
        );
        let nogit = GitWorktrees::new(dir.path().join("no-such-git"));
        assert!(nogit.repository(&root).unwrap().is_none());
    }

    #[test]
    fn add_creates_the_branch_and_copies_the_listed_files() {
        let Some((dir, root)) = repo() else { return };
        fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        fs::create_dir_all(root.join("cfg")).unwrap();
        fs::write(root.join("cfg").join("x.local.json"), "{}").unwrap();
        fs::write(root.join("cfg").join("y.local.json"), "{}").unwrap();
        fs::write(root.join("cfg").join("keep.json"), "{}").unwrap();
        let git = GitWorktrees::default();
        let req = request(
            &root,
            dir.path(),
            1,
            &[".env", "cfg/*.local.json", "nothing", "../escape", "a.txt"],
        );
        let added = git.add(&req).unwrap();
        assert!(added.created_branch);
        assert_eq!(
            added.copied,
            [".env", "cfg/x.local.json", "cfg/y.local.json"],
            "a.txt is tracked, the rest is missing or unsafe"
        );
        assert_eq!(
            fs::read_to_string(req.path.join(".env")).unwrap(),
            "SECRET=1\n"
        );
        assert!(!req.path.join("cfg").join("keep.json").exists());
        assert!(req.path.join("a.txt").exists());
        let branch = git.run(&req.path, &["branch", "--show-current"]).unwrap();
        assert_eq!(branch, "swarmux/demo/1");
    }

    #[test]
    fn adding_again_reuses_the_branch() {
        let Some((dir, root)) = repo() else { return };
        let git = GitWorktrees::default();
        let req = request(&root, dir.path(), 2, &[]);
        git.add(&req).unwrap();
        fs::write(req.path.join("new.txt"), "x").unwrap();
        git.snapshot(&req.path, "save").unwrap();
        // The folder is lost without git knowing: the branch keeps the work.
        remove_dir_retrying(&req.path).unwrap();
        let again = git.add(&req).unwrap();
        assert!(!again.created_branch);
        assert!(req.path.join("new.txt").exists());
    }

    #[test]
    fn snapshots_commit_changes_but_never_copied_files() {
        let Some((dir, root)) = repo() else { return };
        // Not ignored here, so only the copied list keeps it out.
        fs::write(root.join("local.cfg"), "token\n").unwrap();
        let git = GitWorktrees::default();
        let req = request(&root, dir.path(), 3, &["local.cfg"]);
        git.add(&req).unwrap();
        let head = git.head(&req.path).unwrap();
        // Nothing changed: the head is the checkpoint.
        assert_eq!(git.snapshot(&req.path, "none").unwrap(), head);

        fs::write(req.path.join("work.txt"), "done\n").unwrap();
        let first = git.snapshot(&req.path, "checkpoint").unwrap();
        assert_ne!(first, head);
        let files = git.run(&req.path, &["ls-tree", "-r", "--name-only", "HEAD"]);
        let files = files.unwrap();
        assert!(files.contains("work.txt"), "{files}");
        assert!(!files.contains("local.cfg"), "{files}");
        assert_eq!(git.snapshot(&req.path, "again").unwrap(), first);
    }

    #[test]
    fn handoff_lives_in_the_worktrees_git_dir() {
        let Some((dir, root)) = repo() else { return };
        let git = GitWorktrees::default();
        let req = request(&root, dir.path(), 4, &[]);
        git.add(&req).unwrap();
        let handoff = Handoff {
            goal: "g".into(),
            ..Handoff::default()
        };
        let file = git.write_handoff(&req.path, &handoff).unwrap();
        assert!(file.is_file());
        assert!(!file.starts_with(&req.path), "not inside the work tree");
        assert!(fs::read_to_string(&file).unwrap().contains(r#""goal":"g""#));
        // It never shows up as a change.
        assert!(!git.dirty(&req.path).unwrap());
    }

    #[test]
    fn remove_saves_files_then_drops_the_folder_and_keeps_unmerged_work() {
        let Some((dir, root)) = repo() else { return };
        let git = GitWorktrees::default();
        let req = request(&root, dir.path(), 5, &[]);
        git.add(&req).unwrap();
        fs::write(req.path.join("late.txt"), "not yet committed").unwrap();
        let removed = git.remove(&root, &req.path, &req.branch).unwrap();
        assert!(!req.path.exists());
        let saved = removed.snapshot.expect("files were saved");
        assert!(!removed.branch_deleted, "the branch holds unmerged work");
        let tree = git.run(&root, &["ls-tree", "-r", "--name-only", &saved]);
        assert!(tree.unwrap().contains("late.txt"));

        // An untouched, merged branch goes away with its worktree.
        let clean = request(&root, dir.path(), 6, &[]);
        git.add(&clean).unwrap();
        let removed = git.remove(&root, &clean.path, &clean.branch).unwrap();
        assert!(removed.snapshot.is_none());
        assert!(removed.branch_deleted);
        assert!(!git.succeeds(
            &root,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/swarmux/demo/6"
            ]
        ));
    }

    #[test]
    fn unsafe_paths_and_wildcards() {
        assert!(safe_relative(".env").is_some());
        assert!(safe_relative(".claude/settings.local.json").is_some());
        assert!(safe_relative("../x").is_none());
        assert!(safe_relative("a/../../x").is_none());
        assert!(safe_relative("").is_none());
        assert!(safe_relative(if cfg!(windows) { "C:\\x" } else { "/x" }).is_none());
        assert!(wildcard("*.local.json", "a.local.json"));
        assert!(wildcard(".env.*", ".env.test"));
        assert!(!wildcard(".env.*", ".env"));
        assert!(wildcard("?.txt", "a.txt"));
    }
}
