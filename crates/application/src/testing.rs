//! In-memory fakes of every port (feature `test-support`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use swarmux_domain::{
    Decision, Job, JobId, JobResult, JobStatus, Millis, PromptDraft, PromptId, PromptTemplate,
    ReviewState, Session, SessionId,
};

use crate::ledger::Ledger;
use crate::ports::{
    Advice, AdviceAnswer, AdviceError, AdviceReply, AdviceRequest, AdviceSink, AgentCatalog,
    AgentProfile, AgentRunner, Clock, ConsentStore, DecisionAdvisor, DecisionRepository, HookInbox,
    HookRequest, InboxItem, JobRepository, NewJob, PromptRepository, PromptRouter, RouteAnswer,
    RouteReply, RouteRequest, RouteSink, RuleWriter, RunRequest, RunSink, RunnerMsg,
    SessionRepository, StoredEvent, SystemMonitor, TelemetryFeed, TelemetryStore, Workspace,
};
use crate::{AppError, AppResult, DecisionRecord, JobSummary, TelemetrySnapshot};
use swarmux_domain::ledger::{LedgerDraft, LedgerEntry};
use swarmux_domain::telemetry::{Environment, RateLimit};

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
    review: Option<ReviewState>,
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
            review_of: row.job.review_of,
            cache_ratio: r.and_then(|r| {
                swarmux_domain::cache_read_ratio(r.tokens_in, r.cache_read, r.cache_creation)
            }),
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

    fn set_lean(&self, id: SessionId, lean: bool) -> AppResult<()> {
        let mut sessions = self.sessions.borrow_mut();
        let s = sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(AppError::SessionNotFound)?;
        s.lean = lean;
        Ok(())
    }

    fn set_worktree(&self, id: SessionId, worktree: Option<bool>) -> AppResult<()> {
        let mut sessions = self.sessions.borrow_mut();
        let s = sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(AppError::SessionNotFound)?;
        s.worktree = worktree;
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
            wants_review: new.wants_review,
            review_of: new.review_of,
        };
        self.jobs.borrow_mut().insert(
            id.0,
            JobRow {
                job,
                started_at: None,
                result: None,
                events: Vec::new(),
                review: None,
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

    fn set_review(&self, id: JobId, review: &ReviewState) -> AppResult<()> {
        self.with_job(id, |r| r.review = Some(review.clone()))
    }

    fn review(&self, id: JobId) -> AppResult<Option<ReviewState>> {
        self.with_job(id, |r| r.review.clone())
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

    fn command_line(&self, request: &RunRequest) -> Option<String> {
        let mode = if request.review {
            "--tools Read,Grep,Glob --no-session-persistence".to_string()
        } else if request.resume {
            format!("--resume {}", request.session_id)
        } else {
            format!("--session-id {}", request.session_id)
        };
        Some(format!("claude -p --model {} {mode}", request.model))
    }
}

/// Subagent definitions by type, set by the test.
#[derive(Default)]
pub struct FakeAgentCatalog {
    profiles: RefCell<HashMap<String, AgentProfile>>,
    asked: RefCell<Vec<(PathBuf, String)>>,
}

impl FakeAgentCatalog {
    /// Defines `agent_type` with `model`.
    pub fn define(&self, agent_type: &str, model: &str) {
        self.profiles.borrow_mut().insert(
            agent_type.to_string(),
            AgentProfile {
                model: Some(model.to_string()),
                description: None,
            },
        );
    }

    pub fn asked(&self) -> Vec<(PathBuf, String)> {
        self.asked.borrow().clone()
    }
}

impl AgentCatalog for FakeAgentCatalog {
    fn profile(&self, cwd: &Path, agent_type: &str) -> AgentProfile {
        self.asked
            .borrow_mut()
            .push((cwd.to_path_buf(), agent_type.to_string()));
        self.profiles
            .borrow()
            .get(agent_type)
            .cloned()
            .unwrap_or_default()
    }
}

/// Reports whatever free memory and power source the test sets (unknown by default).
#[derive(Default)]
pub struct FakeMonitor(Cell<Option<u64>>, Cell<Option<bool>>);

impl FakeMonitor {
    pub fn set(&self, free: Option<u64>) {
        self.0.set(free);
    }

    pub fn set_battery(&self, on_battery: Option<bool>) {
        self.1.set(on_battery);
    }
}

impl SystemMonitor for FakeMonitor {
    fn free_memory_bytes(&self) -> Option<u64> {
        self.0.get()
    }

    fn on_battery(&self) -> Option<bool> {
        self.1.get()
    }
}

/// Window readings the test sets.
#[derive(Default)]
pub struct FakeUsageProbe(RefCell<Vec<swarmux_domain::telemetry::LimitState>>);

impl FakeUsageProbe {
    pub fn set(&self, windows: Vec<swarmux_domain::telemetry::LimitState>) {
        *self.0.borrow_mut() = windows;
    }
}

impl crate::governor::UsageProbe for FakeUsageProbe {
    fn windows(&self) -> Vec<swarmux_domain::telemetry::LimitState> {
        self.0.borrow().clone()
    }
}

/// Pending resumes kept in memory.
#[derive(Default)]
pub struct InMemoryResumeBook(RefCell<Vec<crate::governor::PendingResume>>);

impl crate::governor::ResumeBook for InMemoryResumeBook {
    fn save(&self, resume: &crate::governor::PendingResume) -> AppResult<()> {
        let mut rows = self.0.borrow_mut();
        rows.retain(|r| r.job != resume.job);
        rows.push(resume.clone());
        Ok(())
    }

    fn remove(&self, job: JobId) -> AppResult<()> {
        self.0.borrow_mut().retain(|r| r.job != job);
        Ok(())
    }

    fn pending(&self) -> AppResult<Vec<crate::governor::PendingResume>> {
        Ok(self.0.borrow().clone())
    }
}

/// Hook requests pushed by the test; records every answer.
#[derive(Default)]
pub struct FakeHookInbox {
    queue: RefCell<Vec<InboxItem>>,
    answers: RefCell<Vec<(u64, Decision)>>,
    next: Cell<u64>,
}

impl FakeHookInbox {
    /// Queues an unanswered request for `tool` on `subject` from folder `/w`.
    /// Returns its id.
    pub fn push(&self, job: Option<JobId>, tool: &str, subject: &str) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.queue.borrow_mut().push(InboxItem {
            request: HookRequest {
                request_id: id,
                job_id: job,
                tool_name: tool.into(),
                subject: subject.into(),
                input: format!("{{\"subject\":{subject:?}}}"),
                cwd: "/w".into(),
            },
            answered: None,
        });
        id
    }

    /// Queues a request the transport already answered.
    pub fn push_answered(&self, tool: &str, subject: &str, decision: Decision) -> u64 {
        let id = self.push(None, tool, subject);
        if let Some(item) = self.queue.borrow_mut().last_mut() {
            item.answered = Some((decision, 7));
        }
        id
    }

    pub fn answers(&self) -> Vec<(u64, Decision)> {
        self.answers.borrow().clone()
    }

    pub fn answer_to(&self, id: u64) -> Option<Decision> {
        self.answers
            .borrow()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, d)| d.clone())
    }
}

