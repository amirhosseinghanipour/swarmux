//! The console's view models: plain data, no terminal types.
//!
//! Every client (the terminal screens, a daemon API, a phone app) reads
//! these. The JSON shape is a contract, written down here:
//!
//! - [`SCHEMA_VERSION`] says which shape this is. Adding a field does not
//!   change it: clients must ignore fields they do not know. Renaming,
//!   removing or re-typing a field, or changing what an enum word means,
//!   does, and the golden tests in `console/tests.rs` fail until the
//!   change is made on purpose.
//! - Field and enum names are `snake_case`, spelled out here with
//!   `rename_all`, so renaming a Rust type or variant never changes the
//!   wire.
//! - A missing value is `null`, always present: clients see a stable set
//!   of keys. Lists are present and may be empty.
//! - Times are unix milliseconds (`*_at`), durations milliseconds
//!   (`*_ms`), money US dollars (`*_usd`), shares `0.0..=1.0`.
//! - Ids are strings that stay the same while the thing lives:
//!   `mission:<slug>`, `job:<n>`, `task:<slug>:<task>`, `hold:<request>`.
//! - The enums mirror the domain's, one by one (see the `From` impls), so
//!   the words on the wire are chosen here and not by a refactor there.
//!
//! What is deliberately not here, because nothing backs it today: the
//! cost of delay, intent inference, per-file accept or revert, pause,
//! merge to main.
//!
//! # Added since the first shape (still schema 1)
//!
//! These were added as optional or empty-by-default fields, which the
//! rules above say do not change [`SCHEMA_VERSION`]: a client written
//! before them ignores what it does not know, and the golden fixtures were
//! updated on purpose.
//!
//! - [`OutcomeView::files`], [`DriftView::outside_files`] and
//!   [`PlanTaskView::changed`]: the files of a change with the lines git
//!   counted in each. Empty when no count was kept (a bundle sealed
//!   before they were, a change compared by name only): then only the
//!   totals ([`DiffView`]) exist, and no count is worked out after the
//!   fact.
//! - [`ContractView::due`]: the mission's due time, `null` when the
//!   contract has none.
//! - [`ActionKind::SendBack`]: send the work back to the agent with a note,
//!   or deny a held call with one.
//! - [`ActionKind::Intervene`]: tell a running job's agent something, in
//!   the mode (`queue` or `steer`) the client picks; disabled, with the
//!   reason, for a job that is not running, not followed by this swarmux, or
//!   one task of a graph. A client sends it with [`ReceiptView`]'s call
//!   (`Engine::intervene`) and shows the receipt it returns.
//! - [`ConsoleSnapshot::metrics`]: the verification figures.

use serde::{Deserialize, Serialize};
use swarmux_domain::console as rules;
use swarmux_domain::{Millis, Priority};

/// The shape of everything in this module. Bump it on a breaking change.
pub const SCHEMA_VERSION: u32 = 1;

/// Mirrors a domain enum word for word: the wire enum is defined here, the
/// mapping is checked by the compiler.
macro_rules! mirror {
    ($wire:ident <= $domain:ty { $($variant:ident),+ $(,)? }) => {
        impl From<$domain> for $wire {
            fn from(d: $domain) -> $wire {
                match d { $( <$domain>::$variant => $wire::$variant ),+ }
            }
        }
    };
}

// ---------------------------------------------------------------------
// The snapshot
// ---------------------------------------------------------------------

/// Everything the Work and Deck screens read, at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsoleSnapshot {
    pub schema: u32,
    /// Counts the rebuilds of this process: a client that has seen a
    /// revision need not read the rest again.
    pub revision: u64,
    pub taken_at: Millis,
    pub header: Header,
    /// Always the four groups, in the order they are shown.
    pub groups: Vec<GroupView>,
    /// Calls held for the user (and recent denials), most urgent first.
    pub holds: Vec<HoldView>,
    pub deck: DeckGraph,
    /// The verification figures of the last hours, from the ledger; `null`
    /// where no ledger is read.
    pub metrics: Option<MetricsView>,
}

