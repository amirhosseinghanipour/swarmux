//! swarmux application layer: use cases, and the ports they need.
//!
//! Depends only on the domain. Adapters (SQLite, the `claude` CLI, the TUI)
//! live in other crates and plug in through the traits in [`ports`].

pub mod anchor;
pub mod attention;
pub mod board;
pub mod cache;
pub mod checkpoint;
pub mod console;
pub mod contracts;
pub mod daemon;
mod decision;
pub mod dist;
pub mod engine;
mod error;
pub mod evidence;
pub mod followup;
pub mod gate;
pub mod governor;
mod guard;
mod history;
pub mod hookbus;
mod jev;
mod jobs;
pub mod knowledge;
pub mod ledger;
pub mod metrics;
pub mod missions;
mod notify;
mod palette;
pub mod ports;
mod prompts;
mod routing;
mod sendback;
mod sessions;
mod settings;
mod telemetry;
mod terminal;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod verifier;
pub mod watchdog;
pub mod workspace;
pub mod worktree;

pub use attention::{
    AlarmEvent, AlarmKind, AlarmStore, AttentionService, Change, ChangeKind, HeldBack, ShelveFor,
    ShelvedItem, Suggestion, TallyRow, alarm_report,
};
pub use board::{
    Board, BoardCall, BoardDigest, BoardInbox, BoardReply, BoardRequest, BoardService, BoardStore,
    Member,
};
pub use checkpoint::{CheckpointService, HandoffBoard, Notes};
pub use contracts::{
    Consumed, ContractEvent, ContractGate, ContractHub, ContractJudgement, ContractRow,
    ContractService, ContractStore, FileRecord, PullContext, Rescanned, ScanStats, SourceFile,
    SourceTree, StartFan, WatchFan,
};
pub use decision::{DecisionExtra, DecisionRecord, JevTrace, Outcome, Source};
pub use engine::{Engine, EngineReport, MissionsPump};
pub use error::{AppError, AppResult};
pub use evidence::{
    Checkout, Checkouts, EvidenceService, EvidenceStore, HashSigner, HeadFacts, OutcomeProbe,
    ProbeReply, ProbeRequest, ProbeSink, Rerunner, Signer, Stored, Vcs, build_cards,
};
pub use followup::{
    BriefChannel, ExtractReply, ExtractRequest, ExtractSink, Extracted, Extractor, FollowPump,
    FollowSettings, FollowStore, FollowUpService, Intent, IntentStore, OwnerReply, Reply,
    ReplySource,
};
pub use gate::{
    Checker, ExecutionLog, GateDetail, GateKeeper, GateRecord, GateRun, GateService, GateSource,
    GateState, JobGates, LoadedGate, WriteFence, run_checks,
};
pub use governor::{Governor, PendingResume, ResumeBook, UsageProbe};
pub use guard::{
    AlwaysAllowed, AlwaysSettings, Confirmation, GuardReport, GuardService, GuardStats,
    IRREVERSIBLE_REASON, PROTECTED_REASON, PendingApproval, Protection, SharedPolicy,
    UnattendedJobs,
};
pub use history::{HISTORY_LIMIT, HistoryService, JobDetail, JobSummary};
pub use hookbus::{
    AllowStop, HookBus, HookFeed, HookNotice, NoSessionContext, SessionSignals,
    SessionStartContext, SessionStartInfo, StopAnswer, StopContext, StopPolicy,
};
pub use jev::{Jev, JevState, SUMMARY_MAX, summarize};
pub use jobs::{
    JobFact, JobService, JobStart, JobStarts, PumpReport, QueueEntry, QueueStats, Restart,
};
pub use knowledge::{
    AnchorReader, Imported, JobInfo, KnowledgeCodec, KnowledgeHub, KnowledgeService,
    KnowledgeStore, MapSaved, NoToolContext, Rebuilt, RuleCheck, RuleChecks, RuleGuard,
    ToolContext, Usage,
};
pub use missions::{
    Begun, ChangedFiles, Claim, LandReport, LandingPolicy, LintReply, LintRequest, LintSink,
    MergeOutcome, MergeQueue, MissionLinter, MissionReport, MissionService, MissionStore,
    MissionWatch, PlanReply, PlanRequest, PlanSettings, PlanSink, PlanStore, Planner, ScopeClaims,
    ScopeDenial, WorkDirs,
};
pub use notify::{
    FocusTracker, IDLE_UNFOCUSED_MS, Moment, NoNotifier, Notification, Notifier, NotifyService,
    NotifySettings, QuietHours, TerminalMode, may_notify,
};
pub use palette::{
    Base16, CHART_ROLE, PaletteSource, THEME_ROLES, ThemeDef, hex, parse_hex, role_arity,
};
pub use prompts::{BUILTIN_VARIABLES, PromptService, RenderedPrompt};
pub use routing::{ROUTE_PROMPT_MAX, RouteReport, Routed, RoutingService};
pub use sendback::{SentBack, send_back_prompt};
pub use sessions::SessionService;
pub use settings::{
    BoardSettings, ConsoleSettings, FailMode, GovernorSettings, GuardSettings, JevProvider,
    JevSettings, LayoutSettings, MAX_SEND_BACK_NOTES, MotionSetting, OnUnreachable, ReviewSettings,
    ReviewWhen, RowSettings, Settings, TelemetrySettings, UiSettings, WorktreeSettings,
};
pub use telemetry::{
    ActivityView, AgentsPoll, Battery, CacheView, CiRun, Collected, CombinedStatusline, Commit,
    DiffStat, DiskUsage, ENVIRONMENTS, EnvironmentView, GitStatus, GithubStatus, InstructionsView,
    LimitView, ProcessStat, PullRequest, StatuslineFeed, StatuslineView, SystemStats, TICKER_LEN,
    TOP_FILES, TelemetryService, TelemetrySnapshot, TruthBoard,
};
pub use terminal::SessionTerminal;
pub use verifier::{CleanCheckout, CleanCopy, HoldoutStore, Verification, Verifier, VerifyRequest};
pub use watchdog::{
    CheckpointRestore, Diagnoser, Diagnosis, DiagnosisReply, DiagnosisRequest, DiagnosisSink,
    HookMessenger, NoMessenger, Nudge, NudgeBoard, NudgeGate, SessionMessenger, WatchReport,
    WatchStats, Watchdog, WatchdogSettings,
};
pub use workspace::{Rescan, ScannedRepo, WorkspaceScanner, WorkspaceService, WorkspaceStore};
pub use worktree::{
    AddWorktree, Added, GcReport, Prepared, PreparedSet, Removed, WorktreeService, WorktreeStore,
    WorktreeView, Worktrees,
};
