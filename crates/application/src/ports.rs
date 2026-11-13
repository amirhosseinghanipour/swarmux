//! Ports: what the use cases need from the outside world.

use std::path::{Path, PathBuf};

use swarmux_domain::telemetry::{Environment, RateLimit};
use swarmux_domain::{
    AgentEvent, Decision, Job, JobId, JobResult, JobStatus, Millis, PromptDraft, PromptId,
    PromptTemplate, ReviewState, Session, SessionId, StoredRule,
};

use crate::{AppResult, DecisionRecord, JobSummary};

pub trait SessionRepository {
    fn insert(&self, session: &Session) -> AppResult<()>;
    /// Non-archived sessions, most recently active first.
    fn list_active(&self) -> AppResult<Vec<Session>>;
    fn get(&self, id: SessionId) -> AppResult<Option<Session>>;
    fn touch(&self, id: SessionId, now: Millis) -> AppResult<()>;
    fn set_lean(&self, id: SessionId, lean: bool) -> AppResult<()>;
    /// `None` follows the folder: worktrees on in a git repository.
    fn set_worktree(&self, id: SessionId, worktree: Option<bool>) -> AppResult<()>;
    /// True once Claude Code has created the session (some job reported `init`).
    fn has_started(&self, id: SessionId) -> AppResult<bool>;
}

/// A job about to be stored; it starts `queued`.
#[derive(Clone, Debug, PartialEq)]
pub struct NewJob {
    pub session_id: SessionId,
    /// The template the prompt was rendered from, if any.
    pub prompt_id: Option<PromptId>,
    pub prompt: String,
    pub model: String,
    pub priority: i32,
    pub fork: bool,
    /// Have a reviewer audit the job once it succeeds.
    pub wants_review: bool,
    /// Set on a review job: the job it audits.
    pub review_of: Option<JobId>,
}

/// An event as stored for a job.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEvent {
    pub ts: Millis,
    pub kind: String,
    pub actor: String,
    pub payload: String,
}

pub trait JobRepository {
    fn insert(&self, job: &NewJob, now: Millis) -> AppResult<JobId>;
    fn get(&self, id: JobId) -> AppResult<Option<Job>>;
    /// Every `queued` job, in no particular order.
    fn queued(&self) -> AppResult<Vec<Job>>;
    fn set_status(&self, id: JobId, status: JobStatus, now: Millis) -> AppResult<()>;
    fn status(&self, id: JobId) -> AppResult<JobStatus>;
    fn set_priority(&self, id: JobId, priority: i32) -> AppResult<()>;
    /// Moves the job to another session (a fork's job belongs to the new session).
    fn set_session(&self, id: JobId, session: SessionId) -> AppResult<()>;
    fn save_result(&self, id: JobId, result: &JobResult) -> AppResult<()>;
    fn result(&self, id: JobId) -> AppResult<Option<JobResult>>;
    fn append_event(
        &self,
        id: JobId,
        kind: &str,
        actor: &str,
        payload: &str,
        now: Millis,
    ) -> AppResult<()>;
    /// Oldest first.
    fn events(&self, id: JobId) -> AppResult<Vec<StoredEvent>>;
    /// The newest `limit` events of the job stored at or before `until`,
    /// oldest first: the tail of the session stream as it stood at that
    /// moment. What the console quotes a held call's agent from.
    ///
    /// The default reads the whole stream; an adapter with an index
    /// should read only the tail, since a long job stores thousands of
    /// events.
    fn recent_events(&self, id: JobId, until: Millis, limit: usize) -> AppResult<Vec<StoredEvent>> {
        let mut all = self.events(id)?;
        all.retain(|e| e.ts <= until);
        let skip = all.len().saturating_sub(limit);
        Ok(all.split_off(skip))
    }
    /// Marks jobs left `running` (crash or quit) as `interrupted`. Returns how many.
    fn interrupt_running(&self, now: Millis) -> AppResult<usize>;
    /// The newest `limit` jobs, newest first.
    fn summaries(&self, limit: usize) -> AppResult<Vec<JobSummary>>;
    fn summary(&self, id: JobId) -> AppResult<Option<JobSummary>>;
    /// Stores where the job's review stands.
    fn set_review(&self, id: JobId, review: &ReviewState) -> AppResult<()>;
    /// `None` when no review was ever started for the job.
    fn review(&self, id: JobId) -> AppResult<Option<ReviewState>>;
}

pub trait PromptRepository {
    fn insert(&self, draft: &PromptDraft, now: Millis) -> AppResult<PromptId>;
    /// Fails with `PromptNotFound` if there is no such prompt.
    fn update(&self, id: PromptId, draft: &PromptDraft, now: Millis) -> AppResult<()>;
    /// Jobs rendered from the prompt keep their text but lose the link.
    fn delete(&self, id: PromptId) -> AppResult<()>;
    fn get(&self, id: PromptId) -> AppResult<Option<PromptTemplate>>;
    /// Most recently updated first.
    fn list(&self) -> AppResult<Vec<PromptTemplate>>;
}