/// How verification has gone: the gate's passes and how long claims took
/// to be verified, from the ledger's entries. A figure there is nothing to
/// derive from is `null`, never zero. The definitions are those of
/// `swarmux_domain::console::compute_metrics`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricsView {
    /// The window's end.
    pub at: Millis,
    /// The window asked for, and how much of it the ledger covers.
    pub window_ms: u64,
    pub covered_ms: u64,
    /// The whole window was read; false when the read was cut short and
    /// the figures describe only its newest part.
    pub complete: bool,
    /// Gate passes at the first claim.
    pub first_run: Option<FirstRunView>,
    /// From the first claim to the pass.
    pub claim_to_verified: Option<SpreadView>,
    /// Jobs the gate passed in the window.
    pub verified: u32,
    pub verified_per_hour: Option<f64>,
    pub checks_run: u32,
    pub bundles_sealed: u32,
}

/// First-run gate passes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FirstRunView {
    pub passed: u32,
    /// Jobs whose first judged claim the window saw.
    pub judged: u32,
    /// `passed / judged`, `0.0..=1.0`.
    pub rate: f64,
}

/// Median and 90th percentile of how long claims took to be verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadView {
    pub median_ms: u64,
    pub p90_ms: u64,
    pub samples: u32,
}

impl From<&swarmux_domain::console::Metrics> for MetricsView {
    fn from(m: &swarmux_domain::console::Metrics) -> MetricsView {
        let ms = |x: Millis| u64::try_from(x).unwrap_or(0);
        MetricsView {
            at: m.at,
            window_ms: ms(m.window_ms),
            covered_ms: ms(m.covered_ms),
            complete: m.complete,
            first_run: m.first_run.map(|f| FirstRunView {
                passed: f.passed,
                judged: f.judged,
                rate: f.rate(),
            }),
            claim_to_verified: m.claim_to_verified.map(|s| SpreadView {
                median_ms: s.median_ms,
                p90_ms: s.p90_ms,
                samples: s.samples,
            }),
            verified: m.verified,
            verified_per_hour: m.verified_per_hour,
            checks_run: m.checks_run,
            bundles_sealed: m.bundles_sealed,
        }
    }
}

/// The Work queue's groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    Decide,
    ReadyToLand,
    Running,
    Landed,
}
mirror!(
    GroupKind
        <= rules::Group {
            Decide,
            ReadyToLand,
            Running,
            Landed
        }
);

/// One group of the queue, ranked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupView {
    pub kind: GroupKind,
    /// How many outcomes belong to the group; `outcomes` holds the first
    /// ones (the landed group is cut to its newest).
    pub total: usize,
    pub outcomes: Vec<OutcomeView>,
}

/// The figures above the queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub counts: Counts,
    pub queue: QueueFigures,
    /// Usage windows, shortest first, with the pace's forecast.
    pub limits: Vec<LimitWindow>,
    pub spend: SpendFigures,
}

/// Outcomes per group, and the ones on no queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub decide: usize,
    pub ready_to_land: usize,
    pub running: usize,
    pub landed: usize,
    /// Outcomes of this run on no queue: finished plain jobs that nothing
    /// asks about.
    pub idle: usize,
}

/// What the scheduler is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueFigures {
    pub running: usize,
    pub queued: usize,
    pub max_concurrent: usize,
    /// Queued jobs the governor holds until a usage window resets.
    pub deferred: usize,
    /// Stopped by a usage limit; they resume by themselves after the reset.
    pub awaiting_reset: usize,
    /// Held back because the user is working the inbox as much as they can.
    pub held_for_attention: usize,
}

/// One usage window (`five_hour`, `seven_day`, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    pub window: String,
    pub status: String,
    /// `0.0..=1.0`, when Claude Code reported it.
    pub utilization: Option<f64>,
    pub resets_at: Option<Millis>,
    /// When the current pace reaches the limit, if before the reset.
    pub hits_at: Option<Millis>,
    /// The window has reset since the reading.
    pub stale: bool,
}

/// What things cost.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendFigures {
    /// Across every Claude Code session on the machine, when read.
    pub today_usd: Option<f64>,
    pub week_usd: Option<f64>,
    /// What the missions on the queue spent (all phases).
    pub missions_usd: f64,
    /// What the jobs of this run cost.
    pub jobs_usd: f64,
}

// ---------------------------------------------------------------------
// One outcome
// ---------------------------------------------------------------------

