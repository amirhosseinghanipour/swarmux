use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux::{app::App, doctor, paths::Paths, store::Store};

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
    let store = Store::open(&paths.db)?;
    store.recover()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let cwd = std::env::current_dir()?;
    let app = App::new(store, doctor::claude_bin(), rt.handle().clone(), cwd);

    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