impl HookInbox for FakeHookInbox {
    fn poll(&self) -> Vec<InboxItem> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }

    fn respond(&self, request_id: u64, decision: &Decision) {
        self.answers
            .borrow_mut()
            .push((request_id, decision.clone()));
    }
}

/// Keeps decisions in memory; can be told to fail.
#[derive(Default)]
pub struct FakeDecisionRepo {
    rows: RefCell<Vec<DecisionRecord>>,
    fail: Cell<bool>,
}

impl FakeDecisionRepo {
    pub fn rows(&self) -> Vec<DecisionRecord> {
        self.rows.borrow().clone()
    }

    pub fn fail(&self, fail: bool) {
        self.fail.set(fail);
    }
}

impl DecisionRepository for FakeDecisionRepo {
    fn record(&self, decision: &DecisionRecord) -> AppResult<()> {
        if self.fail.get() {
            return Err(AppError::Storage("disk full".into()));
        }
        self.rows.borrow_mut().push(decision.clone());
        Ok(())
    }

    fn recent_decisions(&self, limit: usize) -> AppResult<Vec<DecisionRecord>> {
        Ok(self
            .rows
            .borrow()
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect())
    }
}

/// Records saved rules; can be told to refuse.
#[derive(Default)]
pub struct FakeRuleWriter {
    rules: RefCell<Vec<String>>,
    refuse: Cell<bool>,
}

impl FakeRuleWriter {
    pub fn rules(&self) -> Vec<String> {
        self.rules.borrow().clone()
    }

    pub fn refuse(&self, refuse: bool) {
        self.refuse.set(refuse);
    }
}

impl RuleWriter for FakeRuleWriter {
    fn add_allow_rule(&self, rule: &str) -> AppResult<()> {
        if self.refuse.get() {
            return Err(AppError::Config("allow list is too complex to edit".into()));
        }
        self.rules.borrow_mut().push(rule.to_string());
        Ok(())
    }
}

/// Records advice requests; tests answer them with [`FakeAdvisor::reply`],
/// or set an answer given at once with [`FakeAdvisor::answer_with`].
#[derive(Default)]
pub struct FakeAdvisor {
    requests: RefCell<Vec<AdviceRequest>>,
    sinks: RefCell<Vec<Option<AdviceSink>>>,
    instant: RefCell<Option<Result<Advice, AdviceError>>>,
    no_key: Cell<bool>,
}

impl FakeAdvisor {
    pub fn requests(&self) -> Vec<AdviceRequest> {
        self.requests.borrow().clone()
    }

    /// Answers the `i`-th request (once).
    pub fn reply(&self, i: usize, result: Result<Advice, AdviceError>) {
        if let Some(sink) = self.sinks.borrow_mut()[i].take() {
            sink(AdviceReply {
                result,
                latency_ms: 40,
            });
        }
    }

    /// Every later request gets `result` at once.
    pub fn answer_with(&self, result: Result<Advice, AdviceError>) {
        *self.instant.borrow_mut() = Some(result);
    }

    pub fn set_no_key(&self, no_key: bool) {
        self.no_key.set(no_key);
    }
}

/// An advice answer with `confidence`.
pub fn advice(answer: AdviceAnswer, confidence: f64) -> Result<Advice, AdviceError> {
    Ok(Advice { answer, confidence })
}

impl DecisionAdvisor for FakeAdvisor {
    fn provider(&self) -> &str {
        "typesafe"
    }

    fn host(&self) -> &str {
        "fake.example"
    }

    fn has_key(&self) -> bool {
        !self.no_key.get()
    }

    fn advise(&self, request: AdviceRequest, sink: AdviceSink) {
        self.requests.borrow_mut().push(request);
        match self.instant.borrow().clone() {
            Some(result) => {
                sink(AdviceReply {
                    result,
                    latency_ms: 40,
                });
                self.sinks.borrow_mut().push(None);
            }
            None => self.sinks.borrow_mut().push(Some(sink)),
        }
    }
}

/// Consents in memory.
#[derive(Default)]
pub struct FakeConsents(RefCell<Vec<(String, Millis)>>);

impl FakeConsents {
    pub fn granted(&self) -> Vec<(String, Millis)> {
        self.0.borrow().clone()
    }
}