/// A mission, or a plain job when it has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Mission,
    Job,
}

/// Where an outcome is in its life: the mission's state, or for a plain
/// job its job's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Draft,
    Linted,
    Queued,
    Running,
    Verifying,
    Delivered,
    Escalated,
    Refused,
    Done,
    Failed,
    Interrupted,
    Cancelled,
}

/// How urgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityKind {
    P1,
    P2,
    P3,
}

impl From<Priority> for PriorityKind {
    fn from(p: Priority) -> PriorityKind {
        match p {
            Priority::P1 => PriorityKind::P1,
            Priority::P2 => PriorityKind::P2,
            Priority::P3 => PriorityKind::P3,
        }
    }
}

/// Everything the Work screen shows about one outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutcomeView {
    /// `mission:<slug>` or `job:<n>`.
    pub id: String,
    pub kind: OutcomeKind,
    pub title: String,
    /// The mission's slug.
    pub mission: Option<String>,
    pub lifecycle: Lifecycle,
    pub group: GroupKind,
    /// How urgent, for the decisions.
    pub priority: Option<PriorityKind>,
    /// When it entered its present stage (the age the group orders by).
    pub since_at: Millis,
    pub created_at: Millis,
    pub updated_at: Millis,
    /// What needs the user.
    pub asks: Vec<AskView>,
    /// Calls held for the user, by id (see [`ConsoleSnapshot::holds`]).
    pub holds: Vec<String>,
    /// The contract; `None` for a plain job.
    pub contract: Option<ContractView>,
    pub jobs: Vec<JobView>,
    pub plan: Option<PlanView>,
    pub checks: ChecksView,
    /// What the agent says it did.
    pub claim: Option<ClaimView>,
    /// The sealed record; `None` until the gate passes.
    pub evidence: Option<EvidenceView>,
    /// Totals of the change; `None` until the evidence is sealed.
    pub diff: Option<DiffView>,
    /// The files of the change with the lines git counted in each: from
    /// the sealed bundle, else from what was changed when the build was
    /// checked against its plan. Empty when neither kept them; the totals
    /// of [`OutcomeView::diff`] then stand alone.
    pub files: Vec<DiffFileView>,
    /// What landing does or did; `None` where nothing lands (a plain job,
    /// or a mission worked in one folder).
    pub landing: Option<LandingView>,
    pub spend: SpendView,
    /// Every action, enabled or not.
    pub actions: Vec<ActionView>,
}

/// One open ask of the attention inbox.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskView {
    /// The inbox item's id (`approval:12`, `job:3:failed`, `plan:slug`).
    pub id: String,
    pub kind: AskKind,
    pub priority: PriorityKind,
    pub title: String,
    pub detail: String,
    pub since_at: Millis,
    /// When it resolves without the user: a held call is denied.
    pub deadline_at: Option<Millis>,
    /// The held call's request, for an approval.
    pub request: Option<u64>,
    /// Whether it puts the outcome among the decisions.
    pub decides: bool,
}

/// Why something is in the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    Approval,
    Question,
    JobFailed,
    ReviewConcerns,
    ResultReady,
    Stalled,
    GateEscalated,
    Recovery,
    PlanReady,
    Drift,
    Merge,
    BoardQuestion,
    FollowUp,
    Knowledge,
}
mirror!(
    AskKind
        <= swarmux_domain::AttentionKind {
            Approval,
            Question,
            JobFailed,
            ReviewConcerns,
            ResultReady,
            Stalled,
            GateEscalated,
            Recovery,
            PlanReady,
            Drift,
            Merge,
            BoardQuestion,
            FollowUp,
            Knowledge,
        }
);

