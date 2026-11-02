//! In-memory fakes of every port (feature `test-support`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use swarmux_domain::{
    Job, JobId, JobResult, JobStatus, Millis, PromptDraft, PromptId, PromptTemplate, Session,
    SessionId,
};

use crate::ports::{
    AgentRunner, Clock, JobRepository, NewJob, PromptRepository, RunRequest, RunSink, RunnerMsg,
    SessionRepository, StoredEvent, SystemMonitor, Workspace,
};
use crate::{AppError, AppResult, JobSummary};

/// A clock that only moves when told to.
pub struct FixedClock(Cell<Millis>);

impl FixedClock {
    pub fn new(start: Millis) -> FixedClock {
        FixedClock(Cell::new(start))
    }

    pub fn advance(&self, ms: Millis) {
        self.0.set(self.0.get() + ms);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Millis {
        self.0.get()
    }
}

#[derive(Clone)]
struct JobRow {
    job: Job,
    started_at: Option<Millis>,
    result: Option<JobResult>,
    events: Vec<StoredEvent>,
}

/// Sessions, jobs and prompts in memory.
#[derive(Default)]
pub struct InMemoryStore {
    sessions: RefCell<Vec<Session>>,
    jobs: RefCell<HashMap<i64, JobRow>>,
    next_job: Cell<i64>,
    prompts: RefCell<Vec<PromptTemplate>>,
    next_prompt: Cell<i64>,
}

impl InMemoryStore {
    pub fn status_of(&self, id: JobId) -> JobStatus {
        self.jobs.borrow()[&id.0].job.status
    }

    pub fn job_of(&self, id: JobId) -> Job {
        self.jobs.borrow()[&id.0].job.clone()
    }

    pub fn result_of(&self, id: JobId) -> Option<JobResult> {
        self.jobs.borrow()[&id.0].result.clone()
    }

    pub fn events_of(&self, id: JobId) -> Vec<(String, String, String)> {
        self.jobs.borrow()[&id.0]
            .events
            .iter()
            .map(|e| (e.kind.clone(), e.actor.clone(), e.payload.clone()))
            .collect()
    }

    pub fn prompt_of(&self, id: JobId) -> Option<PromptId> {
        self.jobs.borrow()[&id.0].job.prompt_id
    }

    fn summarize(&self, row: &JobRow) -> JobSummary {
        let session_name = self
            .sessions
            .borrow()
            .iter()
            .find(|s| s.id == row.job.session_id)
            .map_or_else(String::new, |s| s.name.clone());
        let r = row.result.as_ref();
        JobSummary {
            job_id: row.job.id,
            session_id: row.job.session_id,
            session_name,
            prompt: row.job.prompt.clone(),
            model: row.job.model.clone(),
            status: row.job.status,
            cost_usd: r.map_or(0.0, |r| r.cost_usd),
            tokens_in: r.map_or(0, |r| r.tokens_in),
            tokens_out: r.map_or(0, |r| r.tokens_out),
            duration_ms: r.map(|r| r.duration_ms),
            created_at: row.job.created_at,
            started_at: row.started_at,
            error: r.and_then(|r| r.error.clone()),
        }
    }

    fn with_job<T>(&self, id: JobId, f: impl FnOnce(&mut JobRow) -> T) -> AppResult<T> {
        let mut jobs = self.jobs.borrow_mut();
        let row = jobs.get_mut(&id.0).ok_or(AppError::JobNotFound)?;
        Ok(f(row))
    }
}

impl SessionRepository for InMemoryStore {
    fn insert(&self, session: &Session) -> AppResult<()> {
        self.sessions.borrow_mut().push(session.clone());
        Ok(())
    }

    fn list_active(&self) -> AppResult<Vec<Session>> {
        let mut list: Vec<Session> = self
            .sessions
            .borrow()
            .iter()
            .filter(|s| !s.archived)
            .cloned()
            .collect();
        list.sort_by_key(|s| std::cmp::Reverse(s.activity()));
        Ok(list)
    }

