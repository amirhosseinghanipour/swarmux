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
use swarmux_domain::verifier::{identity_key, repo_identity, repo_key};

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

/// What the repository `dir` lies in is, apart from where it lives: a hash
/// of its remote URL and its root commit. `None` while it has no commit.
pub(crate) fn identity_of(bin: &Path, dir: &Path) -> Option<String> {
    let roots = ok(bin, dir, &["rev-list", "--max-parents=0", "HEAD"])?;
    // A repository with several roots (merged histories) is known by the
    // smallest, so the choice does not depend on the order git lists them.
    let root = roots.lines().map(str::trim).min()?.to_string();
    let remote = ok(bin, dir, &["config", "--get", "remote.origin.url"])
        .or_else(|| {
            let first = ok(bin, dir, &["remote"])?
                .lines()
                .next()?
                .trim()
                .to_string();
            ok(
                bin,
                dir,
                &["config", "--get", &format!("remote.{first}.url")],
            )
        })
        .unwrap_or_default();
    Some(repo_identity(&remote, &root))
}

/// The key under which hidden checks of the repository `dir` lies in are
/// kept, as [`CleanCheckout::repo`] gives it.
pub fn holdout_key(bin: &Path, dir: &Path) -> Option<String> {
    GitCleanCheckout::new(bin.to_path_buf(), PathBuf::new()).repo(dir)
}