/// The mission contract, as written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContractView {
    pub goal: String,
    /// The repositories it touches, producers first.
    pub repos: Vec<String>,
    /// The checks that must pass.
    pub done_when: Vec<String>,
    /// A person signs off as well.
    pub human_signoff: bool,
    /// Globs the agent may never write.
    pub protected: Vec<String>,
    /// Globs the agent is meant to work in; empty is anywhere.
    pub work_in: Vec<String>,
    pub budget: BudgetView,
    /// When it is needed by, if the contract says so. Never made up.
    pub due: Option<Millis>,
    /// Why it was escalated or refused.
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BudgetView {
    pub usd: Option<f64>,
    pub wall_minutes: Option<u32>,
    pub on_exhaust: OnExhaust,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExhaust {
    Escalate,
    Warn,
}
mirror!(OnExhaust <= swarmux_domain::mission::OnExhaust { Escalate, Warn });

/// A job of the outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    pub id: i64,
    /// The folder the job's session works in (the repository's name).
    pub repo: String,
    pub session: String,
    pub model: String,
    pub state: JobState,
    /// The plan task it does, in a task graph.
    pub task: Option<String>,
    pub spend_usd: f64,
    /// Share of the model's context window the lead has used, `0.0..`.
    pub context_use: Option<f64>,
    pub started_at: Option<Millis>,
    pub finished_at: Option<Millis>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Interrupted,
    Cancelled,
}
mirror!(
    JobState
        <= swarmux_domain::JobStatus {
            Queued,
            Running,
            Done,
            Failed,
            Interrupted,
            Cancelled,
        }
);

// ---------------------------------------------------------------------
// The plan and its drift
// ---------------------------------------------------------------------

/// The approved (or proposed) plan, and how the build matches it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanView {
    pub status: PlanStatus,
    pub summary: String,
    pub tasks: Vec<PlanTaskView>,
    /// Plan against change, once the build was checked.
    pub drift: Option<DriftView>,
    /// How the tasks run: `single` or `fanout`; `None` before the build.
    pub topology: Option<String>,
    /// Plans asked for so far.
    pub rounds: u32,
    /// The user's note on the last rejection, or why planning failed.
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Idle,
    Planning,
    Proposed,
    Approved,
    Failed,
}
mirror!(
    PlanStatus
        <= swarmux_domain::plan::PlanStatus {
            Idle,
            Planning,
            Proposed,
            Approved,
            Failed,
        }
);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanTaskView {
    pub id: String,
    pub title: String,
    /// The files and areas it owns.
    pub files: Vec<String>,
    pub depends_on: Vec<String>,
    pub acceptance: String,
    /// Where it stands, once the plan runs as a task graph.
    pub state: Option<TaskState>,
    pub job: Option<i64>,
    /// The changed files inside the task's areas, with their lines; empty
    /// when no count was kept or nothing changed there.
    pub changed: Vec<DiffFileView>,
}

/// A task of a task graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Waiting,
    Running,
    Gate,
    Merging,
    Merged,
    Conflict,
    Failed,
}
mirror!(
    TaskState
        <= swarmux_domain::taskgraph::TaskState {
            Waiting,
            Running,
            Gate,
            Merging,
            Merged,
            Conflict,
            Failed,
        }
);

/// How what was built strays from the plan (`check_drift`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftView {
    /// Files changed, not counting lockfiles.
    pub changed: u32,
    /// Changed files outside every planned area (the first few).
    pub outside: Vec<String>,
    pub outside_count: u32,
    /// The files outside every planned area with their lines; empty when no
    /// count was kept.
    pub outside_files: Vec<DiffFileView>,
    pub tasks: u32,
    /// Ids of the planned tasks no change touched.
    pub untouched: Vec<String>,
    /// The larger of the two shares.
    pub score: f64,
    pub threshold: f64,
    pub drifted: bool,
    pub summary: String,
}

// ---------------------------------------------------------------------
// Checks, gate stages, the agent's claim
// ---------------------------------------------------------------------