impl ConsentStore for FakeConsents {
    fn has_consent(&self, provider: &str) -> AppResult<bool> {
        Ok(self.0.borrow().iter().any(|(p, _)| p == provider))
    }

    fn grant_consent(&self, provider: &str, now: Millis) -> AppResult<()> {
        self.0.borrow_mut().push((provider.to_string(), now));
        Ok(())
    }
}

/// Records route requests; tests answer them with [`FakeRouter::reply`], or
/// set an answer given at once with [`FakeRouter::answer_with`].
#[derive(Default)]
pub struct FakeRouter {
    requests: RefCell<Vec<RouteRequest>>,
    sinks: RefCell<Vec<Option<RouteSink>>>,
    instant: RefCell<Option<RouteReply>>,
}

impl FakeRouter {
    pub fn requests(&self) -> Vec<RouteRequest> {
        self.requests.borrow().clone()
    }

    /// Answers the `i`-th request (once).
    pub fn reply(&self, i: usize, reply: RouteReply) {
        if let Some(sink) = self.sinks.borrow_mut()[i].take() {
            sink(reply);
        }
    }

    /// Every later request gets `reply` at once.
    pub fn answer_with(&self, reply: RouteReply) {
        *self.instant.borrow_mut() = Some(reply);
    }
}

/// A router reply picking `session`, costing $0.0002.
pub fn picks(session: &str, model: Option<&str>, reason: &str) -> RouteReply {
    RouteReply {
        result: Ok(RouteAnswer {
            session: session.into(),
            model: model.map(Into::into),
            reason: reason.into(),
        }),
        cost_usd: 0.0002,
    }
}

impl PromptRouter for FakeRouter {
    fn route(&self, request: RouteRequest, sink: RouteSink) {
        self.requests.borrow_mut().push(request);
        match self.instant.borrow().clone() {
            Some(reply) => {
                sink(reply);
                self.sinks.borrow_mut().push(None);
            }
            None => self.sinks.borrow_mut().push(Some(sink)),
        }
    }
}

/// A telemetry feed that returns whatever snapshot it was given.
#[derive(Default)]
pub struct FakeTelemetry {
    snapshot: RefCell<TelemetrySnapshot>,
    focused: RefCell<Vec<PathBuf>>,
}

impl FakeTelemetry {
    pub fn set(&self, snapshot: TelemetrySnapshot) {
        *self.snapshot.borrow_mut() = snapshot;
    }

    /// Folders `focus` was called with, oldest first.
    pub fn focused(&self) -> Vec<PathBuf> {
        self.focused.borrow().clone()
    }
}

impl TelemetryFeed for FakeTelemetry {
    fn snapshot(&self) -> TelemetrySnapshot {
        self.snapshot.borrow().clone()
    }

    fn focus(&self, cwd: &Path) {
        self.focused.borrow_mut().push(cwd.to_path_buf());
    }
}

/// Limit readings and environments in memory.
#[derive(Default)]
pub struct FakeTelemetryStore {
    limits: RefCell<Vec<(RateLimit, Millis)>>,
    environments: RefCell<Vec<(Environment, Millis)>>,
}

impl TelemetryStore for FakeTelemetryStore {
    fn save_limit(&self, limit: &RateLimit, observed_at: Millis) -> AppResult<()> {
        let mut limits = self.limits.borrow_mut();
        limits.retain(|(l, _)| l.window != limit.window);
        limits.push((limit.clone(), observed_at));
        Ok(())
    }

    fn limits(&self) -> AppResult<Vec<(RateLimit, Millis)>> {
        Ok(self.limits.borrow().clone())
    }

    fn save_environment(&self, env: &Environment, seen_at: Millis) -> AppResult<()> {
        let mut envs = self.environments.borrow_mut();
        envs.retain(|(e, _)| e.session_id != env.session_id);
        envs.insert(0, (env.clone(), seen_at));
        Ok(())
    }

    fn environments(&self, limit: usize) -> AppResult<Vec<(Environment, Millis)>> {
        Ok(self
            .environments
            .borrow()
            .iter()
            .take(limit)
            .cloned()
            .collect())
    }
}

/// A ledger in memory; tests can forge or drop entries to check verification.
#[derive(Default)]
pub struct InMemoryLedger {
    entries: RefCell<Vec<LedgerEntry>>,
}

impl InMemoryLedger {
    pub fn entries(&self) -> Vec<LedgerEntry> {
        self.entries.borrow().clone()
    }

    /// Replaces the payload of entry `seq` without rehashing it.
    pub fn tamper(&self, seq: u64, payload_json: &str) {
        for e in self.entries.borrow_mut().iter_mut() {
            if e.seq == seq {
                e.draft.payload_json = payload_json.into();
            }
        }
    }

    /// Drops entry `seq`.
    pub fn remove(&self, seq: u64) {
        self.entries.borrow_mut().retain(|e| e.seq != seq);
    }
}

impl Ledger for InMemoryLedger {
    fn append(&self, draft: LedgerDraft) -> AppResult<LedgerEntry> {
        let mut entries = self.entries.borrow_mut();
        if entries.is_empty() {
            entries.push(LedgerEntry::next(
                None,
                LedgerDraft::genesis(draft.ts, "test"),
            ));
        }
        let entry = LedgerEntry::next(entries.last(), draft);
        entries.push(entry.clone());
        Ok(entry)
    }

    fn head(&self) -> AppResult<Option<LedgerEntry>> {
        Ok(self.entries.borrow().last().cloned())
    }

    fn range(&self, from_seq: u64, limit: usize) -> AppResult<Vec<LedgerEntry>> {
        Ok(self
            .entries
            .borrow()
            .iter()
            .filter(|e| e.seq >= from_seq)
            .take(limit)
            .cloned()
            .collect())
    }
}

