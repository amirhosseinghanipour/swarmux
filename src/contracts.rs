//! `swarmux contracts`: what each repository of a workspace provides (a
//! table, a column, an RPC function, an endpoint, an environment key) and
//! which files of which repositories consume it. Read from the sources;
//! no model is asked. Every call reads again only the files that changed.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use swarmux_application::{ContractService, Rescanned, WorkspaceService};
use swarmux_domain::WorkspaceId;
use swarmux_infrastructure::{FsSourceTree, Paths, SqliteStore, SystemClock};

/// What unknown values are shown as.
const UNKNOWN: &str = "—";

#[derive(Args)]
pub struct ContractsArgs {
    /// The workspace, by name (default: the one whose root is the current
    /// folder, or the only one).
    #[arg(long)]
    pub workspace: Option<String>,
    /// List the consumers under each contract.
    #[arg(long)]
    pub consumers: bool,
    #[command(subcommand)]
    pub action: Option<ContractsAction>,
}

#[derive(Subcommand)]
pub enum ContractsAction {
    /// One contract and everything that consumes it: `orders`,
    /// `orders.total`, `get_orders`, `GET /api/orders`, `env API_KEY`.
    Show { symbol: String },
}

/// The contract service over the database in swarmux's folder.
pub fn service(store: Rc<SqliteStore>) -> ContractService {
    ContractService::new(
        store.clone(),
        store,
        Arc::new(FsSourceTree),
        Rc::new(SystemClock),
    )
}

fn open() -> anyhow::Result<(ContractService, WorkspaceService)> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    Ok((service(store.clone()), crate::workspace::service(store)))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    norm(a).trim_end_matches('/') == norm(b).trim_end_matches('/')
}

fn summary(r: &Rescanned) -> String {
    let s = r.stats;
    format!(
        "{} contract{} · {} use{} · {} file{} read ({} read again, {} gone)",
        r.providers,
        if r.providers == 1 { "" } else { "s" },
        r.consumers,
        if r.consumers == 1 { "" } else { "s" },
        s.files,
        if s.files == 1 { "" } else { "s" },
        s.extracted,
        s.removed
    )
}

/// Reads a workspace's files again after it was added or rescanned.
pub fn rescan_workspace(id: WorkspaceId) -> anyhow::Result<()> {
    let (contracts, workspaces) = open()?;
    let Some(w) = workspaces.get(id)? else {
        return Ok(());
    };
    let r = contracts.rescan(&w)?;
    println!("contracts: {}", summary(&r));
    for c in r.changed.iter().take(10) {
        println!("  changed: {c}");
    }
    Ok(())
}

pub fn run(args: ContractsArgs) -> anyhow::Result<()> {
    let (contracts, workspaces) = open()?;
    let w = match &args.workspace {
        Some(name) => workspaces.find(name)?,
        None => {
            let all = workspaces.list()?;
            let here = std::env::current_dir()?;
            match all
                .iter()
                .find(|w| same_dir(&w.root, &here))
                .or_else(|| (all.len() == 1).then(|| &all[0]))
            {
                Some(w) => w.clone(),
                None if all.is_empty() => {
                    bail!("no workspaces: add one with `swarmux workspace add <folder>`")
                }
                None => bail!("which workspace? use --workspace <name> (swarmux workspace list)"),
            }
        }
    };
    let r = contracts.rescan(&w)?;
    println!("workspace {}: {}", w.name, summary(&r));
    match args.action {
        Some(ContractsAction::Show { symbol }) => {
            let found = contracts.consumers(w.id, &symbol)?;
            if found.is_empty() {
                bail!("no contract named {symbol:?} in workspace {}", w.name);
            }
            for c in found {
                println!();
                println!("{} — {}", c.provided.sym.label(), c.provided.place());
                if !c.shape.is_empty() {
                    println!("  shape: {}", c.shape);
                }
                if c.sites.is_empty() {
                    println!("  consumed by: nothing found");
                    continue;
                }
                println!("  consumed by {}:", c.sites.len());
                for s in &c.sites {
                    println!(
                        "    {}{}",
                        s.place(),
                        if s.inferred {
                            "  (inferred from a model)"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
        None => {
            let rows = contracts.rows(w.id)?;
            if rows.is_empty() {
                println!(
                    "no contracts found (SQL migrations, OpenAPI files and .env.example are read)"
                );
                return Ok(());
            }
            for row in &rows {
                println!(
                    "{:<40} {:<48} {}",
                    row.provided.sym.label(),
                    row.provided.place(),
                    match row.consumers {
                        0 => UNKNOWN.to_string(),
                        1 => "1 consumer".to_string(),
                        n => format!("{n} consumers"),
                    }
                );
                if args.consumers {
                    for c in contracts.consumers(w.id, &row.provided.sym.label())? {
                        if c.provided.repo != row.provided.repo
                            || c.provided.file != row.provided.file
                        {
                            continue;
                        }
                        for s in &c.sites {
                            println!("    {}", s.place());
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