/// What judged the work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChecksView {
    /// Where these figures come from: the gate as it ran this session, or
    /// the sealed bundle.
    pub source: ChecksSource,
    pub runs: Vec<CheckRunView>,
    /// Configured checks (`done_when`) with a result so far.
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    /// The gate's latest verdict.
    pub gate: Option<GateView>,
    /// The stages beyond the checks themselves, in the order they run.
    pub stages: Vec<StageView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksSource {
    /// Nothing ran yet.
    None,
    Gate,
    Bundle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckRunView {
    pub name: String,
    pub status: CheckStatus,
    pub duration_ms: u64,
    /// The first failing line, verbatim; only for a check that failed (for
    /// one that could not start, why it could not).
    pub first_failure: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    /// Failed, then passed when run again.
    Flaky,
    Failed,
    TimedOut,
    /// It could not start.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateView {
    pub verdict: GateVerdict,
    /// Stops the gate has blocked so far.
    pub blocks: u32,
    /// How many it blocks before it gives up; not kept in a bundle.
    pub max_blocks: Option<u32>,
    pub at: Millis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    Pass,
    Block,
    Escalate,
}
mirror!(
    GateVerdict
        <= swarmux_domain::gate::GateOutcome {
            Pass,
            Block,
            Escalate
        }
);

/// One stage of the gate beyond the checks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageView {
    pub key: StageKey,
    pub state: StageState,
    /// One line.
    pub summary: String,
    /// What it found: files, lines, concerns.
    pub items: Vec<String>,
    /// Hidden checks only: how many ran and passed. Never which.
    pub ran: Option<u32>,
    pub passed: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKey {
    /// Protected files pinned at the start still match.
    ProtectedFiles,
    /// No test was weakened to pass.
    TestWeakening,
    /// The checks pass against the original tests.
    BaseSuite,
    /// The checks the agent cannot see.
    HiddenChecks,
    /// What the agent claims it ran, that nothing backs.
    EvidenceGaps,
    /// The build against the approved plan.
    Drift,
    /// The reviewer's verdict.
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    Passed,
    /// The gate counts it against the work.
    Failed,
    /// Reported, and does not stop the work (drift, a reviewer's concerns).
    Flagged,
    NotRun,
}

/// What the agent says it did: the final answer of its job, as it wrote it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimView {
    pub job: i64,
    pub text: String,
    /// The text was cut.
    pub truncated: bool,
}

// ---------------------------------------------------------------------
// Evidence, diff, landing
// ---------------------------------------------------------------------

/// Where the record of a delivered mission is, and whether it checks out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceView {
    /// The bundle's digest, hex.
    pub bundle_hash: String,
    pub sealed_at: Millis,
    /// The ledger entries that tell the story.
    pub seq_from: u64,
    pub seq_to: u64,
    /// The hash of the last of them.
    pub head_hash: String,
    /// The entry that recorded the sealing.
    pub seal_seq: Option<u64>,
    pub signature: SignatureState,
    /// A signed checkpoint of the ledger covers the bundle.
    pub anchored: bool,
}

/// Whether the bundle's signature matches its content (a local check; the
/// ledger chain is verified by `swarmux evidence verify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureState {
    Valid,
    Invalid,
    Unsigned,
    /// Signed with an algorithm this version cannot check.
    Unchecked,
}

/// What changed, in totals. The files behind them are
/// [`OutcomeView::files`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiffView {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
    /// The digest of the diff.
    pub hash: String,
}

/// One file of a change, with the lines git counted in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFileView {
    /// Relative to the repository, with `/`.
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
    /// git counts no lines of a binary file: 0 and 0 there do not mean
    /// nothing changed.
    pub binary: bool,
    /// How it changed, when git said.
    pub change: Option<ChangeKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}
mirror!(
    ChangeKind
        <= swarmux_domain::evidence::ChangeKind {
            Added,
            Modified,
            Deleted
        }
);

impl From<&swarmux_domain::evidence::DiffFile> for DiffFileView {
    fn from(f: &swarmux_domain::evidence::DiffFile) -> DiffFileView {
        DiffFileView {
            path: f.path.clone(),
            insertions: f.insertions,
            deletions: f.deletions,
            binary: f.binary,
            change: f.kind.map(ChangeKind::from),
        }
    }
}

