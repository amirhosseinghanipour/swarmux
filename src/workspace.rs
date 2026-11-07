//! `swarmux workspace`: add a product folder of several repositories,
//! list, show, rescan and forget workspaces.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Context;
use clap::Subcommand;
use swarmux_application::WorkspaceService;
use swarmux_domain::{Repo, Workspace, iso_minute};
use swarmux_infrastructure::{FsWorkspaceScanner, Paths, SqliteStore, SystemClock};

/// What unknown values are shown as.
const UNKNOWN: &str = "—";

#[derive(Subcommand)]
pub enum Action {
    /// Record a folder as a workspace: its git repositories (the folder
    /// itself, its folders and theirs, two levels down), what each is built
    /// with and the commands it offers.
    Add {
        /// The product folder (it need not be a git repository).
        dir: PathBuf,
        /// What to call it (default: the folder's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Show every workspace.
    List,
    /// Show a workspace's repositories, stacks and commands.
    Show { name: String },
    /// Look at a workspace's folder again for repositories that came or went.
    Rescan { name: String },
    /// Forget a workspace. No file is touched; its sessions and missions
    /// carry on as plain ones in its folder.
    Rm { name: String },
}

/// The workspace service over the database in swarmux's folder.
pub fn service(store: Rc<SqliteStore>) -> WorkspaceService {
    WorkspaceService::new(
        store,
        Rc::new(FsWorkspaceScanner::default()),
        Rc::new(SystemClock),
    )
}

fn open() -> anyhow::Result<WorkspaceService> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    Ok(service(Rc::new(SqliteStore::open(&paths.db)?)))
}

pub fn run(action: Action) -> anyhow::Result<()> {
    let svc = open()?;
    match action {
        Action::Add { dir, name } => add(&svc, &dir, name.as_deref()),
        Action::List => list(&svc),
        Action::Show { name } => {
            print!("{}", show(&svc.find(&name)?));
            Ok(())
        }
        Action::Rescan { name } => {
            let (w, change) = svc.rescan(&name)?;
            for repo in &change.added {
                println!("+ {repo}");
            }
            for repo in &change.removed {
                println!("- {repo}");
            }
            if change.added.is_empty() && change.removed.is_empty() {
                println!("no repositories came or went");
            }
            print!("{}", show(&w));
            crate::knowledge::recheck_workspace(w.id)?;
            Ok(())
        }
        Action::Rm { name } => {
            let w = svc.remove(&name)?;
            println!(
                "forgot {} ({}); nothing on disk was touched",
                w.name,
                w.root.display()
            );
            Ok(())
        }
    }
}

fn add(svc: &WorkspaceService, dir: &Path, name: Option<&str>) -> anyhow::Result<()> {
    let root = std::path::absolute(dir).with_context(|| format!("{}", dir.display()))?;
    anyhow::ensure!(root.is_dir(), "{} is not a folder", root.display());
    let w = svc.add(&root, name)?;
    print!("{}", show(&w));
    if w.repos.is_empty() {
        println!("no git repositories found: swarmux looks at the folder, its folders and theirs");
    }
    println!(
        "start a session on it from the Sessions screen (6, then w), or name its repositories in a mission's repos = [...]"
    );
    Ok(())
}

fn list(svc: &WorkspaceService) -> anyhow::Result<()> {
    let all = svc.list()?;
    if all.is_empty() {
        println!("no workspaces: add one with `swarmux workspace add <folder>`");
        return Ok(());
    }
    for w in &all {
        let names = w.repo_names();
        println!(
            "{:<20} {:>2} repos  {}  {}",
            w.name,
            w.repos.len(),
            if names.is_empty() {
                UNKNOWN.to_string()
            } else {
                names.join(", ")
            },
            w.root.display()
        );
    }
    Ok(())
}

/// A workspace as `show` prints it.
pub fn show(w: &Workspace) -> String {
    let mut out = format!(
        "{} ({})\n  root     {}\n  scanned  {}\n",
        w.name,
        w.slug,
        w.root.display(),
        iso_minute(w.updated_at)
    );
    for r in &w.repos {
        out.push_str(&repo_block(r));
    }
    out
}

fn repo_block(r: &Repo) -> String {
    let stacks: Vec<&str> = r.stacks.iter().map(|s| s.as_str()).collect();
    let mut out = format!(
        "\n  {}  {}\n    branch   {}\n    stacks   {}\n",
        r.name,
        r.path,
        r.default_branch.as_deref().unwrap_or(UNKNOWN),
        if stacks.is_empty() {
            UNKNOWN.to_string()
        } else {
            stacks.join(", ")
        }
    );
    if r.commands.is_empty() {
        out.push_str(&format!("    commands {UNKNOWN}\n"));
    }
    for (i, c) in r.commands.iter().enumerate() {
        let label = if i == 0 { "commands" } else { "" };
        out.push_str(&format!("    {label:<8} {:<28} {}\n", c.line, c.source));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::workspace::RepoCommand;
    use swarmux_domain::{Stack, WorkspaceId};

    #[test]
    fn unknown_values_show_as_a_dash() {
        let mut w = Workspace::new(WorkspaceId::random(), "Product", "/p".into(), 0).unwrap();
        w.set_repos(
            vec![
                Repo {
                    name: "app".into(),
                    path: "app".into(),
                    git_root: "/p/app".into(),
                    default_branch: None,
                    stacks: Vec::new(),
                    commands: Vec::new(),
                },
                Repo {
                    name: "api".into(),
                    path: "services/api".into(),
                    git_root: "/p/services/api".into(),
                    default_branch: Some("main".into()),
                    stacks: vec![Stack::Go],
                    commands: vec![RepoCommand {
                        name: "test".into(),
                        line: "go test ./...".into(),
                        source: "go".into(),
                    }],
                },
            ],
            0,
        );
        let text = show(&w);
        assert!(text.contains("branch   —"), "{text}");
        assert!(text.contains("stacks   —"));
        assert!(text.contains("commands —"));
        assert!(text.contains("go test ./..."));
        assert!(text.contains("api  services/api"));
    }
}
