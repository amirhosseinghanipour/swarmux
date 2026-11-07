//! `WorkspaceScanner` adapter: finds the git repositories of a product
//! folder on disk and lists what detection needs of each.

use std::fs;
use std::path::{Path, PathBuf};

use swarmux_application::{AppResult, ScannedRepo, WorkspaceScanner};
use swarmux_domain::workspace::{MANIFESTS, RepoListing};

use crate::git::git_command;

/// Folders never searched for repositories or listed for detection:
/// dependencies and build output.
const SKIP: [&str; 12] = [
    "node_modules",
    "target",
    "build",
    "dist",
    "vendor",
    "Pods",
    "Library",
    "obj",
    "bin",
    "venv",
    "__pycache__",
    "DerivedData",
];
/// How deep a repository's files are listed for detection.
const LIST_DEPTH: usize = 3;
/// Most entries listed per repository.
const LIST_MAX: usize = 4_000;
/// Longest manifest read, in bytes.
const MANIFEST_MAX: u64 = 512 * 1024;

pub struct FsWorkspaceScanner {
    git: PathBuf,
}

impl FsWorkspaceScanner {
    pub fn new(git: PathBuf) -> FsWorkspaceScanner {
        FsWorkspaceScanner { git }
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let out = git_command(&self.git, dir).args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// `origin`'s HEAD, else the branch checked out (also before the
    /// first commit).
    fn default_branch(&self, repo: &Path) -> Option<String> {
        if let Some(remote) = self.git(
            repo,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        ) {
            return Some(
                remote
                    .strip_prefix("origin/")
                    .unwrap_or(&remote)
                    .to_string(),
            );
        }
        self.git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
    }
}

impl Default for FsWorkspaceScanner {
    fn default() -> FsWorkspaceScanner {
        FsWorkspaceScanner::new("git".into())
    }
}

/// A folder holding a git repository (`.git` is a folder, or a file for
/// worktrees and submodules).
fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

fn skipped(name: &str) -> bool {
    name.starts_with('.') || SKIP.contains(&name)
}

/// The folders in `dir`, by name, without hidden and skipped ones.
fn folders(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .filter(|(name, _)| !skipped(name))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The repositories at `root`, `root` itself first, as relative paths.
pub fn find_repos(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if is_repo(root) {
        out.push((".".to_string(), root.to_path_buf()));
    }
    for (name, dir) in folders(root) {
        if is_repo(&dir) {
            out.push((name, dir));
            continue;
        }
        for (inner, sub) in folders(&dir) {
            if is_repo(&sub) {
                out.push((format!("{name}/{inner}"), sub));
            }
        }
    }
    out
}

/// What detection reads of the repository at `root`.
pub fn listing(root: &Path) -> RepoListing {
    let mut files = Vec::new();
    walk(root, "", 0, &mut files);
    let texts = MANIFESTS
        .iter()
        .filter_map(|name| {
            let path = root.join(name);
            let small = fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MANIFEST_MAX);
            if !small {
                return None;
            }
            fs::read_to_string(&path)
                .ok()
                .map(|t| (name.to_string(), t))
        })
        .collect();
    RepoListing { files, texts }
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<String>) {
    if depth >= LIST_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= LIST_MAX {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let path = format!("{rel}{name}");
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if skipped(&name) {
                continue;
            }
            out.push(format!("{path}/"));
            walk(&e.path(), &format!("{path}/"), depth + 1, out);
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

impl WorkspaceScanner for FsWorkspaceScanner {
    fn scan(&self, root: &Path) -> AppResult<Vec<ScannedRepo>> {
        Ok(find_repos(root)
            .into_iter()
            .map(|(path, dir)| ScannedRepo {
                default_branch: self.default_branch(&dir),
                listing: listing(&dir),
                path,
                git_root: dir,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn repositories_are_found_two_levels_down_but_not_inside_one() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        repo(&root.join("app"));
        repo(&root.join("app").join("nested"));
        repo(&root.join("services").join("api"));
        repo(&root.join("node_modules").join("pkg"));
        repo(&root.join(".cache").join("x"));
        repo(&root.join("a").join("b").join("too-deep"));
        fs::create_dir_all(root.join("notes")).unwrap();
        let found: Vec<String> = find_repos(root).into_iter().map(|(p, _)| p).collect();
        assert_eq!(found, ["app", "services/api"]);
        repo(root);
        assert_eq!(find_repos(root)[0].0, ".");
    }

    #[test]
    fn the_listing_holds_folders_files_and_manifests() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("supabase").join("migrations")).unwrap();
        fs::create_dir_all(root.join("node_modules").join("x")).unwrap();
        fs::write(root.join("supabase").join("config.toml"), "").unwrap();
        fs::write(root.join("package.json"), r#"{"scripts":{"test":"x"}}"#).unwrap();
        let l = listing(root);
        assert!(l.files.contains(&"supabase/".to_string()), "{:?}", l.files);
        assert!(l.files.contains(&"supabase/config.toml".to_string()));
        assert!(!l.files.iter().any(|f| f.starts_with("node_modules")));
        assert_eq!(l.texts.len(), 1);
        assert_eq!(l.texts[0].0, "package.json");
    }
}
