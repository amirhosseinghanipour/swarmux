mod app;
mod doctor;
mod ui;

use clap::{Parser, Subcommand};

/// swarmux — a prompt-first command center for Claude Code.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Check that Claude Code is installed and ready.
    Doctor,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        None => run_tui(),
        Some(Command::Doctor) => doctor::run(),
    }
}

fn run_tui() -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = app::App::default().run(&mut terminal);
    ratatui::restore();
    result
}
