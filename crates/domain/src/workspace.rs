//! Workspaces: a product folder that holds several git repositories (an
//! app, a backend, an admin site, ...) worked on as one.
//!
//! The folder itself is often not a repository. A workspace session runs
//! from its root and sees every repository; a workspace job gets one
//! worktree per repository it touches, laid out like the workspace.
//!
//! Each repository is named within its workspace, so [`RepoRef`] (the
//! workspace id and the repository's name) stays the same across rescans:
//! what later refers to a repository (knowledge, contracts) keys on it.

use std::fmt;
use std::path::PathBuf;

use uuid::Uuid;

use crate::Millis;

mod detect;

pub use detect::{Detected, MANIFESTS, RepoListing, detect};

/// Longest workspace name, in characters.
const NAME_MAX: usize = 64;

/// Identifies a workspace for good, whatever it is renamed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub Uuid);

impl WorkspaceId {
    pub fn random() -> WorkspaceId {
        WorkspaceId(Uuid::new_v4())
    }

    pub fn parse(s: &str) -> Option<WorkspaceId> {
        Uuid::parse_str(s.trim()).ok().map(WorkspaceId)
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One repository of a workspace, for whatever refers to it later.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoRef {
    pub workspace: WorkspaceId,
    /// The repository's name within the workspace.
    pub repo: String,
}

/// A language, framework or platform a repository is built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stack {
    Flutter,
    Dart,
    Node,
    Rust,
    Python,
    Go,
    Supabase,
    Ruby,
    DotNet,
    Unity,
    Godot,
}

impl Stack {
    pub const ALL: [Stack; 11] = [
        Stack::Flutter,
        Stack::Dart,
        Stack::Node,
        Stack::Rust,
        Stack::Python,
        Stack::Go,
        Stack::Supabase,
        Stack::Ruby,
        Stack::DotNet,
        Stack::Unity,
        Stack::Godot,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Stack::Flutter => "flutter",
            Stack::Dart => "dart",
            Stack::Node => "node",
            Stack::Rust => "rust",
            Stack::Python => "python",
            Stack::Go => "go",
            Stack::Supabase => "supabase",
            Stack::Ruby => "ruby",
            Stack::DotNet => "dotnet",
            Stack::Unity => "unity",
            Stack::Godot => "godot",
        }
    }

    pub fn parse(s: &str) -> Option<Stack> {
        Stack::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

impl fmt::Display for Stack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A command a repository offers: a script, a make target, a recipe, or
/// what its stack runs by convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoCommand {
    /// Short name: `test`, `build`, `lint`, ...
    pub name: String,
    /// The command line, run in the repository's folder.
    pub line: String,
    /// Where it was found: `package.json`, `Makefile`, `justfile`, or the
    /// stack it is the convention of.
    pub source: String,
}

/// A git repository inside a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// Unique within the workspace: the folder's name, or its path with
    /// `-` when two folders share a name.
    pub name: String,
    /// Relative to the workspace root, with `/`; `.` for the root itself.
    pub path: String,
    /// The repository's top folder.
    pub git_root: PathBuf,
    /// Unknown when the repository has neither a remote HEAD nor a branch.
    pub default_branch: Option<String>,
    pub stacks: Vec<Stack>,
    pub commands: Vec<RepoCommand>,
}

impl Repo {
    /// The repository's folder below `base` (the workspace root, or a task
    /// folder laid out like it).
    pub fn under(&self, base: &std::path::Path) -> PathBuf {
        if self.path == "." {
            return base.to_path_buf();
        }
        self.path
            .split('/')
            .fold(base.to_path_buf(), |p, c| p.join(c))
    }

    /// How deep the repository sits below the root (0 for the root).
    pub fn depth(&self) -> usize {
        if self.path == "." {
            0
        } else {
            self.path.split('/').count()
        }
    }
}

/// A product folder and the repositories in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    /// Unique; what the CLI and missions name it by.
    pub slug: String,
    pub name: String,
    pub root: PathBuf,
    /// In the order they were found: the root first, then by path.
    pub repos: Vec<Repo>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