/// A path that stays inside the folder it is joined to: relative, no `..`.
fn inside(path: &str) -> AppResult<String> {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches("./");
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.as_bytes().get(1) == Some(&b':')
        || p.split('/').any(|s| s.is_empty() || s == "..");
    if bad {
        return Err(AppError::Workspace(format!(
            "{path:?} is not a path inside the copy"
        )));
    }
    Ok(p.to_string())
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
        let name = repo_key(name).unwrap_or_else(|| UNNAMED.to_string());
        // Known by what it is: its remote and its root commit, not only
        // the folder it lives in. A repository with no commit yet has no
        // identity and keeps the plain name.
        Some(match identity_of(&self.bin, dir) {
            Some(id) => identity_key(&name, &id),
            None => name,
        })
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
        // A file the base never had is removed; the others are checked
        // out. `--` and literal pathspecs keep a file name from being read
        // as an option or a pattern.
        let mut had = Vec::new();
        for p in paths {
            let p = inside(p)?;
            if self.show(base, &p)?.is_some() {
                had.push(p);
            } else {
                self.remove(&p)?;
            }
        }
        for chunk in had.chunks(100) {
            let mut args: Vec<String> = vec!["checkout".into(), base.into(), "--".into()];
            args.extend(chunk.iter().map(|p| format!(":(literal){p}")));
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            must(&self.bin, &self.path, &args, None)?;
        }
        Ok(())
    }

    fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    fn show(&self, commit: &str, path: &str) -> AppResult<Option<Vec<u8>>> {
        let path = inside(path)?;
        let spec = format!("{commit}:{path}");
        let out = run(
            &self.bin,
            &self.path,
            &["cat-file", "blob", spec.as_str()],
            None,
        )?;
        Ok(out.status.success().then_some(out.stdout))
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let p = inside(path).ok()?;
        fs::read(self.path.join(p)).ok()
    }

    fn write(&self, path: &str, bytes: &[u8]) -> AppResult<()> {
        let to = self.path.join(inside(path)?);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AppError::Workspace(format!("{}: {e}", parent.display())))?;
        }
        fs::write(&to, bytes).map_err(|e| AppError::Workspace(format!("{}: {e}", to.display())))
    }

    fn reset(&self) -> AppResult<()> {
        // The copy is a detached worktree at the snapshot commit: hard
        // reset puts back every tracked file, and `clean` removes what the
        // checks made (ignored files too: a check that leaves a cache
        // behind leaves none for the next).
        must(
            &self.bin,
            &self.path,
            &["reset", "--hard", "--quiet", self.commit.as_str()],
            None,
        )?;
        must(&self.bin, &self.path, &["clean", "-ffdxq"], None)?;
        Ok(())
    }

    fn files(&self) -> AppResult<Vec<String>> {
        // `-z` keeps odd file names whole. The copy was made from a commit,
        // so what git lists is what the copy holds.
        let out = run(&self.bin, &self.path, &["ls-files", "-z"], None)?;
        if !out.status.success() {
            return Err(AppError::Workspace(format!(
                "git ls-files: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect())
    }

    fn remove(&self, path: &str) -> AppResult<()> {
        let at = self.path.join(inside(path)?);
        match fs::remove_file(&at) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Workspace(format!("{}: {e}", at.display()))),
        }
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

    /// The key a repository `Shop` with no remote has: its folder name and
    /// what its root commit is.
    fn shop_key(p: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .output()
            .unwrap();
        let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
        identity_key("shop", &repo_identity("", &root))
    }

    #[test]
    fn a_copy_holds_uncommitted_work_but_nothing_ignored_and_goes_away() {
        let Some((dir, p)) = repo() else { return };
        let base = dir.path().join("verify");
        let checkouts = GitCleanCheckout::new("git".into(), base.clone());
        let head = checkouts.head(&p).expect("a head");
        let key = shop_key(&p);
        assert_eq!(checkouts.repo(&p).as_deref(), Some(key.as_str()));

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
        assert!(path.starts_with(base.join(&key)), "{}", path.display());
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
        assert_eq!(copy.repo(), key);
        let diff = copy.diff_from(Some(&head)).unwrap();
        assert!(diff.contains("-assert_eq!(f(), 1);"), "{diff}");
        assert!(diff.contains("+fn new() {}"), "{diff}");
        // Builds of the copies share one folder.
        let config = fs::read_to_string(base.join(&key).join(".cargo/config.toml")).unwrap();
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
        let key = shop_key(&p);
        assert_eq!(checkouts.repo(&wt).as_deref(), Some(key.as_str()));
        let copy = checkouts.snapshot(&wt).unwrap();
        assert_eq!(copy.repo(), key);
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

/// The operations the verifier needs of a copy besides making it: reading
/// what the base had, putting files back (and taking away what the base did
/// not have), starting over, and listing.
#[cfg(test)]
mod copy_operations {
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

    /// A repository with a test, a harness file and a source file.
    fn repo(name: &str) -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let p = dir.path().join(name);
        fs::create_dir_all(p.join("src")).ok()?;
        fs::create_dir_all(p.join("tests")).ok()?;
        if !git(&p, &["init", "-q", "-b", "main"]) {
            eprintln!("git not available; skipping");
            return None;
        }
        fs::write(p.join("src/lib.rs"), "pub fn f() -> i32 { 1 }\n").ok()?;
        fs::write(p.join("tests/a.rs"), "assert_eq!(f(), 1);\n").ok()?;
        fs::write(p.join("pytest.ini"), "[pytest]\n").ok()?;
        assert!(git(&p, &["add", "."]));
        assert!(git(&p, &["commit", "-q", "-m", "init"]));
        Some((dir, p))
    }

    #[test]
    fn a_copy_can_be_read_written_listed_and_put_back_as_the_base_had_it() {
        let Some((dir, p)) = repo("shop") else { return };
        let checkouts = GitCleanCheckout::new("git".into(), dir.path().join("verify"));
        let base = checkouts.head(&p).unwrap();
        // The agent edits a test and the harness, and plants two files.
        fs::write(p.join("tests/a.rs"), "assert_eq!(f(), 2);\n").unwrap();
        fs::write(p.join("pytest.ini"), "[pytest]\naddopts = -p evil\n").unwrap();
        fs::write(p.join("conftest.py"), "planted\n").unwrap();
        fs::create_dir_all(p.join(".cargo")).unwrap();
        fs::write(p.join(".cargo/config.toml"), "planted\n").unwrap();
        let copy = checkouts.snapshot(&p).unwrap();
        assert_eq!(copy.head(), Some(base.as_str()));

        // What the base had, and what the copy has now.
        let shown = |c: &str, path: &str| copy.show(c, path).unwrap();
        assert_eq!(shown(&base, "pytest.ini").unwrap(), b"[pytest]\n");
        assert_eq!(shown(&base, "conftest.py"), None, "the base never had it");
        assert!(
            String::from_utf8(copy.read("pytest.ini").unwrap())
                .unwrap()
                .contains("evil")
        );
        assert_eq!(copy.read("nope.txt"), None);
        let files = copy.files().unwrap();
        for f in [
            "src/lib.rs",
            "tests/a.rs",
            "conftest.py",
            ".cargo/config.toml",
            "pytest.ini",
        ] {
            assert!(files.iter().any(|x| x == f), "{f} in {files:?}");
        }

        // Put back: a file the base had is written as it was, one it did not
        // have is removed, the others stay as the agent left them.
        copy.restore(
            &base,
            &[
                "pytest.ini".into(),
                "tests/a.rs".into(),
                "conftest.py".into(),
                ".cargo/config.toml".into(),
            ],
        )
        .unwrap();
        assert_eq!(copy.read("pytest.ini").unwrap(), b"[pytest]\n");
        assert!(
            String::from_utf8(copy.read("tests/a.rs").unwrap())
                .unwrap()
                .contains("f(), 1")
        );
        assert_eq!(copy.read("conftest.py"), None);
        assert_eq!(copy.read(".cargo/config.toml"), None);
        // Nothing of the repository itself changed.
        assert!(p.join("conftest.py").is_file());
        assert!(
            fs::read_to_string(p.join("pytest.ini"))
                .unwrap()
                .contains("evil")
        );

        // Write and remove inside the copy only.
        copy.write("deep/er/new.txt", b"x").unwrap();
        assert_eq!(copy.read("deep/er/new.txt").unwrap(), b"x");
        copy.remove("deep/er/new.txt").unwrap();
        copy.remove("deep/er/new.txt").unwrap();
        assert_eq!(copy.read("deep/er/new.txt"), None);
        assert!(!p.join("deep").exists());

        // Starting over: what the checks wrote is gone, and every file is
        // as the work had it, the planted ones included.
        copy.write("src/lib.rs", b"rewritten by a build script\n")
            .unwrap();
        copy.write("left-by-a-test.txt", b"x").unwrap();
        copy.reset().unwrap();
        assert_eq!(
            copy.read("src/lib.rs").unwrap(),
            b"pub fn f() -> i32 { 1 }\n"
        );
        assert_eq!(copy.read("left-by-a-test.txt"), None);
        assert_eq!(copy.read("conftest.py").unwrap(), b"planted\n");
        assert!(
            String::from_utf8(copy.read("pytest.ini").unwrap())
                .unwrap()
                .contains("evil")
        );
    }

    #[test]
    fn paths_that_leave_the_copy_are_refused() {
        let Some((dir, p)) = repo("shop") else { return };
        let checkouts = GitCleanCheckout::new("git".into(), dir.path().join("verify"));
        let copy = checkouts.snapshot(&p).unwrap();
        for bad in ["../x", "a/../../x", "/etc/passwd", "C:/x", "", "a//b"] {
            assert!(copy.write(bad, b"x").is_err(), "{bad:?}");
            assert!(copy.remove(bad).is_err(), "{bad:?}");
            assert!(copy.show("HEAD", bad).is_err(), "{bad:?}");
            assert_eq!(copy.read(bad), None, "{bad:?}");
        }
        assert!(copy.restore("HEAD", &["../x".into()]).is_err());
    }

    #[test]
    fn two_repositories_in_folders_of_one_name_have_two_keys_and_a_worktree_shares_its_own() {
        let Some((d1, one)) = repo("shop") else {
            return;
        };
        let Some((_d2, two)) = repo("shop") else {
            return;
        };
        // Make the second one a different repository (another first commit).
        fs::write(two.join("other.txt"), "x").unwrap();
        assert!(git(&two, &["add", "."]));
        assert!(git(
            &two,
            &["commit", "-q", "--amend", "-m", "another start"]
        ));
        let checkouts = GitCleanCheckout::new("git".into(), d1.path().join("verify"));
        let (k1, k2) = (checkouts.repo(&one).unwrap(), checkouts.repo(&two).unwrap());
        assert!(
            k1.starts_with("shop.id-") && k2.starts_with("shop.id-"),
            "{k1} {k2}"
        );
        assert_ne!(k1, k2, "the same folder name, not the same repository");

        // A remote is part of what a repository is; moving the folder is not.
        assert!(git(
            &one,
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/acme/shop.git"
            ]
        ));
        let with_remote = checkouts.repo(&one).unwrap();
        assert_ne!(with_remote, k1);
        assert_eq!(
            holdout_key(Path::new("git"), &one).as_deref(),
            Some(with_remote.as_str())
        );
        let moved = d1.path().join("renamed-checkout");
        fs::rename(&one, &moved).unwrap();
        let after = checkouts.repo(&moved).unwrap();
        assert_eq!(
            after.split_once(".id-").map(|(_, id)| id),
            with_remote.split_once(".id-").map(|(_, id)| id),
            "the identity does not depend on where it lives"
        );
        assert!(after.starts_with("renamed-checkout.id-"), "{after}");

        // A worktree of a repository is the repository.
        let wt = d1.path().join("job-7");
        assert!(git(
            &moved,
            &["worktree", "add", "-q", "-b", "job", wt.to_str().unwrap()]
        ));
        assert_eq!(checkouts.repo(&wt), Some(after));
    }
}