/// The project folder a session works in.
pub trait Workspace {
    /// Uncommitted changes as `git diff` prints them; empty outside a git repository.
    fn git_diff(&self, cwd: &Path) -> AppResult<String>;
    /// The files [`Workspace::git_diff`] shows, with the lines git counted
    /// in each (`git diff --numstat`), so the counts and the text agree. An
    /// adapter with no way to count answers none: counts are never guessed.
    fn git_numstat(&self, _cwd: &Path) -> AppResult<Vec<swarmux_domain::evidence::DiffFile>> {
        Ok(Vec::new())
    }
}

/// What to run: one prompt in one Claude Code session.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRequest {
    pub job_id: JobId,
    pub cwd: PathBuf,
    pub session_id: SessionId,
    /// Continue the existing Claude session instead of creating it.
    pub resume: bool,
    /// With `resume`, branch into a new session.
    pub fork: bool,
    pub model: String,
    pub permission_mode: String,
    pub prompt: String,
    /// Load only project and local Claude Code settings, not the user's.
    pub lean: bool,
    /// A review run: read-only tools, no session file, the review system prompt.
    pub review: bool,
    /// Extra environment for the child (a worktree's port block, …).
    pub env: Vec<(String, String)>,
    /// Stop once the run has spent this many dollars (`--max-budget-usd`).
    pub max_budget_usd: Option<f64>,
    /// Register the `swarmux-board` MCP server for the run.
    pub board: bool,
    /// Folders beyond `cwd` the run works in, with their own instructions
    /// loaded: the repositories of a workspace job.
    pub add_dirs: Vec<PathBuf>,
}

/// What a running job reports back.
#[derive(Clone, Debug, PartialEq)]
pub enum RunnerMsg {
    Event(AgentEvent),
    Exited {
        code: Option<i32>,
        stderr_tail: String,
    },
    SpawnFailed(String),
}

/// Receives a job's messages. Called from the runner's own threads.
pub type RunSink = Box<dyn Fn(RunnerMsg) + Send + 'static>;

pub trait AgentRunner {
    /// Starts the job and returns at once; messages arrive through `sink`,
    /// ending with exactly one `Exited` or `SpawnFailed`.
    fn start(&self, request: RunRequest, sink: RunSink);
    /// Stops a started job. Its sink may receive nothing more afterwards.
    fn cancel(&self, job: JobId);
    /// The command line `request` runs with, for display.
    fn command_line(&self, _request: &RunRequest) -> Option<String> {
        None
    }
}

/// What a subagent definition (`.claude/agents/<type>.md`) says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentProfile {
    /// The `model:` front matter value ("sonnet", "haiku", "inherit", …).
    pub model: Option<String>,
    /// The `description:` front matter value.
    pub description: Option<String>,
}

/// Finds subagent definitions by type.
pub trait AgentCatalog {
    /// The definition for `agent_type`, looked up from the session folder
    /// `cwd` first, then the user's own agents. Unknown types give defaults.
    fn profile(&self, cwd: &Path, agent_type: &str) -> AgentProfile;
}

/// The machine swarmux runs on.
pub trait SystemMonitor {
    /// Memory available to new processes; `None` when it can't be read.
    fn free_memory_bytes(&self) -> Option<u64>;
    /// Running on battery rather than mains power; `None` when unknown.
    fn on_battery(&self) -> Option<bool> {
        None
    }
}

pub trait Clock {
    fn now(&self) -> Millis;
}

/// A tool call Claude Code asks swarmux about (its PreToolUse hook).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookRequest {
    /// Unique within this run.
    pub request_id: u64,
    /// The job whose `claude` made the call, when known.
    pub job_id: Option<JobId>,
    pub tool_name: String,
    /// What the call acts on: the command, file path or URL ("" if none).
    pub subject: String,
    /// The tool input as JSON, possibly shortened, for display.
    pub input: String,
    /// The folder the call was made from.
    pub cwd: String,
    /// The text a file tool writes (`content`, `new_string`, every edit's
    /// `new_string`), for knowledge rules; empty for other tools.
    pub content: String,
    /// The subagent that made the call, when Claude Code says which.
    pub agent_id: Option<String>,
    /// The kind of that subagent (`Explore`, `general-purpose`, ...).
    pub agent_type: Option<String>,
}

/// A hook request as it left the inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub request: HookRequest,
    /// Set when the transport already answered from the shared policy:
    /// the decision and how long deciding took, in microseconds.
    pub answered: Option<(Decision, u64)>,
}