/// Lifecycle events pushed by the test.
#[derive(Default)]
pub struct FakeHookFeed {
    queue: RefCell<Vec<crate::hookbus::HookNotice>>,
}

impl FakeHookFeed {
    pub fn push(&self, notice: crate::hookbus::HookNotice) {
        self.queue.borrow_mut().push(notice);
    }
}

impl crate::hookbus::HookFeed for FakeHookFeed {
    fn poll_notices(&self) -> Vec<crate::hookbus::HookNotice> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }
}

/// Worktrees and checkpoints in memory.
#[derive(Default)]
pub struct InMemoryWorktrees {
    records: RefCell<Vec<swarmux_domain::WorktreeRecord>>,
    checkpoints: RefCell<Vec<swarmux_domain::Checkpoint>>,
}

impl crate::worktree::WorktreeStore for InMemoryWorktrees {
    fn insert(&self, record: &swarmux_domain::WorktreeRecord) -> AppResult<()> {
        let mut records = self.records.borrow_mut();
        if records
            .iter()
            .any(|r| r.removed_at.is_none() && r.slot == record.slot && r.job != record.job)
        {
            return Err(AppError::Storage("slot taken".into()));
        }
        let seq = |r: &swarmux_domain::WorktreeRecord| r.member.as_ref().map(|m| m.seq);
        records.retain(|r| r.job != record.job || seq(r) != seq(record));
        records.push(record.clone());
        Ok(())
    }

    fn get(&self, job: JobId) -> AppResult<Option<swarmux_domain::WorktreeRecord>> {
        Ok(self
            .records
            .borrow()
            .iter()
            .filter(|r| r.job == job)
            .min_by_key(|r| r.member.as_ref().map_or(0, |m| m.seq))
            .cloned())
    }

    fn set(&self, job: JobId) -> AppResult<Vec<swarmux_domain::WorktreeRecord>> {
        let mut out: Vec<_> = self
            .records
            .borrow()
            .iter()
            .filter(|r| r.job == job)
            .cloned()
            .collect();
        out.sort_by_key(|r| r.member.as_ref().map_or(0, |m| m.seq));
        Ok(out)
    }

    fn active(&self) -> AppResult<Vec<swarmux_domain::WorktreeRecord>> {
        Ok(self
            .records
            .borrow()
            .iter()
            .filter(|r| r.removed_at.is_none())
            .cloned()
            .collect())
    }

    fn rebind(&self, from: JobId, to: JobId) -> AppResult<bool> {
        let mut records = self.records.borrow_mut();
        let mut moved = false;
        for r in records
            .iter_mut()
            .filter(|r| r.job == from && r.removed_at.is_none())
        {
            r.job = to;
            moved = true;
        }
        if !moved {
            return Ok(false);
        }
        for c in self.checkpoints.borrow_mut().iter_mut() {
            if c.job == from {
                c.job = to;
            }
        }
        Ok(true)
    }

    fn mark_removed(&self, job: JobId, now: Millis) -> AppResult<()> {
        for r in self
            .records
            .borrow_mut()
            .iter_mut()
            .filter(|r| r.job == job)
        {
            r.removed_at = Some(now);
        }
        Ok(())
    }

    fn insert_checkpoint(&self, c: &swarmux_domain::Checkpoint) -> AppResult<i64> {
        let mut all = self.checkpoints.borrow_mut();
        let id = all.len() as i64 + 1;
        let mut c = c.clone();
        c.id = id;
        all.push(c);
        Ok(id)
    }

    fn checkpoints(&self, job: JobId) -> AppResult<Vec<swarmux_domain::Checkpoint>> {
        Ok(self
            .checkpoints
            .borrow()
            .iter()
            .filter(|c| c.job == job)
            .cloned()
            .collect())
    }

    fn recent_checkpoints(&self, limit: usize) -> AppResult<Vec<swarmux_domain::Checkpoint>> {
        Ok(self
            .checkpoints
            .borrow()
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect())
    }
}

/// A git that only remembers what it was asked to do.
#[derive(Default)]
pub struct FakeGit {
    /// Repository roots by folder prefix: any folder under a key is in it.
    pub repos: RefCell<Vec<PathBuf>>,
    pub folders: RefCell<Vec<PathBuf>>,
    pub snapshots: Cell<u32>,
    pub removed: RefCell<Vec<PathBuf>>,
    pub handoffs: RefCell<Vec<(PathBuf, String)>>,
    /// The branch each new worktree started from (`None`: HEAD).
    pub bases: RefCell<Vec<(PathBuf, Option<String>)>>,
}

impl FakeGit {
    pub fn with_repo(repo: &str) -> FakeGit {
        let git = FakeGit::default();
        git.repos.borrow_mut().push(PathBuf::from(repo));
        git
    }
}

impl crate::worktree::Worktrees for FakeGit {
    fn repository(&self, dir: &Path) -> AppResult<Option<PathBuf>> {
        Ok(self
            .repos
            .borrow()
            .iter()
            .find(|r| dir.starts_with(r))
            .cloned())
    }

    fn add(&self, req: &crate::worktree::AddWorktree) -> AppResult<crate::worktree::Added> {
        self.folders.borrow_mut().push(req.path.clone());
        self.bases
            .borrow_mut()
            .push((req.path.clone(), req.base.clone()));
        Ok(crate::worktree::Added {
            created_branch: true,
            copied: req.copy.clone(),
        })
    }

    fn snapshot(&self, _: &Path, _: &str) -> AppResult<String> {
        self.snapshots.set(self.snapshots.get() + 1);
        Ok(format!("{:040x}", self.snapshots.get()))
    }

