//! swarmux infrastructure: adapters that implement the application's ports.

pub mod claude_cli;
mod clock;
pub mod config;
pub mod git;
mod monitor;
pub mod paths;
pub mod sqlite;

pub use claude_cli::ClaudeCli;
pub use clock::SystemClock;
pub use config::{ConfigError, load_settings};
pub use git::GitCli;
pub use monitor::SysinfoMonitor;
pub use paths::Paths;
pub use sqlite::SqliteStore;
