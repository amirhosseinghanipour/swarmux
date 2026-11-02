//! swarmux infrastructure: adapters that implement the application's ports.

pub mod claude_cli;
mod clock;
pub mod config;
pub mod config_edit;
pub mod git;
pub mod hook;
pub mod jev;
mod monitor;
pub mod paths;
pub mod sqlite;

pub use claude_cli::{ClaudeCli, GuardHook};
pub use clock::SystemClock;
pub use config::{ConfigError, load_settings};
pub use config_edit::ConfigRuleWriter;
pub use git::GitCli;
pub use hook::HookServer;
pub use jev::{Endpoint, JevHttp};
pub use monitor::SysinfoMonitor;
pub use paths::Paths;
pub use sqlite::SqliteStore;