    fn remove(&self, _: &Path, path: &Path, _: &str) -> AppResult<crate::worktree::Removed> {
        self.folders.borrow_mut().retain(|p| p != path);
        self.removed.borrow_mut().push(path.to_path_buf());
        Ok(crate::worktree::Removed {
            snapshot: Some("saved".into()),
            branch_deleted: false,
        })
    }

    fn present(&self, path: &Path) -> bool {
        self.folders.borrow().iter().any(|p| p == path)
    }

    fn write_handoff(&self, path: &Path, handoff: &swarmux_domain::Handoff) -> AppResult<PathBuf> {
        self.handoffs
            .borrow_mut()
            .push((path.to_path_buf(), handoff.to_json()));
        Ok(path.join("handoff.json"))
    }
}

/// Everything worktrees and checkpoints need, wired to fakes: one session
/// "Demo app" whose folder `/repo` is a git repository.
pub struct WorktreeRig {
    pub git: Rc<FakeGit>,
    pub store: Rc<InMemoryStore>,
    pub records: Rc<InMemoryWorktrees>,
    pub ledger: Rc<InMemoryLedger>,
    pub clock: Rc<FixedClock>,
    pub ledger_service: Rc<crate::ledger::LedgerService>,
    pub board: std::sync::Arc<crate::checkpoint::HandoffBoard>,
    pub worktrees: Rc<crate::worktree::WorktreeService>,
    pub checkpoints: Rc<crate::checkpoint::CheckpointService>,
    pub session: Session,
}

impl WorktreeRig {
    pub fn new() -> WorktreeRig {
        WorktreeRig::with_settings(&crate::WorktreeSettings::default())
    }

    pub fn with_settings(settings: &crate::WorktreeSettings) -> WorktreeRig {
        let git = Rc::new(FakeGit::with_repo("/repo"));
        let store = Rc::new(InMemoryStore::default());
        let records = Rc::new(InMemoryWorktrees::default());
        let ledger = Rc::new(InMemoryLedger::default());
        let clock = Rc::new(FixedClock::new(1_000_000));
        let ledger_service = Rc::new(crate::ledger::LedgerService::new(
            ledger.clone(),
            clock.clone(),
        ));
        let board = crate::checkpoint::HandoffBoard::new();
        let worktrees = Rc::new(
            crate::worktree::WorktreeService::new(
                git.clone(),
                records.clone(),
                store.clone(),
                clock.clone(),
                PathBuf::from("/wt"),
            )
            .with_settings(settings)
            .with_ledger(ledger_service.clone())
            .with_handoffs(board.clone()),
        );
        let checkpoints = Rc::new(
            crate::checkpoint::CheckpointService::new(
                worktrees.clone(),
                store.clone(),
                ledger_service.clone(),
                clock.clone(),
            )
            .with_handoffs(board.clone()),
        );
        let session =
            Session::new(SessionId::random(), "Demo app", "/repo".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(&*store, &session).unwrap();
        WorktreeRig {
            git,
            store,
            records,
            ledger,
            clock,
            ledger_service,
            board,
            worktrees,
            checkpoints,
            session,
        }
    }

    /// Queues a job of the session and returns its id.
    pub fn job(&self, prompt: &str) -> JobId {
        JobRepository::insert(
            &*self.store,
            &NewJob {
                session_id: self.session.id,
                prompt_id: None,
                prompt: prompt.into(),
                model: "sonnet".into(),
                priority: 0,
                fork: false,
                wants_review: false,
                review_of: None,
            },
            self.clock.now(),
        )
        .unwrap()
    }
}

impl Default for WorktreeRig {
    fn default() -> Self {
        WorktreeRig::new()
    }
}

/// Status lines pushed by the test.
#[derive(Default)]
pub struct FakeStatuslineFeed {
    queue: RefCell<Vec<swarmux_domain::telemetry::Statusline>>,
}

impl FakeStatuslineFeed {
    pub fn push(&self, line: swarmux_domain::telemetry::Statusline) {
        self.queue.borrow_mut().push(line);
    }
}

impl crate::telemetry::StatuslineFeed for FakeStatuslineFeed {
    fn poll_statuslines(&self) -> Vec<swarmux_domain::telemetry::Statusline> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }
}

/// Records the sessions it was asked to open; fails when told to.
#[derive(Default)]
pub struct FakeTerminal {
    opened: RefCell<Vec<(PathBuf, String)>>,
    fail: Cell<bool>,
}

impl FakeTerminal {
    pub fn opened(&self) -> Vec<(PathBuf, String)> {
        self.opened.borrow().clone()
    }

    pub fn fail_next(&self) {
        self.fail.set(true);
    }
}

impl crate::SessionTerminal for FakeTerminal {
    fn open_resume(&self, cwd: &Path, session: &str) -> AppResult<()> {
        if self.fail.replace(false) {
            return Err(AppError::Workspace("no terminal".into()));
        }
        self.opened
            .borrow_mut()
            .push((cwd.to_path_buf(), session.to_string()));
        Ok(())
    }
}

/// Missions kept in memory, by slug.
#[derive(Default)]
pub struct InMemoryMissions {
    missions: RefCell<Vec<swarmux_domain::mission::Mission>>,
    /// Saves so far.
    pub saves: Cell<usize>,
}

