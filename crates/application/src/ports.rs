//! Ports: what the use cases need from the outside world.

use std::path::{Path, PathBuf};

use swarmux_domain::{
    AgentEvent, Decision, Job, JobId, JobResult, JobStatus, Millis, PromptDraft, PromptId,
    PromptTemplate, ReviewState, Session, SessionId,
};

use crate::{AppResult, DecisionRecord, JobSummary};

pub trait SessionRepository {
    fn insert(&self, session: &Session) -> AppResult<()>;
    /// Non-archived sessions, most recently active first.
    fn list_active(&self) -> AppResult<Vec<Session>>;
    fn get(&self, id: SessionId) -> AppResult<Option<Session>>;
    fn touch(&self, id: SessionId, now: Millis) -> AppResult<()>;
    fn set_lean(&self, id: SessionId, lean: bool) -> AppResult<()>;
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
}

pub trait Clock {
    fn now(&self) -> Millis;
}

/// A tool call Claude Code asks swarmux about (its PreToolUse hook).
#[derive(Clone, Debug, PartialEq)]
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

/// Saves guard rules the user adds while swarmux runs.
pub trait RuleWriter {
    /// Adds `rule` to the allow list of the user's settings.
    fn add_allow_rule(&self, rule: &str) -> AppResult<()>;
}

/// A fuzzy decision put to a hosted decision model (JEV). Texts are already
/// shortened to what may leave the machine.
#[derive(Clone, Debug, PartialEq)]
pub enum AdviceRequest {
    /// Should a tool call no rule matched run without asking the user?
    ToolCall { tool: String, summary: String },
    /// Which of these sessions should take the prompt?
    Session {
        prompt: String,
        candidates: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AdviceAnswer {
    Allow,
    Deny,
    /// One of the candidate session names.
    Session(String),
}

/// A typed answer and how sure the model is (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Advice {
    pub answer: AdviceAnswer,
    pub confidence: f64,
}

/// Why no advice came back. Messages never contain the API key.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AdviceError {
    #[error("no {0} API key: run `swarmux jev set-key {0}`")]
    NoKey(String),
    #[error("no answer within {0} ms")]
    Timeout(u64),
    #[error("HTTP {0}")]
    Status(u16),
    #[error("request failed: {0}")]
    Network(String),
    #[error("invalid reply: {0}")]
    Invalid(String),
}

/// An advisor's answer and how long it took.
#[derive(Clone, Debug, PartialEq)]
pub struct AdviceReply {
    pub result: Result<Advice, AdviceError>,
    pub latency_ms: u64,
}

/// Receives an advisor's reply, from any thread.
pub type AdviceSink = Box<dyn FnOnce(AdviceReply) + Send + 'static>;

/// A hosted decision model for fuzzy forks. Answers always arrive through
/// the sink, bounded by the provider's timeout; `advise` itself never blocks.
pub trait DecisionAdvisor {
    /// Provider name, e.g. "typesafe".
    fn provider(&self) -> &str;
    /// The host requests go to, for the consent screen.
    fn host(&self) -> &str;
    fn has_key(&self) -> bool;
    fn advise(&self, request: AdviceRequest, sink: AdviceSink);
}

/// Remembers which JEV providers the user agreed to send data to.
pub trait ConsentStore {
    fn has_consent(&self, provider: &str) -> AppResult<bool>;
    fn grant_consent(&self, provider: &str, now: Millis) -> AppResult<()>;
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