    fn get(&self, id: SessionId) -> AppResult<Option<Session>> {
        Ok(self.sessions.borrow().iter().find(|s| s.id == id).cloned())
    }

    fn touch(&self, id: SessionId, now: Millis) -> AppResult<()> {
        if let Some(s) = self.sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
            s.last_used_at = Some(now);
        }
        Ok(())
    }

    fn has_started(&self, id: SessionId) -> AppResult<bool> {
        Ok(self
            .jobs
            .borrow()
            .values()
            .any(|j| j.job.session_id == id && j.events.iter().any(|e| e.kind == "init")))
    }
}

impl JobRepository for InMemoryStore {
    fn insert(&self, new: &NewJob, now: Millis) -> AppResult<JobId> {
        let id = JobId(self.next_job.get() + 1);
        self.next_job.set(id.0);
        let job = Job {
            id,
            session_id: new.session_id,
            prompt_id: new.prompt_id,
            prompt: new.prompt.clone(),
            model: new.model.clone(),
            priority: new.priority,
            fork: new.fork,
            status: JobStatus::Queued,
            created_at: now,
        };
        self.jobs.borrow_mut().insert(
            id.0,
            JobRow {
                job,
                started_at: None,
                result: None,
                events: Vec::new(),
            },
        );
        Ok(id)
    }

    fn get(&self, id: JobId) -> AppResult<Option<Job>> {
        Ok(self.jobs.borrow().get(&id.0).map(|r| r.job.clone()))
    }

    fn queued(&self) -> AppResult<Vec<Job>> {
        Ok(self
            .jobs
            .borrow()
            .values()
            .filter(|r| r.job.status == JobStatus::Queued)
            .map(|r| r.job.clone())
            .collect())
    }

    fn set_status(&self, id: JobId, status: JobStatus, now: Millis) -> AppResult<()> {
        self.with_job(id, |r| {
            r.job.status = status;
            if status == JobStatus::Running {
                r.started_at = Some(now);
            }
        })
    }

    fn status(&self, id: JobId) -> AppResult<JobStatus> {
        self.with_job(id, |r| r.job.status)
    }

    fn set_priority(&self, id: JobId, priority: i32) -> AppResult<()> {
        self.with_job(id, |r| r.job.priority = priority)
    }

    fn set_session(&self, id: JobId, session: SessionId) -> AppResult<()> {
        self.with_job(id, |r| r.job.session_id = session)
    }

    fn save_result(&self, id: JobId, result: &JobResult) -> AppResult<()> {
        self.with_job(id, |r| r.result = Some(result.clone()))
    }

    fn result(&self, id: JobId) -> AppResult<Option<JobResult>> {
        self.with_job(id, |r| r.result.clone())
    }

    fn append_event(
        &self,
        id: JobId,
        kind: &str,
        actor: &str,
        payload: &str,
        now: Millis,
    ) -> AppResult<()> {
        self.with_job(id, |r| {
            r.events.push(StoredEvent {
                ts: now,
                kind: kind.into(),
                actor: actor.into(),
                payload: payload.into(),
            })
        })
    }

    fn events(&self, id: JobId) -> AppResult<Vec<StoredEvent>> {
        self.with_job(id, |r| r.events.clone())
    }

    fn interrupt_running(&self, _now: Millis) -> AppResult<usize> {
        let mut n = 0;
        for row in self.jobs.borrow_mut().values_mut() {
            if row.job.status == JobStatus::Running {
                row.job.status = JobStatus::Interrupted;
                n += 1;
            }
        }
        Ok(n)
    }

    fn summaries(&self, limit: usize) -> AppResult<Vec<JobSummary>> {
        let jobs = self.jobs.borrow();
        let mut rows: Vec<&JobRow> = jobs.values().collect();
        rows.sort_by_key(|r| std::cmp::Reverse((r.job.created_at, r.job.id)));
        Ok(rows
            .into_iter()
            .take(limit)
            .map(|r| self.summarize(r))
            .collect())
    }

