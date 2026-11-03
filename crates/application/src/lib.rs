//! swarmux application layer: use cases, and the ports they need.
//!
//! Depends only on the domain. Adapters (SQLite, the `claude` CLI, the TUI)
//! live in other crates and plug in through the traits in [`ports`].

mod attention;
pub mod checkpoint;
mod decision;
mod error;
pub mod gate;
mod guard;
mod history;
pub mod hookbus;
mod jev;
mod jobs;
pub mod ledger;
pub mod ports;
mod prompts;
mod routing;
mod sessions;
mod settings;
mod telemetry;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod worktree;

pub use attention::{AttentionService, Change, ChangeKind};
pub use checkpoint::{CheckpointService, HandoffBoard, Notes};
pub use decision::{DecisionRecord, Outcome, Source};
pub use error::{AppError, AppResult};
pub use gate::{
    Checker, ExecutionLog, GateKeeper, GateRecord, GateService, GateSource, GateState, LoadedGate,
    WriteFence, run_checks,
};
pub use guard::{
    AlwaysAllowed, GuardReport, GuardService, GuardStats, PendingApproval, SharedPolicy,
};
pub use history::{HISTORY_LIMIT, HistoryService, JobDetail, JobSummary};
pub use hookbus::{
    AllowStop, HookBus, HookFeed, HookNotice, NoSessionContext, SessionSignals,
    SessionStartContext, SessionStartInfo, StopAnswer, StopContext, StopPolicy,
};
pub use jev::{Jev, JevState, SUMMARY_MAX, summarize};
pub use jobs::{JobFact, JobService, PumpReport, QueueEntry, QueueStats};
pub use prompts::{BUILTIN_VARIABLES, PromptService, RenderedPrompt};
pub use routing::{ROUTE_PROMPT_MAX, RouteReport, Routed, RoutingService};
pub use sessions::SessionService;
pub use settings::{
    GuardSettings, JevProvider, JevSettings, LayoutSettings, ReviewSettings, RowSettings, Settings,
    TelemetrySettings, UiSettings, WorktreeSettings,
};
pub use telemetry::{
    ActivityView, Battery, CiRun, Collected, Commit, DiffStat, DiskUsage, ENVIRONMENTS,
    EnvironmentView, GitStatus, GithubStatus, LimitView, ProcessStat, PullRequest, SystemStats,
    TICKER_LEN, TOP_FILES, TelemetryService, TelemetrySnapshot,
};
pub use worktree::{
    AddWorktree, Added, GcReport, Prepared, Removed, WorktreeService, WorktreeStore, WorktreeView,
    Worktrees,
};