/// What landing does, or did: each repository's branch merged into the
/// mission's integration branch, producers first, all or none. Never
/// into the user's own branches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandingView {
    pub mode: LandingMode,
    /// `swarmux/<mission>/integration`.
    pub into: String,
    /// The repositories, in the order they land.
    pub repos: Vec<LandingRepoView>,
    /// A task graph's merge queue: tasks that passed, in the order they
    /// merge.
    pub queue: Vec<String>,
    /// Every repository landed.
    pub complete: bool,
    pub summary: String,
    pub at: Option<Millis>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandingMode {
    /// Nothing landed yet: this is what would happen.
    Preview,
    /// This is what happened.
    Result,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandingRepoView {
    pub repo: String,
    /// The job's branch in that repository, once known.
    pub branch: Option<String>,
    pub state: LandState,
    /// The merge commit, once landed.
    pub commit: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandState {
    Waiting,
    Landed,
    Failed,
    Undone,
    Skipped,
}
mirror!(
    LandState
        <= swarmux_domain::mission::LandState {
            Waiting,
            Landed,
            Failed,
            Undone,
            Skipped,
        }
);

/// What the outcome has cost.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendView {
    pub total_usd: f64,
    /// By phase, for a mission.
    pub plan_usd: f64,
    pub implement_usd: f64,
    pub verify_usd: f64,
    pub review_usd: f64,
}

// ---------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------

/// An action, and whether it can be taken now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionView {
    pub kind: ActionKind,
    pub enabled: bool,
    /// Why not, when it is disabled.
    pub reason: Option<String>,
    /// The job it acts on.
    pub job: Option<i64>,
    /// The mission it acts on.
    pub mission: Option<String>,
    /// The held call it answers.
    pub request: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Kill,
    Retry,
    Resume,
    LandToIntegration,
    OpenEvidence,
    AnswerHeldCall,
    ApprovePlan,
    RejectPlan,
    SendBack,
    Intervene,
}
mirror!(
    ActionKind
        <= rules::ActionKind {
            Kill,
            Retry,
            Resume,
            LandToIntegration,
            OpenEvidence,
            AnswerHeldCall,
            ApprovePlan,
            RejectPlan,
            SendBack,
            Intervene,
        }
);

/// What a client is told about a message to a running agent: the receipt
/// `Engine::intervene` returns, and the same record later as it settles.
/// `state` is `queued`, `handed_over`, `delivered`, `refused` or
/// `undelivered`; `reason` says why for the last two (and `summary` says
/// it in a sentence). `delivered_at` is set only once the agent's turn with
/// the message was seen to start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReceiptView {
    /// 0 for a message that was refused: it was never numbered.
    pub id: u64,
    pub job: i64,
    /// `queue` or `steer`.
    pub mode: String,
    pub state: String,
    pub summary: String,
    pub text: String,
    pub at: Millis,
    pub reason: Option<String>,
    /// The job the agent carries on as, once the message is handed over.
    pub resumed_as: Option<i64>,
    pub delivered_at: Option<Millis>,
    /// The ledger entry that records the message.
    pub seq: Option<u64>,
}

impl From<&swarmux_domain::intervene::Receipt> for ReceiptView {
    fn from(r: &swarmux_domain::intervene::Receipt) -> ReceiptView {
        use swarmux_domain::intervene::State;
        let (reason, resumed_as, delivered_at) = match &r.state {
            State::Queued => (None, None, None),
            State::Handed { job } => (None, Some(job.0), None),
            State::Delivered { job, at } => (None, Some(job.0), Some(*at)),
            State::Refused(why) => (Some(why.reason()), None, None),
            State::Undelivered(why) => (Some(why.clone()), None, None),
        };
        ReceiptView {
            id: r.id,
            job: r.job.0,
            mode: r.mode.as_str().to_string(),
            state: r.state.as_str().to_string(),
            summary: r.summary(),
            text: r.text.clone(),
            at: r.at,
            reason,
            resumed_as,
            delivered_at,
            seq: r.seq,
        }
    }
}

// ---------------------------------------------------------------------
// Guard holds
// ---------------------------------------------------------------------

