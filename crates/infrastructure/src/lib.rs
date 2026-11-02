//! swarmux infrastructure: adapters that implement the application's ports.

pub mod claude_cli;
mod clock;
pub mod git;
pub mod paths;
pub mod sqlite;

pub use claude_cli::ClaudeCli;
pub use clock::SystemClock;
pub use git::GitCli;
pub use paths::Paths;
pub use sqlite::SqliteStore;
