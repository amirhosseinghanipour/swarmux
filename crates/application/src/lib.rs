//! swarmux application layer: use cases, and the ports they need.
//!
//! Depends only on the domain. Adapters (SQLite, the `claude` CLI, the TUI)
//! live in other crates and plug in through the traits in [`ports`].

mod error;
mod guard;
mod history;
mod jobs;
pub mod ports;
mod prompts;
mod sessions;
mod settings;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use error::{AppError, AppResult};
pub use guard::{
    AlwaysAllowed, DecisionRecord, GuardReport, GuardService, GuardStats, Outcome, PendingApproval,
    SharedPolicy,
};
pub use history::{HISTORY_LIMIT, HistoryService, JobDetail, JobSummary};
pub use jobs::{JobService, PumpReport, QueueEntry, QueueStats};
pub use prompts::{BUILTIN_VARIABLES, PromptService, RenderedPrompt};
pub use sessions::SessionService;
pub use settings::{GuardSettings, Settings};