    fn summary(&self, id: JobId) -> AppResult<Option<JobSummary>> {
        Ok(self.jobs.borrow().get(&id.0).map(|r| self.summarize(r)))
    }
}

impl PromptRepository for InMemoryStore {
    fn insert(&self, draft: &PromptDraft, now: Millis) -> AppResult<PromptId> {
        let id = PromptId(self.next_prompt.get() + 1);
        self.next_prompt.set(id.0);
        self.prompts.borrow_mut().push(PromptTemplate {
            id,
            name: draft.name.clone(),
            body: draft.body.clone(),
            tags: draft.tags.clone(),
            created_at: now,
            updated_at: now,
        });
        Ok(id)
    }

    fn update(&self, id: PromptId, draft: &PromptDraft, now: Millis) -> AppResult<()> {
        let mut prompts = self.prompts.borrow_mut();
        let p = prompts
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(AppError::PromptNotFound)?;
        p.name = draft.name.clone();
        p.body = draft.body.clone();
        p.tags = draft.tags.clone();
        p.updated_at = now;
        Ok(())
    }

    fn delete(&self, id: PromptId) -> AppResult<()> {
        self.prompts.borrow_mut().retain(|p| p.id != id);
        for row in self.jobs.borrow_mut().values_mut() {
            if row.job.prompt_id == Some(id) {
                row.job.prompt_id = None;
            }
        }
        Ok(())
    }

    fn get(&self, id: PromptId) -> AppResult<Option<PromptTemplate>> {
        Ok(self.prompts.borrow().iter().find(|p| p.id == id).cloned())
    }

    fn list(&self) -> AppResult<Vec<PromptTemplate>> {
        let mut list = self.prompts.borrow().clone();
        list.sort_by_key(|p| std::cmp::Reverse((p.updated_at, p.id)));
        Ok(list)
    }
}

/// Returns a fixed diff and records which folders were asked about.
#[derive(Default)]
pub struct FakeWorkspace {
    diff: String,
    calls: RefCell<Vec<PathBuf>>,
}

impl FakeWorkspace {
    pub fn with_diff(diff: &str) -> FakeWorkspace {
        FakeWorkspace {
            diff: diff.to_string(),
            calls: RefCell::default(),
        }
    }

    pub fn calls(&self) -> Vec<PathBuf> {
        self.calls.borrow().clone()
    }
}

impl Workspace for FakeWorkspace {
    fn git_diff(&self, cwd: &Path) -> AppResult<String> {
        self.calls.borrow_mut().push(cwd.to_path_buf());
        Ok(self.diff.clone())
    }
}

/// Records requests and cancels; tests push messages through the captured sinks.
#[derive(Default)]
pub struct FakeRunner {
    requests: RefCell<Vec<RunRequest>>,
    sinks: RefCell<Vec<RunSink>>,
    cancelled: RefCell<Vec<JobId>>,
}

impl FakeRunner {
    pub fn request(&self, i: usize) -> RunRequest {
        self.requests.borrow()[i].clone()
    }

    pub fn started(&self) -> usize {
        self.requests.borrow().len()
    }

    /// Sends `msg` as if the `i`-th started job reported it.
    pub fn emit(&self, i: usize, msg: RunnerMsg) {
        (self.sinks.borrow()[i])(msg);
    }

    pub fn cancelled(&self) -> Vec<JobId> {
        self.cancelled.borrow().clone()
    }
}

impl AgentRunner for FakeRunner {
    fn start(&self, request: RunRequest, sink: RunSink) {
        self.requests.borrow_mut().push(request);
        self.sinks.borrow_mut().push(sink);
    }

    fn cancel(&self, job: JobId) {
        self.cancelled.borrow_mut().push(job);
    }
}

/// Reports whatever free memory the test sets (unknown by default).
#[derive(Default)]
pub struct FakeMonitor(Cell<Option<u64>>);

impl FakeMonitor {
    pub fn set(&self, free: Option<u64>) {
        self.0.set(free);
    }
}

impl SystemMonitor for FakeMonitor {
    fn free_memory_bytes(&self) -> Option<u64> {
        self.0.get()
    }
}