impl crate::missions::MissionStore for InMemoryMissions {
    fn save(&self, mission: &swarmux_domain::mission::Mission) -> AppResult<()> {
        self.saves.set(self.saves.get() + 1);
        let mut all = self.missions.borrow_mut();
        match all.iter_mut().find(|m| m.slug == mission.slug) {
            Some(m) => *m = mission.clone(),
            None => all.push(mission.clone()),
        }
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<swarmux_domain::mission::Mission>> {
        Ok(self.missions.borrow().clone())
    }
}

/// Records lint requests; tests answer them with [`FakeLinter::reply`], or
/// set an answer given at once with [`FakeLinter::answer_with`].
#[derive(Default)]
pub struct FakeLinter {
    requests: RefCell<Vec<crate::missions::LintRequest>>,
    sinks: RefCell<Vec<Option<crate::missions::LintSink>>>,
    instant: RefCell<Option<crate::missions::LintReply>>,
}

impl FakeLinter {
    pub fn requests(&self) -> Vec<crate::missions::LintRequest> {
        self.requests.borrow().clone()
    }

    /// Answers the `i`-th request (once).
    pub fn reply(&self, i: usize, reply: crate::missions::LintReply) {
        if let Some(sink) = self.sinks.borrow_mut()[i].take() {
            sink(reply);
        }
    }

    /// Every later request gets `reply` at once.
    pub fn answer_with(&self, reply: crate::missions::LintReply) {
        *self.instant.borrow_mut() = Some(reply);
    }
}

/// A linter reply asking `questions`, costing $0.0005.
pub fn asks(questions: &[&str]) -> crate::missions::LintReply {
    crate::missions::LintReply {
        result: Ok(questions
            .iter()
            .map(|q| swarmux_domain::mission::Question {
                question: q.to_string(),
                why: "it matters".into(),
            })
            .collect()),
        cost_usd: 0.0005,
        source: "haiku".into(),
    }
}

impl crate::missions::MissionLinter for FakeLinter {
    fn lint(&self, request: crate::missions::LintRequest, sink: crate::missions::LintSink) {
        self.requests.borrow_mut().push(request);
        match self.instant.borrow().clone() {
            Some(reply) => {
                sink(reply);
                self.sinks.borrow_mut().push(None);
            }
            None => self.sinks.borrow_mut().push(Some(sink)),
        }
    }
}

/// Records the gates jobs were given and the checks asked for.
#[derive(Default)]
pub struct FakeJobGates {
    pub assigned: RefCell<Vec<(JobId, swarmux_domain::mission::MissionGate)>>,
    pub verified: RefCell<Vec<(JobId, PathBuf)>>,
    /// Makes `verify` fail with this.
    pub broken: RefCell<Option<String>>,
}

impl crate::gate::JobGates for FakeJobGates {
    fn assign(&self, job: JobId, gate: swarmux_domain::mission::MissionGate) {
        self.assigned.borrow_mut().push((job, gate));
    }

    fn verify(&self, job: JobId, dir: &Path) -> Result<(), String> {
        self.verified.borrow_mut().push((job, dir.to_path_buf()));
        match self.broken.borrow().clone() {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }
}

/// Records diagnosis requests; tests answer them with
/// [`FakeDiagnoser::reply`], or set an answer given at once.
#[derive(Default)]
pub struct FakeDiagnoser {
    requests: RefCell<Vec<crate::watchdog::DiagnosisRequest>>,
    sinks: RefCell<Vec<Option<crate::watchdog::DiagnosisSink>>>,
    instant: RefCell<Option<crate::watchdog::DiagnosisReply>>,
}

impl FakeDiagnoser {
    pub fn requests(&self) -> Vec<crate::watchdog::DiagnosisRequest> {
        self.requests.borrow().clone()
    }

    /// Answers the `i`-th request (once).
    pub fn reply(&self, i: usize, reply: crate::watchdog::DiagnosisReply) {
        if let Some(sink) = self.sinks.borrow_mut()[i].take() {
            sink(reply);
        }
    }

    /// Every later request gets `reply` at once.
    pub fn answer_with(&self, reply: crate::watchdog::DiagnosisReply) {
        *self.instant.borrow_mut() = Some(reply);
    }
}

/// A diagnosis naming `suspect`, with `instruction`.
pub fn diagnosis(suspect: &str, instruction: &str) -> crate::watchdog::DiagnosisReply {
    crate::watchdog::DiagnosisReply {
        result: Ok(crate::watchdog::Diagnosis {
            suspect: suspect.into(),
            instruction: instruction.into(),
        }),
        cost_usd: 0.001,
    }
}

impl crate::watchdog::Diagnoser for FakeDiagnoser {
    fn diagnose(
        &self,
        request: crate::watchdog::DiagnosisRequest,
        sink: crate::watchdog::DiagnosisSink,
    ) {
        self.requests.borrow_mut().push(request);
        match self.instant.borrow().clone() {
            Some(reply) => {
                sink(reply);
                self.sinks.borrow_mut().push(None);
            }
            None => self.sinks.borrow_mut().push(Some(sink)),
        }
    }
}

/// Records restores; each returns a made-up commit keeping what was replaced.
#[derive(Default)]
pub struct FakeRestore {
    pub calls: RefCell<Vec<(PathBuf, String)>>,
}

impl crate::watchdog::CheckpointRestore for FakeRestore {
    fn restore(&self, worktree: &Path, commit: &str) -> AppResult<String> {
        let mut calls = self.calls.borrow_mut();
        calls.push((worktree.to_path_buf(), commit.to_string()));
        Ok(format!("saved{:035x}", calls.len()))
    }
}

/// Records planning requests; tests answer them with
/// [`FakePlanner::reply`], or set an answer given at once.
#[derive(Default)]
pub struct FakePlanner {
    requests: RefCell<Vec<crate::missions::PlanRequest>>,
    sinks: RefCell<Vec<Option<crate::missions::PlanSink>>>,
    instant: RefCell<Option<crate::missions::PlanReply>>,
}

impl FakePlanner {
    pub fn requests(&self) -> Vec<crate::missions::PlanRequest> {
        self.requests.borrow().clone()
    }

