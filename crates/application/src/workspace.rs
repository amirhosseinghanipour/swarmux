//! Workspace use cases: add a product folder, find the git repositories in
//! it and what each is built with, keep that up to date, and forget it.
//!
//! The ports this needs are declared here: [`WorkspaceStore`] keeps
//! workspaces, [`WorkspaceScanner`] looks at the disk. Detection itself is
//! the domain's (`swarmux_domain::workspace::detect`), so what a scan
//! finds can be checked without a disk.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use swarmux_domain::workspace::{RepoListing, detect, repo_names};
use swarmux_domain::{Repo, Workspace, WorkspaceId};

use crate::ports::Clock;
use crate::{AppError, AppResult};

/// Where workspaces are kept.
pub trait WorkspaceStore {
    /// Inserts the workspace, or replaces the one with its id (and its
    /// repositories).
    fn save(&self, workspace: &Workspace) -> AppResult<()>;
    /// Every workspace, by name.
    fn list(&self) -> AppResult<Vec<Workspace>>;
    fn get(&self, id: WorkspaceId) -> AppResult<Option<Workspace>>;
    /// Forgets the workspace. Sessions and missions that belonged to it
    /// keep working in its folder as plain ones.
    fn remove(&self, id: WorkspaceId) -> AppResult<()>;
}

/// A git repository a scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedRepo {
    /// Relative to the scanned root, with `/`; `.` for the root itself.
    pub path: String,
    /// The repository's top folder.
    pub git_root: PathBuf,
    /// `origin`'s HEAD, else the branch checked out; `None` when neither
    /// is known.
    pub default_branch: Option<String>,
    /// Its files, for detection.
    pub listing: RepoListing,
}

/// Looks for git repositories on disk.
pub trait WorkspaceScanner {
    /// The repositories at `root`: the root itself when it is one, its
    /// folders, and their folders (two levels down). A repository's own
    /// folders are not searched. Ordered by path, the root first.
    fn scan(&self, root: &Path) -> AppResult<Vec<ScannedRepo>>;
}

/// What a rescan changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rescan {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Adds, lists, rescans and forgets workspaces.
pub struct WorkspaceService {
    store: Rc<dyn WorkspaceStore>,
    scanner: Rc<dyn WorkspaceScanner>,
    clock: Rc<dyn Clock>,
}

impl WorkspaceService {
    pub fn new(
        store: Rc<dyn WorkspaceStore>,
        scanner: Rc<dyn WorkspaceScanner>,
        clock: Rc<dyn Clock>,
    ) -> WorkspaceService {
        WorkspaceService {
            store,
            scanner,
            clock,
        }
    }

    /// Records the folder `root` (absolute) as a workspace named `name`
    /// (the folder's name by default) with the repositories found in it.
    pub fn add(&self, root: &Path, name: Option<&str>) -> AppResult<Workspace> {
        let folder = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let name = name.unwrap_or(&folder);
        let mut workspace = Workspace::new(
            WorkspaceId::random(),
            name,
            root.to_path_buf(),
            self.clock.now(),
        )
        .map_err(|e| AppError::Workspaces(e.to_string()))?;
        for other in self.store.list()? {
            if other.slug == workspace.slug {
                return Err(AppError::Workspaces(format!(
                    "there is a workspace named {} already",
                    other.name
                )));
            }
            if same_folder(&other.root, root) {
                return Err(AppError::Workspaces(format!(
                    "{} is the workspace {} already",
                    root.display(),
                    other.name
                )));
            }
        }
        let repos = self.scan(root, name)?;
        workspace.set_repos(repos, self.clock.now());
        self.store.save(&workspace)?;
        Ok(workspace)
    }

    /// Every workspace, by name.
    pub fn list(&self) -> AppResult<Vec<Workspace>> {
        self.store.list()
    }

    pub fn get(&self, id: WorkspaceId) -> AppResult<Option<Workspace>> {
        self.store.get(id)
    }

    /// The workspace called `name` (its name or slug, any case).
    pub fn find(&self, name: &str) -> AppResult<Workspace> {
        let want = name.trim();
        let slug = swarmux_domain::slug(want);
        self.store
            .list()?
            .into_iter()
            .find(|w| w.name.eq_ignore_ascii_case(want) || w.slug == slug)
            .ok_or_else(|| AppError::WorkspaceNotFound(want.to_string()))
    }

    /// Looks at the workspace's folder again. Repositories keep their
    /// names, so what refers to them still does.
    pub fn rescan(&self, name: &str) -> AppResult<(Workspace, Rescan)> {
        let mut workspace = self.find(name)?;
        let before = workspace.repo_names();
        let mut repos = self.scan(&workspace.root, &workspace.name)?;
        // A repository found where one was keeps its name.
        for repo in &mut repos {
            if let Some(old) = workspace.repos.iter().find(|o| o.path == repo.path) {
                repo.name = old.name.clone();
            }
        }
        let after: Vec<String> = repos.iter().map(|r| r.name.clone()).collect();
        let change = Rescan {
            added: after
                .iter()
                .filter(|n| !before.contains(n))
                .cloned()
                .collect(),
            removed: before
                .iter()
                .filter(|n| !after.contains(n))
                .cloned()
                .collect(),
        };
        workspace.set_repos(repos, self.clock.now());
        self.store.save(&workspace)?;
        Ok((workspace, change))
    }

    /// Forgets the workspace. No file is touched.
    pub fn remove(&self, name: &str) -> AppResult<Workspace> {
        let workspace = self.find(name)?;
        self.store.remove(workspace.id)?;
        Ok(workspace)
    }

    fn scan(&self, root: &Path, name: &str) -> AppResult<Vec<Repo>> {
        let found = self.scanner.scan(root)?;
        let paths: Vec<String> = found.iter().map(|s| s.path.clone()).collect();
        let names = repo_names(&paths, name);
        Ok(found
            .into_iter()
            .zip(names)
            .map(|(scanned, name)| {
                let detected = detect(&scanned.listing);
                Repo {
                    name,
                    path: scanned.path,
                    git_root: scanned.git_root,
                    default_branch: scanned.default_branch,
                    stacks: detected.stacks,
                    commands: detected.commands,
                }
            })
            .collect())
    }
}

/// Whether two folders are the same, as far as their text tells
/// (Windows paths compare without case).
fn same_folder(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests;
