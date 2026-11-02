//! In-memory fakes of every port (feature `test-support`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use swarmux_domain::{
    JobId, JobResult, JobStatus, Millis, PromptDraft, PromptId, PromptTemplate, Session, SessionId,
};

use crate::ports::{
    AgentRunner, Clock, JobRepository, PromptRepository, RunRequest, RunSink, RunnerMsg,
    SessionRepository, Workspace,
};
use crate::{AppError, AppResult};

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
    session: SessionId,
    prompt_id: Option<PromptId>,
    status: JobStatus,
    result: Option<JobResult>,
    events: Vec<(String, String, String)>,
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
        self.jobs.borrow()[&id.0].status
    }

    pub fn result_of(&self, id: JobId) -> Option<JobResult> {
        self.jobs.borrow()[&id.0].result.clone()
    }

    pub fn events_of(&self, id: JobId) -> Vec<(String, String, String)> {
        self.jobs.borrow()[&id.0].events.clone()
    }

    pub fn prompt_of(&self, id: JobId) -> Option<PromptId> {
        self.jobs.borrow()[&id.0].prompt_id
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
            .any(|j| j.session == id && j.events.iter().any(|(kind, _, _)| kind == "init")))
    }
}

impl JobRepository for InMemoryStore {
    fn insert(
        &self,
        session: SessionId,
        prompt_id: Option<PromptId>,
        _prompt: &str,
        _model: &str,
        _now: Millis,
    ) -> AppResult<JobId> {
        let id = self.next_job.get() + 1;
        self.next_job.set(id);
        self.jobs.borrow_mut().insert(
            id,
            JobRow {
                session,
                prompt_id,
                status: JobStatus::Queued,
                result: None,
                events: Vec::new(),
            },
        );
        Ok(JobId(id))
    }

    fn set_status(&self, id: JobId, status: JobStatus, _now: Millis) -> AppResult<()> {
        self.jobs.borrow_mut().get_mut(&id.0).unwrap().status = status;
        Ok(())
    }

    fn status(&self, id: JobId) -> AppResult<JobStatus> {
        Ok(self.status_of(id))
    }

    fn save_result(&self, id: JobId, result: &JobResult) -> AppResult<()> {
        self.jobs.borrow_mut().get_mut(&id.0).unwrap().result = Some(result.clone());
        Ok(())
    }

    fn result(&self, id: JobId) -> AppResult<Option<JobResult>> {
        Ok(self.result_of(id))
    }

    fn append_event(
        &self,
        id: JobId,
        kind: &str,
        actor: &str,
        payload: &str,
        _now: Millis,
    ) -> AppResult<()> {
        self.jobs.borrow_mut().get_mut(&id.0).unwrap().events.push((
            kind.into(),
            actor.into(),
            payload.into(),
        ));
        Ok(())
    }

    fn interrupt_running(&self, _now: Millis) -> AppResult<usize> {
        let mut n = 0;
        for job in self.jobs.borrow_mut().values_mut() {
            if job.status == JobStatus::Running {
                job.status = JobStatus::Interrupted;
                n += 1;
            }
        }
        Ok(n)
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
        for job in self.jobs.borrow_mut().values_mut() {
            if job.prompt_id == Some(id) {
                job.prompt_id = None;
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

/// Records requests; tests push messages through the captured sinks.
#[derive(Default)]
pub struct FakeRunner {
    requests: RefCell<Vec<RunRequest>>,
    sinks: RefCell<Vec<RunSink>>,
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
}

impl AgentRunner for FakeRunner {
    fn start(&self, request: RunRequest, sink: RunSink) {
        self.requests.borrow_mut().push(request);
        self.sinks.borrow_mut().push(sink);
    }
}