    /// Answers the `i`-th request (once).
    pub fn reply(&self, i: usize, reply: crate::missions::PlanReply) {
        if let Some(sink) = self.sinks.borrow_mut()[i].take() {
            sink(reply);
        }
    }

    /// Every later request gets `reply` at once.
    pub fn answer_with(&self, reply: crate::missions::PlanReply) {
        *self.instant.borrow_mut() = Some(reply);
    }
}

impl crate::missions::Planner for FakePlanner {
    fn plan(&self, request: crate::missions::PlanRequest, sink: crate::missions::PlanSink) {
        self.requests.borrow_mut().push(request);
        match self.instant.borrow().clone() {
            Some(reply) => {
                sink(reply);
                self.sinks.borrow_mut().push(None);
            }
            None => self.sinks.borrow_mut().push(Some(sink)),
        }
    }
}

/// Plans kept in memory.
#[derive(Default)]
pub struct InMemoryPlans {
    records: RefCell<Vec<(String, swarmux_domain::plan::PlanRecord)>>,
}

impl crate::missions::PlanStore for InMemoryPlans {
    fn save(
        &self,
        slug: &str,
        record: &swarmux_domain::plan::PlanRecord,
        _at: Millis,
    ) -> AppResult<()> {
        let mut all = self.records.borrow_mut();
        match all.iter_mut().find(|(s, _)| s == slug) {
            Some(r) => r.1 = record.clone(),
            None => all.push((slug.to_string(), record.clone())),
        }
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<(String, swarmux_domain::plan::PlanRecord)>> {
        Ok(self.records.borrow().clone())
    }
}

/// Reports the files it was told changed (or fails).
#[derive(Default)]
pub struct FakeChanges {
    pub files: RefCell<Vec<String>>,
    pub broken: RefCell<Option<String>>,
    /// The folders asked about.
    pub asked: RefCell<Vec<PathBuf>>,
}

impl crate::missions::ChangedFiles for FakeChanges {
    fn changed(&self, dir: &Path) -> AppResult<Vec<String>> {
        self.asked.borrow_mut().push(dir.to_path_buf());
        match self.broken.borrow().clone() {
            Some(why) => Err(AppError::Workspace(why)),
            None => Ok(self.files.borrow().clone()),
        }
    }
}

/// A merge queue over branches that exist only as lists of changed files.
#[derive(Default)]
pub struct FakeMergeQueue {
    /// Integration branches opened, with the repository.
    pub opened: RefCell<Vec<(PathBuf, String)>>,
    /// The files each branch changed; tests (and fake agents) fill it.
    pub changes: RefCell<HashMap<String, Vec<String>>>,
    /// Branches whose next merges conflict: how many more times, in which files.
    pub conflicts: RefCell<HashMap<String, (u32, Vec<String>)>>,
    /// Branches merged, in order.
    pub merged: RefCell<Vec<String>>,
    /// Every file that landed in the integration branch.
    pub landed: RefCell<Vec<String>>,
    /// Merges tried, conflicts included.
    pub attempts: Cell<u32>,
    /// Worktrees brought up to a branch.
    pub forwarded: RefCell<Vec<(PathBuf, String)>>,
    /// Branches merged, by the folder their integration branch is in.
    pub merged_in: RefCell<Vec<(PathBuf, String)>>,
    /// Integration folders whose merges conflict.
    pub conflict_in: RefCell<Vec<PathBuf>>,
    /// Integration folders put back, and to what.
    pub resets: RefCell<Vec<(PathBuf, String)>>,
}

impl FakeMergeQueue {
    /// Notes that `branch` changed `file`.
    pub fn touch(&self, branch: &str, file: &str) {
        self.changes
            .borrow_mut()
            .entry(branch.to_string())
            .or_default()
            .push(file.to_string());
    }
}

impl crate::missions::MergeQueue for FakeMergeQueue {
    fn open(&self, repo: &Path, branch: &str) -> AppResult<PathBuf> {
        self.opened
            .borrow_mut()
            .push((repo.to_path_buf(), branch.to_string()));
        let name = repo
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        Ok(PathBuf::from("/wt/integration").join(name))
    }

    fn changed(&self, _: &Path, branch: &str) -> AppResult<Vec<String>> {
        Ok(self
            .changes
            .borrow()
            .get(branch)
            .cloned()
            .unwrap_or_default())
    }

    fn merge(
        &self,
        workdir: &Path,
        branch: &str,
        _: &str,
    ) -> AppResult<crate::missions::MergeOutcome> {
        self.attempts.set(self.attempts.get() + 1);
        if self.conflict_in.borrow().iter().any(|w| w == workdir) {
            return Ok(crate::missions::MergeOutcome::Conflict {
                files: vec!["lib/api.dart".into()],
            });
        }
        if let Some((left, files)) = self.conflicts.borrow_mut().get_mut(branch)
            && *left > 0
        {
            *left -= 1;
            return Ok(crate::missions::MergeOutcome::Conflict {
                files: files.clone(),
            });
        }
        self.merged_in
            .borrow_mut()
            .push((workdir.to_path_buf(), branch.to_string()));
        self.merged.borrow_mut().push(branch.to_string());
        let files = self.changed(Path::new(""), branch)?;
        self.landed.borrow_mut().extend(files);
        Ok(crate::missions::MergeOutcome::Merged {
            commit: format!("{:040x}", self.merged.borrow().len()),
        })
    }

    fn contains(&self, workdir: &Path, branch: &str) -> AppResult<bool> {
        // Branches of one name live in many repositories: only this
        // folder's merges count.
        Ok(self
            .merged_in
            .borrow()
            .iter()
            .any(|(w, b)| w == workdir && b == branch))
    }