/// Why a workspace or a choice of its repositories is not valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    EmptyName,
    NameTooLong {
        max: usize,
    },
    /// Names a repository the workspace does not have.
    UnknownRepo {
        repo: String,
        known: Vec<String>,
    },
    /// The same repository is named twice.
    RepeatedRepo(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceError::EmptyName => f.write_str("a workspace needs a name"),
            WorkspaceError::NameTooLong { max } => {
                write!(f, "a workspace name has at most {max} characters")
            }
            WorkspaceError::UnknownRepo { repo, known } if known.is_empty() => {
                write!(f, "no repository {repo:?}: the workspace has none")
            }
            WorkspaceError::UnknownRepo { repo, known } => write!(
                f,
                "no repository {repo:?} in the workspace (it has {})",
                known.join(", ")
            ),
            WorkspaceError::RepeatedRepo(repo) => write!(f, "{repo:?} is named twice"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl Workspace {
    /// A workspace with no repositories yet. The name is trimmed; the slug
    /// is made from it.
    pub fn new(
        id: WorkspaceId,
        name: &str,
        root: PathBuf,
        now: Millis,
    ) -> Result<Workspace, WorkspaceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::EmptyName);
        }
        if name.chars().count() > NAME_MAX {
            return Err(WorkspaceError::NameTooLong { max: NAME_MAX });
        }
        Ok(Workspace {
            id,
            slug: crate::slug(name),
            name: name.to_string(),
            root,
            repos: Vec::new(),
            created_at: now,
            updated_at: now,
        })
    }

    pub fn repo(&self, name: &str) -> Option<&Repo> {
        self.repos.iter().find(|r| r.name == name)
    }

    pub fn repo_ref(&self, name: &str) -> Option<RepoRef> {
        self.repo(name).map(|r| RepoRef {
            workspace: self.id,
            repo: r.name.clone(),
        })
    }

    pub fn repo_names(&self) -> Vec<String> {
        self.repos.iter().map(|r| r.name.clone()).collect()
    }

    /// The repositories a job touches, in the order given (producers
    /// first). Naming none means all of them, in the workspace's order:
    /// every repository gets a worktree, which is safe but costs a
    /// checkout per repository.
    pub fn touched(&self, names: &[String]) -> Result<Vec<Repo>, WorkspaceError> {
        if names.iter().all(|n| n.trim().is_empty()) {
            return Ok(self.repos.clone());
        }
        let mut out: Vec<Repo> = Vec::new();
        for name in names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
            let repo = self.repo(name).ok_or_else(|| WorkspaceError::UnknownRepo {
                repo: name.to_string(),
                known: self.repo_names(),
            })?;
            if out.iter().any(|r| r.name == repo.name) {
                return Err(WorkspaceError::RepeatedRepo(name.to_string()));
            }
            out.push(repo.clone());
        }
        Ok(out)
    }

    /// Replaces the repositories with what a scan found, in its order.
    pub fn set_repos(&mut self, repos: Vec<Repo>, now: Millis) {
        self.repos = repos;
        self.updated_at = now;
    }
}

/// Names for repositories found at `paths` (relative, with `/`; `.` for
/// the root): each folder's own name, or for folders sharing a name their
/// whole path joined with `-`. `root_name` names the root's repository.
pub fn repo_names(paths: &[String], root_name: &str) -> Vec<String> {
    let base = |p: &str| -> String {
        if p == "." {
            crate::slug(root_name)
        } else {
            p.rsplit('/').next().unwrap_or(p).to_string()
        }
    };
    paths
        .iter()
        .map(|p| {
            let own = base(p);
            let shared = paths.iter().filter(|q| base(q) == own).count() > 1;
            if shared && p != "." {
                p.replace('/', "-")
            } else {
                own
            }
        })
        .collect()
}

/// Where a check of an acceptance list runs: `"app: flutter test"` runs
/// `flutter test` in the repository (or subfolder) `app`. A check without
/// a prefix runs at the root. The prefix is one word ending in `:`
/// followed by a space, so command lines with colons elsewhere are left
/// alone.
pub fn split_check(line: &str) -> (Option<&str>, &str) {
    let trimmed = line.trim_start();
    let Some((head, rest)) = trimmed.split_once(char::is_whitespace) else {
        return (None, trimmed);
    };
    let Some(target) = head.strip_suffix(':') else {
        return (None, trimmed);
    };
    let ok = !target.is_empty()
        && target
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        && !target.starts_with('/')
        && !target.split('/').any(|c| c == "..");
    // `C:` is a drive, not a repository.
    let drive = target.len() == 1 && target.chars().all(|c| c.is_ascii_alphabetic());
    if !ok || drive || rest.trim().is_empty() {
        return (None, trimmed);
    }
    (Some(target), rest.trim_start())
}

#[cfg(test)]
mod tests;
