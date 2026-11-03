//! swarmux infrastructure: adapters that implement the application's ports.

pub mod agents;
pub mod claude_cli;
mod clock;
pub mod config;
pub mod config_edit;
mod config_telemetry;
pub mod gate;
pub mod git;
pub mod hook;
pub mod jev;
mod monitor;
pub mod paths;
pub mod sqlite;
pub mod telemetry;

pub use agents::FsAgentCatalog;
pub use claude_cli::{ClaudeCli, GuardHook, HaikuRouter};
pub use clock::SystemClock;
pub use config::{ConfigError, load_settings};
pub use config_edit::ConfigRuleWriter;
pub use gate::{FsGateSource, ProcessChecker};
pub use git::GitCli;
pub use hook::HookServer;
pub use jev::{Endpoint, JevHttp};
pub use monitor::SysinfoMonitor;
pub use paths::Paths;
pub use sqlite::{SqliteLedger, SqliteStore};
