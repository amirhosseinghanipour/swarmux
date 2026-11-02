//! swarmux presentation layer: the terminal interface.
//!
//! Reads domain types and calls application use cases. Knows nothing about
//! SQLite, processes or the `claude` CLI.

mod app;
mod dashboard;
mod history;
mod prompts;
mod queue;
mod routing;
pub mod theme;
mod ui;

pub use app::App;

/// Takes over the terminal, runs the app until the user quits, then restores it.
pub fn run(app: App) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
