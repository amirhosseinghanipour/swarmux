//! swarmux application layer: use cases, and the ports they need.
//!
//! Depends only on the domain. Adapters (SQLite, the `claude` CLI, the TUI)
//! live in other crates and plug in through the traits in [`ports`].

mod attention;
pub mod checkpoint;
mod decision;
mod error;
pub mod evidence;
pub mod gate;
pub mod governor;
mod guard;
mod history;
pub mod hookbus;
mod jev;
mod jobs;
pub mod ledger;
pub mod missions;
mod notify;
pub mod ports;
mod prompts;
mod routing;
mod sessions;
mod settings;
mod telemetry;
mod terminal;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod worktree;

pub use attention::{AttentionService, Change, ChangeKind};
pub use checkpoint::{CheckpointService, HandoffBoard, Notes};
pub use decision::{DecisionRecord, Outcome, Source};
pub use error::{AppError, AppResult};
pub use evidence::{
    Checkout, Checkouts, EvidenceService, EvidenceStore, HashSigner, HeadFacts, OutcomeProbe,
    ProbeReply, ProbeRequest, ProbeSink, Rerunner, Signer, Stored, Vcs, build_cards,
};
pub use gate::{
    Checker, ExecutionLog, GateKeeper, GateRecord, GateService, GateSource, GateState, JobGates,
    LoadedGate, WriteFence, run_checks,
};
pub use governor::{Governor, PendingResume, ResumeBook, UsageProbe};
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
pub use missions::{
    LintReply, LintRequest, LintSink, MissionLinter, MissionReport, MissionService, MissionStore,
};
pub use notify::{
    FocusTracker, IDLE_UNFOCUSED_MS, Moment, NoNotifier, Notification, Notifier, NotifyService,
    NotifySettings, QuietHours, TerminalMode, may_notify,
};
pub use prompts::{BUILTIN_VARIABLES, PromptService, RenderedPrompt};
pub use routing::{ROUTE_PROMPT_MAX, RouteReport, Routed, RoutingService};
pub use sessions::SessionService;
pub use settings::{
    GovernorSettings, GuardSettings, JevProvider, JevSettings, LayoutSettings, ReviewSettings,
    RowSettings, Settings, TelemetrySettings, UiSettings, WorktreeSettings,
};
pub use telemetry::{
    ActivityView, AgentsPoll, Battery, CacheView, CiRun, Collected, CombinedStatusline, Commit,
    DiffStat, DiskUsage, ENVIRONMENTS, EnvironmentView, GitStatus, GithubStatus, InstructionsView,
    LimitView, ProcessStat, PullRequest, StatuslineFeed, StatuslineView, SystemStats, TICKER_LEN,
    TOP_FILES, TelemetryService, TelemetrySnapshot, TruthBoard,
};
pub use terminal::SessionTerminal;
pub use worktree::{
    AddWorktree, Added, GcReport, Prepared, Removed, WorktreeService, WorktreeStore, WorktreeView,
    Worktrees,
};
