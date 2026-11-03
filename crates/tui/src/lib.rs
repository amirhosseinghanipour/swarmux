//! swarmux presentation layer: the terminal interface.
//!
//! Reads domain types and calls application use cases. Knows nothing about
//! SQLite, processes or the `claude` CLI.

mod app;
mod dashboard;
mod focus;
mod inbox;
mod ledger;
mod mission;
mod missions;
mod prompts;
mod queue;
mod routing;
pub mod telemetry;
pub mod theme;
pub mod theme_export;
#[cfg(test)]
mod theme_lint;
mod ui;

pub use app::{App, Launch};

/// Takes over the terminal, runs the app until the user quits, then restores it.
pub fn run(app: App) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    // Terminals that don't know focus reporting ignore this.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableFocusChange);
    let result = app.run(&mut terminal);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableFocusChange);
    ratatui::restore();
    result
}