    fn head(&self, workdir: &Path) -> AppResult<String> {
        let n = self
            .merged_in
            .borrow()
            .iter()
            .filter(|(w, _)| w == workdir)
            .count();
        Ok(format!("head-{n}"))
    }

    fn reset(&self, workdir: &Path, commit: &str) -> AppResult<()> {
        let keep: usize = commit
            .strip_prefix("head-")
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| AppError::Workspace(format!("no commit {commit}")))?;
        let mut seen = 0;
        self.merged_in.borrow_mut().retain(|(w, _)| {
            if w != workdir {
                return true;
            }
            seen += 1;
            seen <= keep
        });
        self.resets
            .borrow_mut()
            .push((workdir.to_path_buf(), commit.to_string()));
        Ok(())
    }

    fn fast_forward(&self, worktree: &Path, branch: &str) -> AppResult<()> {
        self.forwarded
            .borrow_mut()
            .push((worktree.to_path_buf(), branch.to_string()));
        Ok(())
    }
}

/// Board entries kept in memory.
#[derive(Default)]
pub struct InMemoryBoard {
    rows: RefCell<Vec<swarmux_domain::board::BoardEntry>>,
}

impl crate::board::BoardStore for InMemoryBoard {
    fn append(&self, entry: &swarmux_domain::board::BoardEntry) -> AppResult<i64> {
        let mut rows = self.rows.borrow_mut();
        let mut e = entry.clone();
        e.id = rows.len() as i64 + 1;
        let id = e.id;
        rows.push(e);
        Ok(id)
    }

    fn recent(&self, limit: usize) -> AppResult<Vec<swarmux_domain::board::BoardEntry>> {
        let rows = self.rows.borrow();
        Ok(rows[rows.len().saturating_sub(limit)..].to_vec())
    }

    fn update(&self, entry: &swarmux_domain::board::BoardEntry) -> AppResult<()> {
        if let Some(r) = self.rows.borrow_mut().iter_mut().find(|r| r.id == entry.id) {
            *r = entry.clone();
        }
        Ok(())
    }
}

/// Board calls queued by a test, and the replies they got.
#[derive(Default)]
pub struct FakeBoardInbox {
    queue: RefCell<Vec<crate::board::BoardCall>>,
    replies: RefCell<Vec<(u64, crate::board::BoardReply)>>,
    next: Cell<u64>,
}

impl FakeBoardInbox {
    /// Queues a call; returns its id.
    pub fn push(&self, job: Option<JobId>, request: crate::board::BoardRequest) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.queue
            .borrow_mut()
            .push(crate::board::BoardCall { id, job, request });
        id
    }

    pub fn reply(&self, call: u64) -> Option<crate::board::BoardReply> {
        self.replies
            .borrow()
            .iter()
            .find(|(id, _)| *id == call)
            .map(|(_, r)| r.clone())
    }
}

impl crate::board::BoardInbox for FakeBoardInbox {
    fn poll(&self) -> Vec<crate::board::BoardCall> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }

    fn respond(&self, call: u64, reply: crate::board::BoardReply) {
        self.replies.borrow_mut().push((call, reply));
    }
}

/// Workspaces kept in memory.
#[derive(Default)]
pub struct InMemoryWorkspaces {
    all: RefCell<Vec<swarmux_domain::Workspace>>,
}

impl crate::workspace::WorkspaceStore for InMemoryWorkspaces {
    fn save(&self, workspace: &swarmux_domain::Workspace) -> AppResult<()> {
        let mut all = self.all.borrow_mut();
        match all.iter_mut().find(|w| w.id == workspace.id) {
            Some(w) => *w = workspace.clone(),
            None => all.push(workspace.clone()),
        }
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<swarmux_domain::Workspace>> {
        let mut all = self.all.borrow().clone();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(all)
    }

    fn get(&self, id: swarmux_domain::WorkspaceId) -> AppResult<Option<swarmux_domain::Workspace>> {
        Ok(self.all.borrow().iter().find(|w| w.id == id).cloned())
    }

    fn remove(&self, id: swarmux_domain::WorkspaceId) -> AppResult<()> {
        self.all.borrow_mut().retain(|w| w.id != id);
        Ok(())
    }
}

/// A disk with the repositories a test puts on it, by root.
#[derive(Default)]
pub struct FakeScanner {
    pub found: RefCell<HashMap<PathBuf, Vec<crate::workspace::ScannedRepo>>>,
}

impl FakeScanner {
    /// Puts a repository at `path` below `root`, with these files.
    pub fn put(&self, root: &str, path: &str, files: &[&str]) {
        let root = PathBuf::from(root);
        let git_root = if path == "." {
            root.clone()
        } else {
            root.join(path)
        };
        self.found
            .borrow_mut()
            .entry(root)
            .or_default()
            .push(crate::workspace::ScannedRepo {
                path: path.to_string(),
                git_root,
                default_branch: Some("main".into()),
                listing: swarmux_domain::workspace::RepoListing {
                    files: files.iter().map(|f| f.to_string()).collect(),
                    texts: Vec::new(),
                },
            });
    }

    /// Takes the repository at `path` away.
    pub fn drop_repo(&self, root: &str, path: &str) {
        if let Some(v) = self.found.borrow_mut().get_mut(Path::new(root)) {
            v.retain(|s| s.path != path);
        }
    }
}

impl crate::workspace::WorkspaceScanner for FakeScanner {
    fn scan(&self, root: &Path) -> AppResult<Vec<crate::workspace::ScannedRepo>> {
        Ok(self.found.borrow().get(root).cloned().unwrap_or_default())
    }
}