/// A tool call the Guard held for the user, or refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldView {
    /// `hold:<request>` for a held call, `denied:<n>` for a refusal.
    pub id: String,
    pub state: HoldState,
    /// The held call's request.
    pub request: Option<u64>,
    /// The rule that held or refused it, as the decision log names it.
    /// `null` when no rule matched and the Guard asked by default.
    pub rule: Option<String>,
    /// The knowledge rule that asked, when one did.
    pub knowledge_rule: Option<String>,
    pub tool: String,
    /// What the call acts on: the command, file path or URL.
    pub target: String,
    pub cwd: Option<String>,
    pub risk: Option<RiskView>,
    pub priority: Option<PriorityKind>,
    /// Naming the target is needed to allow it.
    pub destructive: bool,
    /// Who decided.
    pub decided_by: DecisionSource,
    /// Only for a model's answer that came with one.
    pub confidence: Option<f64>,
    /// When the call was made.
    pub at: Millis,
    /// When a held call is denied for lack of an answer.
    pub deadline_at: Option<Millis>,
    pub job: Option<i64>,
    /// The outcome it belongs to.
    pub outcome: Option<String>,
    pub session: Option<String>,
    /// The text the session wrote just before the call, quoted from its
    /// stored stream. Never inferred.
    pub agent_words: Option<AgentWords>,
    /// The last steps of the job up to the call, oldest first.
    pub steps: Vec<StepView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldState {
    /// Waiting for the user.
    Held,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskView {
    pub tier: RiskTier,
    /// `0.0..=1.0`.
    pub score: f64,
    /// The call may be sending something secret out.
    pub secret: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    Routine,
    Elevated,
    Destructive,
}
mirror!(
    RiskTier
        <= swarmux_domain::alarm::RiskTier {
            Routine,
            Elevated,
            Destructive
        }
);

/// Who made a decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    Guard,
    Loop,
    /// Legacy: made by the hosted JEV model, which was removed. Never sent
    /// for new decisions; the word stays so history made with it reads.
    Jev,
    /// Legacy: JEV was asked and gave no usable answer. See `Jev`.
    JevError,
    Haiku,
    HaikuError,
    Fallback,
}
mirror!(
    DecisionSource
        <= crate::Source {
            Guard,
            Loop,
            Jev,
            JevError,
            Haiku,
            HaikuError,
            Fallback,
        }
);

/// The agent's own words, quoted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentWords {
    /// What this is, for a client that labels it: the agent's own words,
    /// as stored in its session stream.
    pub label: String,
    pub text: String,
    pub at: Millis,
    /// The quote was cut.
    pub truncated: bool,
    /// Steps between the words and the call.
    pub steps_since: u32,
}

/// One step of a job, as the Focus action stream shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepView {
    pub at: Millis,
    pub kind: StepKind,
    /// The lead, or the id of a subagent.
    pub actor: String,
    pub text: String,
    /// For a tool result: whether it succeeded.
    pub ok: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Text,
    ToolCall,
    ToolResult,
}

// ---------------------------------------------------------------------
// The Deck's graph
// ---------------------------------------------------------------------

/// The cards and what relates them. No layout: `depth` orders producers
/// before consumers and a client places the rest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeckGraph {
    pub nodes: Vec<DeckNode>,
    pub edges: Vec<DeckEdge>,
    /// The deepest card's depth.
    pub max_depth: u32,
}

/// One card: a plain job, a mission worked as one job, or one task of a
/// task graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeckNode {
    /// An outcome's id, or `task:<slug>:<task>` for a task of a graph.
    pub id: String,
    pub kind: NodeKind,
    /// The outcome it belongs to.
    pub outcome: String,
    pub title: String,
    pub repo: Option<String>,
    pub state: NodeState,
    /// The queue group of its outcome; `None` on no queue.
    pub group: Option<GroupKind>,
    pub checks: CheckProgress,
    /// How many producers sit between it and the start of its chain.
    pub depth: u32,
    pub spend_usd: f64,
    pub job: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Job,
    Task,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// Has not started: it waits for something.
    Waiting,
    Queued,
    Running,
    /// The job ended; the gate's verdict is awaited.
    Verifying,
    /// The gate passed; the work waits to land.
    Ready,
    /// Merged or delivered.
    Done,
    /// Needs the user.
    Failed,
    /// Cancelled before it finished.
    Stopped,
}

/// How far the checks are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckProgress {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeckEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub state: DepState,
    /// What it is about: the contract's symbols, the claimed areas.
    pub detail: Vec<String>,
}

/// What relates two cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A plan's task depends on another.
    TaskDependency,
    /// A repository provides what another uses.
    ContractLink,
    /// Two cards claim overlapping files; symmetric, `from` holds them.
    FileClaim,
}
mirror!(
    EdgeKind
        <= rules::EdgeKind {
            TaskDependency,
            ContractLink,
            FileClaim
        }
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepState {
    Met,
    Pending,
    Blocked,
}
mirror!(
    DepState
        <= rules::DepState {
            Met,
            Pending,
            Blocked
        }
);
