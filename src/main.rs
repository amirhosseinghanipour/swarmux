//! Composition root: the only place that wires concrete adapters to use cases.

mod doctor;

use std::rc::Rc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux_application::{JobService, SessionService};
use swarmux_infrastructure::{ClaudeCli, Paths, SqliteStore, SystemClock};
use swarmux_tui::App;

/// swarmux — a prompt-first command center for Claude Code.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Check that Claude Code is installed and swarmux's data is healthy.
    Doctor,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        None => run_tui(),
        Some(Command::Doctor) => doctor::run(),
    }
}

fn run_tui() -> anyhow::Result<()> {
    // Everything that can fail with a readable message happens before the
    // terminal switches into full-screen mode.
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(SystemClock);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let runner = Rc::new(ClaudeCli::new(doctor::claude_bin(), rt.handle().clone()));

    let sessions = SessionService::new(store.clone(), clock.clone());
    let jobs = JobService::new(store.clone(), store, runner, clock);
    jobs.recover()?;
    let app = App::new(sessions, jobs, std::env::current_dir()?);
    Ok(swarmux_tui::run(app)?)
}