/// Where hook requests arrive and their answers go back.
pub trait HookInbox {
    /// Requests received since the last call.
    fn poll(&self) -> Vec<InboxItem>;
    /// Answers a request that wasn't answered yet. Unknown ids are ignored.
    fn respond(&self, request_id: u64, decision: &Decision);
}

/// The guard's decision log.
pub trait DecisionRepository {
    fn record(&self, decision: &DecisionRecord) -> AppResult<()>;
    /// The newest `limit` decisions, newest first.
    fn recent_decisions(&self, limit: usize) -> AppResult<Vec<DecisionRecord>>;
}

/// The allow rules the user added while swarmux ran, with where they apply
/// and when they stop.
pub trait RuleStore {
    /// Every rule kept, expired ones included.
    fn list(&self) -> AppResult<Vec<StoredRule>>;
    fn add(&self, rule: &StoredRule) -> AppResult<()>;
    /// Removes the rule called `id`; `false` when there is none.
    fn remove(&self, id: &str) -> AppResult<bool>;
}

/// Which repository and workspace a call was made in.
pub trait RepoRoots: Send + Sync {
    /// The repository's own folder: for a worktree, the repository it was
    /// made from. `None` outside any repository.
    fn repo_root(&self, job: Option<JobId>, cwd: &str) -> Option<String>;
    /// The root of the workspace the job belongs to, when it does.
    fn workspace_root(&self, job: Option<JobId>, cwd: &str) -> Option<String>;
    /// The folder the call's own working tree starts at (a worktree's top).
    fn tree_root(&self, cwd: &str) -> Option<String>;
}

/// A hook call that fell back because swarmux could not be reached: the
/// hook process leaves a line, the app puts it in the ledger at its next
/// start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FallbackRecord {
    /// When the hook gave up, milliseconds since the epoch.
    pub ts: Millis,
    pub job: Option<JobId>,
    pub tool: String,
    /// SHA-256 of the hook input, when there was one.
    pub input_sha256: String,
    /// Why: `no socket`, `connect failed`, `no answer`, `unreadable input`.
    pub reason: String,
    /// What the hook printed: `deny`, `ask` or `pass`.
    pub mode: String,
    /// Identifies the line, so a line read twice is told.
    pub line_id: String,
}

/// Where the hook processes leave their fallbacks.
pub trait FallbackSpool {
    /// Every record in the spool.
    fn read(&self) -> AppResult<Vec<FallbackRecord>>;
    /// Empties the spool (after its records reached the ledger).
    fn clear(&self) -> AppResult<()>;
}

/// Saves guard rules the user adds while swarmux runs.
pub trait RuleWriter {
    /// Adds `rule` to the allow list of the user's settings.
    fn add_allow_rule(&self, rule: &str) -> AppResult<()>;
}

/// A session the router may pick.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteCandidate {
    pub name: String,
    pub cwd: String,
    pub model: String,
    /// Previews of its latest prompts, newest first.
    pub recent: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteRequest {
    /// The start of the prompt.
    pub prompt: String,
    pub candidates: Vec<RouteCandidate>,
}

/// The router's pick.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteAnswer {
    /// A candidate name.
    pub session: String,
    /// "haiku", "sonnet" or "opus", when the router suggests one.
    pub model: Option<String>,
    pub reason: String,
}

/// A router's answer (or why there is none) and what asking cost.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteReply {
    pub result: Result<RouteAnswer, String>,
    pub cost_usd: f64,
}

/// Receives a router's reply, from any thread.
pub type RouteSink = Box<dyn FnOnce(RouteReply) + Send + 'static>;

/// Picks the session a prompt should go to. Never blocks; the reply arrives
/// through the sink.
pub trait PromptRouter {
    fn route(&self, request: RouteRequest, sink: RouteSink);
}

/// Machine-wide telemetry collected in the background (transcripts, system,
/// git, GitHub). Both calls are cheap and never block on a collector.
pub trait TelemetryFeed {
    /// The latest data of every source. Job-derived parts are left empty;
    /// [`crate::TelemetryService`] fills them in.
    fn snapshot(&self) -> crate::TelemetrySnapshot;
    /// The folder the git and GitHub sources follow (the selected session's).
    fn focus(&self, cwd: &Path);
}

/// Keeps usage-limit readings and session environments across runs.
pub trait TelemetryStore {
    /// Replaces the stored reading of the limit's window.
    fn save_limit(&self, limit: &RateLimit, observed_at: Millis) -> AppResult<()>;
    /// The latest reading of every window.
    fn limits(&self) -> AppResult<Vec<(RateLimit, Millis)>>;
    /// Replaces the stored environment of the session.
    fn save_environment(&self, env: &Environment, seen_at: Millis) -> AppResult<()>;
    /// The latest environment of every session, newest first.
    fn environments(&self, limit: usize) -> AppResult<Vec<(Environment, Millis)>>;
}
