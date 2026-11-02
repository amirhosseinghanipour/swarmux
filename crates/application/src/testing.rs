//! In-memory fakes of every port (feature `test-support`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use swarmux_domain::{JobId, JobResult, JobStatus, Millis, Session, SessionId};

use crate::AppResult;
use crate::ports::{
    AgentRunner, Clock, JobRepository, RunRequest, RunSink, RunnerMsg, SessionRepository,
};

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
    status: JobStatus,
    result: Option<JobResult>,
    events: Vec<(String, String, String)>,
}

/// Sessions and jobs in memory.
#[derive(Default)]
pub struct InMemoryStore {
    sessions: RefCell<Vec<Session>>,
    jobs: RefCell<HashMap<i64, JobRow>>,
    next_job: Cell<i64>,
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
