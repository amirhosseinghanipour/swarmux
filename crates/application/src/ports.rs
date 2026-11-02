//! Ports: what the use cases need from the outside world.

use std::path::PathBuf;

use swarmux_domain::{AgentEvent, JobId, JobResult, JobStatus, Millis, Session, SessionId};

use crate::AppResult;

pub trait SessionRepository {
    fn insert(&self, session: &Session) -> AppResult<()>;
    /// Non-archived sessions, most recently active first.
    fn list_active(&self) -> AppResult<Vec<Session>>;
    fn get(&self, id: SessionId) -> AppResult<Option<Session>>;
    fn touch(&self, id: SessionId, now: Millis) -> AppResult<()>;
    /// True once Claude Code has created the session (some job reported `init`).
    fn has_started(&self, id: SessionId) -> AppResult<bool>;
}

pub trait JobRepository {
    fn insert(
        &self,
        session: SessionId,
        prompt: &str,
        model: &str,
        now: Millis,
    ) -> AppResult<JobId>;
    fn set_status(&self, id: JobId, status: JobStatus, now: Millis) -> AppResult<()>;
    fn status(&self, id: JobId) -> AppResult<JobStatus>;
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
    /// Marks jobs left `running` (crash or quit) as `interrupted`. Returns how many.
    fn interrupt_running(&self, now: Millis) -> AppResult<usize>;
}

/// What to run: one prompt in one Claude Code session.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRequest {
    pub cwd: PathBuf,
    pub session_id: SessionId,
    /// Continue the existing Claude session instead of creating it.
    pub resume: bool,
    /// With `resume`, branch into a new session.
    pub fork: bool,
    pub model: String,
    pub permission_mode: String,
    pub prompt: String,
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
}

pub trait Clock {
    fn now(&self) -> Millis;
}
