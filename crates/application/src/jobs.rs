mod review;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::budget::{Admission, JobClass, is_limit_error};
use swarmux_domain::telemetry::{
    AgentNode, JobActivity, LimitState, RateLimit, SwarmActivity, TodoStatus, agent_tree,
    context_window,
};
use swarmux_domain::{
    AgentEvent, DomainError, Job, JobId, JobProgress, JobResult, JobStatus, Limits, LoopBreaker,
    LoopTrip, Millis, PromptId, ReviewState, Session, SessionId, SessionState, describe, plan,
    run_order,
};

use crate::gate::ExecutionLog;
use crate::governor::Governor;
use crate::ports::{
    AgentCatalog, AgentRunner, Clock, JobRepository, NewJob, RunRequest, RunSink, RunnerMsg,
    SessionRepository, SystemMonitor, TelemetryStore, Workspace,
};
use crate::worktree::WorktreeService;
use crate::{AppError, AppResult, RenderedPrompt, ReviewSettings, Settings};

/// Finished jobs kept in memory for the queue and dashboard.
const KEEP_FINISHED: usize = 50;
/// Finished jobs listed on the queue screen.
const RECENT_FINISHED: usize = 10;
/// How often free memory is read.
const RAM_EVERY_MS: Millis = 1_000;
/// How often the power source is read.
const POWER_EVERY_MS: Millis = 30_000;

struct Tracked {
    job: Job,
    progress: JobProgress,
    started_at: Option<Millis>,
    breaker: LoopBreaker,
    /// The session folder, once started.
    cwd: Option<PathBuf>,
    activity: JobActivity,
    /// The latest runner message, for stall detection.
    last_event_at: Option<Millis>,
    finished_at: Option<Millis>,
    /// How the job ended, once it has.
    outcome: Option<JobOutcome>,
    /// What Claude Code last said the session is doing.
    session_state: Option<SessionState>,
    /// Shell commands waiting for their result, by tool_use id.
    shell: HashMap<String, String>,
    /// The governor's latest word on the job, while queued and once started.
    admission: Option<Admission>,
    /// A window that refused the job's requests, if one did.
    refused: Option<RateLimit>,
}

/// What a finished job reported, as the inbox needs it.
struct JobOutcome {
    error: Option<String>,
    text: String,
}

/// One row of the queue screen.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub job_id: JobId,
    pub session_name: String,
    pub prompt: String,
    pub model: String,
    pub status: JobStatus,
    pub priority: i32,
    pub cost_usd: f64,
    /// Why the governor holds or downgraded the job:
    /// "deferred: 5h at 94%, resumes 15:58".
    pub note: Option<String>,
}

/// Minutes of token flow a [`JobFact`] carries.
const FACT_FLOW_MINUTES: usize = 30;

/// One job of this run, with the timestamps and outcome the inbox, the wall
/// and the change log are built from.
#[derive(Clone, Debug, PartialEq)]
pub struct JobFact {
    pub job_id: JobId,
    pub session_id: SessionId,
    pub session_name: String,
    pub prompt: String,
    pub model: String,
    pub status: JobStatus,
    pub created_at: Millis,
    pub started_at: Option<Millis>,
    /// The latest message from the running `claude`.
    pub last_event_at: Option<Millis>,
    pub finished_at: Option<Millis>,
    /// Why it failed, once finished.
    pub error: Option<String>,
    /// The final answer, once finished.
    pub result: Option<String>,
    pub review: Option<ReviewState>,
    /// Set on a review job: the job it audits.
    pub review_of: Option<JobId>,
    pub cost_usd: f64,
    /// Lead context used, `0.0..` of the window, when known.
    pub context: Option<f64>,
    /// Tokens per minute over the last half hour, oldest first.
    pub flow: Vec<u64>,
    /// Items of the lead's todo list: done, and all.
    pub todos_done: u32,
    pub todos_total: u32,
    /// Times the loop breaker stopped an agent of this job.
    pub loop_stops: u32,
    /// Subagents running now.
    pub agents_running: u32,
    /// What Claude Code last said the session is doing, while it runs.
    pub session_state: Option<SessionState>,
}

/// Counts for the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueStats {
    pub running: usize,
    pub queued: usize,
    pub max_concurrent: usize,
    /// `None` until read, or when it can't be read.
    pub free_ram: Option<u64>,
    /// Free memory is below the configured minimum, so nothing starts.
    pub ram_low: bool,
    /// Queued jobs the governor holds until a usage window resets.
    pub deferred: usize,
    /// Stopped by a usage limit; resumed by themselves after the reset.
    pub awaiting_reset: usize,
    /// On battery, so fewer jobs run at once.
    pub on_battery: bool,
    /// Queued jobs held back because the user is working the inbox as much
    /// as they can take.
    pub held: usize,
}

/// What one [`JobService::pump`] did.
#[derive(Debug, Default)]
pub struct PumpReport {
    /// Errors to show the user.
    pub errors: Vec<AppError>,
    /// Sessions created by forks.
    pub new_sessions: Vec<Session>,
    /// Some job made progress, started or ended.
    pub changed: bool,
    /// Jobs the loop breaker stopped.
    pub loop_trips: Vec<(JobId, LoopTrip)>,
}

/// What applying one runner message led to.
#[derive(Default)]
struct Applied {
    forked: Option<Session>,
    trip: Option<LoopTrip>,
}

/// Use cases for jobs: queueing prompts, running them when allowed and
/// following them to the end.
pub struct JobService {
    sessions: Rc<dyn SessionRepository>,
    jobs: Rc<dyn JobRepository>,
    runner: Rc<dyn AgentRunner>,
    clock: Rc<dyn Clock>,
    monitor: Option<Rc<dyn SystemMonitor>>,
    limits: Limits,
    loop_repeats: u32,
    tx: Sender<(JobId, RunnerMsg)>,
    rx: Receiver<(JobId, RunnerMsg)>,
    tracked: Vec<Tracked>,
    /// Last memory reading and when it was taken.
    ram: Option<(Millis, Option<u64>)>,
    catalog: Option<Rc<dyn AgentCatalog>>,
    /// Where review jobs read the diff; without it there are no reviews.
    workspace: Option<Rc<dyn Workspace>>,
    review: ReviewSettings,
    /// Token flow, tool calls, limits and environments across jobs.
    activity: SwarmActivity,
    /// Where limit readings and environments outlive the run.
    telemetry: Option<Rc<dyn TelemetryStore>>,
    /// Where the shell commands agents ran are reported, as evidence.
    executions: Option<Rc<dyn ExecutionLog>>,
    /// Gives each job its own git worktree.
    worktrees: Option<Rc<WorktreeService>>,
    /// When worktrees were last reclaimed.
    last_gc: Option<Millis>,
    /// Admission by usage window, budgets and auto-resume after a reset.
    governor: Option<Governor>,
    /// The share of their time the user spends in the inbox, when known.
    load: Option<f64>,
    /// Dollar caps of single jobs (a mission's share of its reservation).
    caps: HashMap<JobId, f64>,
    /// Last power reading and when it was taken: on battery or not.
    power: Option<(Millis, bool)>,
}

/// Where a queued job comes from, and what it may spend.
#[derive(Clone, Copy, Default)]
struct Lineage {
    wants_review: bool,
    review_of: Option<JobId>,
    /// The interrupted job whose worktree it continues.
    inherits: Option<JobId>,
    /// `--max-budget-usd` of the job.
    cap: Option<f64>,
}

/// How often stale worktrees are looked for.
const GC_EVERY_MS: Millis = 3_600_000;
/// What a resumed job is told.
const RESUME_PROMPT: &str = "The previous run stopped before it finished. Check the files in the working folder to see what is already done, then carry on from there.";

/// Environments restored at start.
const RESTORE_ENVIRONMENTS: usize = 50;

impl JobService {
    pub fn new(
        sessions: Rc<dyn SessionRepository>,
        jobs: Rc<dyn JobRepository>,
        runner: Rc<dyn AgentRunner>,
        clock: Rc<dyn Clock>,
    ) -> JobService {
        let (tx, rx) = channel();
        JobService {
            sessions,
            jobs,
            runner,
            clock,
            monitor: None,
            limits: Settings::default().limits(),
            loop_repeats: Settings::default().guard.loop_repeats,
            tx,
            rx,
            tracked: Vec::new(),
            ram: None,
            catalog: None,
            workspace: None,
            review: ReviewSettings::default(),
            activity: SwarmActivity::default(),
            telemetry: None,
            executions: None,
            worktrees: None,
            last_gc: None,
            governor: None,
            load: None,
            caps: HashMap::new(),
            power: None,
        }
    }

    /// Admits queued jobs by usage window (holding, deferring or
    /// downgrading them), caps their spend, runs fewer on battery and
    /// resumes jobs a usage limit stopped once the window resets.
    pub fn with_governor(mut self, governor: Governor) -> JobService {
        self.governor = Some(governor);
        self
    }

    /// Tells the queue how much of their time the user spends in the inbox
    /// (`0.0..=1.0`, `None` while that isn't known). Above the governor's
    /// line new jobs wait and the queue says so; reviews still run. Held
    /// jobs start at the pump after the load drops.
    pub fn set_supervisor_load(&mut self, load: Option<f64>) {
        self.load = load;
    }

    pub fn governor(&self) -> Option<&Governor> {
        self.governor.as_ref()
    }

    /// Runs each job in its own git worktree (when its session wants one),
    /// and reclaims stale worktrees at start and every hour.
    pub fn with_worktrees(mut self, worktrees: Rc<WorktreeService>) -> JobService {
        self.worktrees = Some(worktrees);
        self
    }

    /// Keeps usage-limit readings and session environments in `store`;
    /// [`JobService::recover`] loads the stored ones.
    /// Reports every shell command agents run, and whether it succeeded.
    pub fn with_execution_log(mut self, log: Rc<dyn ExecutionLog>) -> JobService {
        self.executions = Some(log);
        self
    }

    pub fn with_telemetry_store(mut self, store: Rc<dyn TelemetryStore>) -> JobService {
        self.telemetry = Some(store);
        self
    }

    /// Activity across the jobs of this run, plus restored limits and environments.
    pub fn activity(&self) -> &SwarmActivity {
        &self.activity
    }

    /// Token flow, tool calls, todos and environment of a job of this run.
    pub fn job_activity(&self, id: JobId) -> Option<&JobActivity> {
        self.find(id).map(|i| &self.tracked[i].activity)
    }

    /// The job's lead, its subagents and what each is running now.
    pub fn agent_tree(&self, id: JobId) -> Option<AgentNode> {
        self.find(id)
            .map(|i| agent_tree(&self.tracked[i].progress, &self.tracked[i].activity))
    }

    /// Reviews finished jobs (when asked for) with `settings.model`, reading
    /// the diff from `workspace`. `settings.enabled` is the starting default.
    pub fn with_review(
        mut self,
        workspace: Rc<dyn Workspace>,
        settings: &ReviewSettings,
    ) -> JobService {
        self.workspace = Some(workspace);
        self.review = settings.clone();
        self
    }

    /// Looks up which model each subagent runs on.
    pub fn with_agent_catalog(mut self, catalog: Rc<dyn AgentCatalog>) -> JobService {
        self.catalog = Some(catalog);
        self
    }

    /// The uncommitted changes in `cwd`, when a workspace is configured.
    pub fn working_diff(&self, cwd: &std::path::Path) -> AppResult<Option<String>> {
        self.workspace.as_ref().map(|w| w.git_diff(cwd)).transpose()
    }

    /// Whether newly dispatched jobs get a review.
    pub fn review_next(&self) -> bool {
        self.review.enabled && self.workspace.is_some()
    }

    /// Turns reviews of new jobs on or off. Returns the new setting, which
    /// stays off without a workspace to read diffs from.
    pub fn set_review_next(&mut self, on: bool) -> bool {
        self.review.enabled = on;
        self.review_next()
    }

    /// The reviewer's model.
    pub fn review_model(&self) -> &str {
        &self.review.model
    }

    pub fn with_limits(mut self, limits: Limits) -> JobService {
        self.limits = limits;
        self
    }

    /// Identical tool failures in a row that stop a job (0 = never).
    pub fn with_loop_repeats(mut self, repeats: u32) -> JobService {
        self.loop_repeats = repeats;
        self
    }

    /// Without a monitor free memory is unknown and never holds jobs back.
    pub fn with_monitor(mut self, monitor: Rc<dyn SystemMonitor>) -> JobService {
        self.monitor = Some(monitor);
        self
    }

    /// Marks jobs a previous run left `running` as `interrupted` and picks up
    /// jobs still queued. Returns how many were interrupted.
    pub fn recover(&mut self) -> AppResult<usize> {
        if let Some(store) = &self.telemetry {
            for (limit, at) in store.limits()? {
                self.activity.record_limit(limit, at);
            }
            for (env, at) in store.environments(RESTORE_ENVIRONMENTS)? {
                self.activity.record_environment(env, at);
            }
        }
        if let Some(g) = &mut self.governor {
            g.restore()?;
        }
        let interrupted = self.jobs.interrupt_running(self.clock.now())?;
        let mut queued = self.jobs.queued()?;
        queued.sort_by_key(|j| (j.created_at, j.id));
        for job in queued {
            if self.find(job.id).is_none() {
                let name = self
                    .sessions
                    .get(job.session_id)?
                    .map_or_else(|| "?".to_string(), |s| s.name);
                self.track(job, &name);
            }
        }
        Ok(interrupted)
    }

    /// Queues `prompt` for the session; it starts as soon as it is allowed to.
    pub fn dispatch(&mut self, session_id: SessionId, prompt: &str) -> AppResult<JobId> {
        self.dispatch_capped(session_id, prompt, None)
    }

    /// Like [`JobService::dispatch`], spending at most `cap` dollars
    /// (`--max-budget-usd`).
    pub fn dispatch_capped(
        &mut self,
        session_id: SessionId,
        prompt: &str,
        cap: Option<f64>,
    ) -> AppResult<JobId> {
        let lineage = Lineage {
            wants_review: self.review_next(),
            cap,
            ..Lineage::default()
        };
        self.enqueue(session_id, prompt, None, None, false, lineage)
    }

    /// Like [`JobService::dispatch`], and records which template the job came from.
    pub fn dispatch_template(
        &mut self,
        session_id: SessionId,
        rendered: &RenderedPrompt,
    ) -> AppResult<JobId> {
        let lineage = Lineage {
            wants_review: self.review_next(),
            ..Lineage::default()
        };
        self.enqueue(
            session_id,
            rendered.text(),
            Some(rendered.prompt_id()),
            None,
            false,
            lineage,
        )
    }

    /// Queues a finished job's prompt again: same session, model and template.
    /// A job that was interrupted continues in the worktree it had.
    pub fn retry(&mut self, id: JobId) -> AppResult<JobId> {
        self.requeue(id, false)
    }

    /// Continues a job that stopped early (interrupted by a quit or a crash,
    /// or failed) in its Claude session and its worktree, with a short
    /// instruction to pick up where it left off.
    pub fn resume(&mut self, id: JobId) -> AppResult<JobId> {
        let job = self.jobs.get(id)?.ok_or(AppError::JobNotFound)?;
        if !matches!(job.status, JobStatus::Interrupted | JobStatus::Failed) {
            return Err(AppError::NotResumable(id));
        }
        if let Some(g) = &mut self.governor {
            g.forget(id)?;
        }
        let lineage = Lineage {
            wants_review: job.wants_review,
            review_of: job.review_of,
            inherits: Some(id),
            cap: self.caps.get(&id).copied(),
        };
        self.enqueue(
            job.session_id,
            RESUME_PROMPT,
            None,
            Some(job.model),
            false,
            lineage,
        )
    }

    /// Queues a finished job's prompt in a branch of its Claude session. Once
    /// Claude reports the new session it becomes a swarmux session of its own.
    pub fn fork(&mut self, id: JobId) -> AppResult<JobId> {
        self.requeue(id, true)
    }

    /// Cancels a queued job, or stops a running one.
    pub fn cancel(&mut self, id: JobId) -> AppResult<()> {
        let Some(i) = self.find(id) else {
            return Err(match self.jobs.get(id)? {
                Some(_) => AppError::JobFinished(id),
                None => AppError::JobNotFound,
            });
        };
        let status = self.tracked[i].progress.status;
        if status.is_finished() {
            return Err(AppError::JobFinished(id));
        }
        if status == JobStatus::Running {
            self.runner.cancel(id);
        }
        let tick = self.tick_of(i);
        let t = &mut self.tracked[i];
        if let Some(result) = t.progress.cancel(tick) {
            t.job.status = JobStatus::Cancelled;
            self.jobs.save_result(id, &result)?;
            self.jobs
                .set_status(id, JobStatus::Cancelled, self.clock.now())?;
        }
        Ok(())
    }

    /// Raises (positive `delta`) or lowers a queued job's priority.
    pub fn change_priority(&mut self, id: JobId, delta: i32) -> AppResult<i32> {
        let i = self
            .find(id)
            .filter(|&i| self.tracked[i].job.status == JobStatus::Queued)
            .ok_or(AppError::JobNotQueued(id))?;
        let priority = self.tracked[i].job.priority.saturating_add(delta);
        self.jobs.set_priority(id, priority)?;
        self.tracked[i].job.priority = priority;
        Ok(priority)
    }

    /// Applies pending runner messages, then starts whatever may start.
    pub fn pump(&mut self) -> PumpReport {
        let mut report = PumpReport::default();
        while let Ok((job_id, msg)) = self.rx.try_recv() {
            report.changed = true;
            match self.apply(job_id, msg) {
                Ok(applied) => {
                    report.new_sessions.extend(applied.forked);
                    report.loop_trips.extend(applied.trip.map(|t| (job_id, t)));
                }
                Err(e) => report.errors.push(e),
            }
        }
        self.reclaim_worktrees(&mut report);
        self.resume_after_reset(&mut report);
        // Read even with nothing queued, so the status bar always shows it.
        self.free_ram();
        match self.schedule() {
            Ok(started) => report.changed |= started,
            Err(e) => report.errors.push(e),
        }
        self.prune();
        report
    }

    /// Resumes the jobs a usage limit stopped whose window has reset.
    fn resume_after_reset(&mut self, report: &mut PumpReport) {
        let now = self.clock.now();
        let due = match self.governor.as_mut().map(|g| g.due(now)) {
            Some(Ok(due)) => due,
            Some(Err(e)) => {
                report.errors.push(e);
                return;
            }
            None => return,
        };
        for p in due {
            match self.resume(p.job) {
                Ok(new) => {
                    report.changed = true;
                    let text = format!("resumed #{} after the {} reset", p.job, p.window);
                    self.note(new, "governor", &text);
                }
                // Resumed or retried by hand meanwhile.
                Err(AppError::NotResumable(_) | AppError::JobNotFound) => {}
                Err(e) => report.errors.push(e),
            }
        }
    }

    /// Reclaims stale worktrees: at the first pump, then every hour.
    fn reclaim_worktrees(&mut self, report: &mut PumpReport) {
        let Some(worktrees) = &self.worktrees else {
            return;
        };
        let now = self.clock.now();
        if self.last_gc.is_some_and(|at| now - at < GC_EVERY_MS) {
            return;
        }
        self.last_gc = Some(now);
        match worktrees.gc(false) {
            Ok(gc) => report
                .errors
                .extend(gc.errors.into_iter().map(AppError::Workspace)),
            Err(e) => report.errors.push(e),
        }
    }

    /// Adds a swarmux line to a job's log (this run only).
    pub fn note(&mut self, id: JobId, kind: &str, text: &str) {
        if let Some(i) = self.find(id) {
            self.tracked[i].progress.note(kind, text);
        }
    }

    /// The most recently dispatched job (reviews aside).
    pub fn latest(&self) -> Option<&JobProgress> {
        self.tracked
            .iter()
            .rev()
            .find(|t| t.job.review_of.is_none())
            .map(|t| &t.progress)
    }

    /// The job a review job audits, or the job itself.
    pub fn subject_of(&self, id: JobId) -> JobId {
        self.find(id)
            .and_then(|i| self.tracked[i].job.review_of)
            .unwrap_or(id)
    }

    /// The review of `job` from this run, queued, running or finished.
    pub fn review_progress(&self, job: JobId) -> Option<&JobProgress> {
        self.tracked
            .iter()
            .rev()
            .find(|t| t.job.review_of == Some(job))
            .map(|t| &t.progress)
    }

    /// Notes a guard, routing or JEV decision made for `job`.
    pub fn observe_decision(&mut self, job: JobId) {
        if let Some(i) = self.find(job) {
            let tick = self.tick_of(i);
            self.tracked[i].progress.record_decision(tick);
        }
    }

    /// Where the timeline of `job` ends: now while it (or its review) runs,
    /// else when the last of them finished.
    pub fn timeline_end(&self, job: JobId) -> u32 {
        let end = |i: usize| {
            let p = &self.tracked[i].progress;
            let own = if p.status == JobStatus::Running {
                self.tick_of(i).max(p.last_tick)
            } else {
                p.last_tick
            };
            own + p.log_offset
        };
        let Some(i) = self.find(job) else {
            return 0;
        };
        let review = self
            .tracked
            .iter()
            .rposition(|t| t.job.review_of == Some(job) && t.started_at.is_some())
            .map_or(0, end);
        end(i).max(review)
    }

    /// A job queued, running or recently finished in this run.
    pub fn progress(&self, id: JobId) -> Option<&JobProgress> {
        self.find(id).map(|i| &self.tracked[i].progress)
    }

    /// Running jobs, then queued ones in run order, then recently finished ones.
    pub fn queue(&self) -> Vec<QueueEntry> {
        let entry = |t: &Tracked| QueueEntry {
            job_id: t.job.id,
            session_name: t.progress.session_name.clone(),
            prompt: t.job.prompt.clone(),
            model: t.job.model.clone(),
            status: t.progress.status,
            priority: t.job.priority,
            cost_usd: t.progress.cost_usd,
            note: match (&self.governor, &t.admission) {
                (Some(g), Some(a)) if !t.progress.status.is_finished() => g.reason(a),
                _ => None,
            },
        };
        let running = self
            .tracked
            .iter()
            .filter(|t| t.progress.status == JobStatus::Running);
        let queued = run_order(self.queued_jobs())
            .into_iter()
            .filter_map(|j| self.find(j.id).map(|i| &self.tracked[i]));
        let finished = self
            .tracked
            .iter()
            .rev()
            .filter(|t| t.progress.status.is_finished())
            .take(RECENT_FINISHED);
        running.chain(queued).chain(finished).map(entry).collect()
    }

    /// What the inbox and the wall need to know about every job of this
    /// run, oldest first.
    pub fn facts(&self) -> Vec<JobFact> {
        let now = self.clock.now();
        self.tracked
            .iter()
            .map(|t| {
                let p = &t.progress;
                let flow = t.activity.flow.series(now);
                let last = flow.len().saturating_sub(FACT_FLOW_MINUTES);
                JobFact {
                    job_id: t.job.id,
                    session_id: t.job.session_id,
                    session_name: p.session_name.clone(),
                    prompt: t.job.prompt.clone(),
                    model: t.job.model.clone(),
                    status: p.status,
                    created_at: t.job.created_at,
                    started_at: t.started_at,
                    last_event_at: t.last_event_at,
                    finished_at: t.finished_at,
                    error: t.outcome.as_ref().and_then(|o| o.error.clone()),
                    result: t.outcome.as_ref().map(|o| o.text.clone()),
                    review: p.review.clone(),
                    review_of: t.job.review_of,
                    cost_usd: p.cost_usd,
                    context: p
                        .context_window
                        .or_else(|| context_window(&p.model, p.context_used))
                        .filter(|w| *w > 0)
                        .map(|w| p.context_used as f64 / w as f64)
                        .filter(|_| p.context_used > 0),
                    flow: flow[last..].iter().map(|b| b.total()).collect(),
                    todos_done: t
                        .activity
                        .todos
                        .iter()
                        .filter(|d| d.status == TodoStatus::Completed)
                        .count() as u32,
                    todos_total: t.activity.todos.len() as u32,
                    loop_stops: p.loop_stops.len() as u32,
                    agents_running: p.subagents_running() as u32,
                    session_state: t.session_state.filter(|_| p.status == JobStatus::Running),
                }
            })
            .collect()
    }

    pub fn stats(&self) -> QueueStats {
        let count = |s: JobStatus| {
            self.tracked
                .iter()
                .filter(|t| t.progress.status == s)
                .count()
        };
        let free_ram = self.ram.and_then(|(_, free)| free);
        let on_battery = self.power.is_some_and(|(_, b)| b);
        QueueStats {
            running: count(JobStatus::Running),
            queued: count(JobStatus::Queued),
            max_concurrent: self.effective_limits(on_battery).max_concurrent,
            free_ram,
            ram_low: free_ram.is_some_and(|f| f < self.limits.min_free_ram),
            deferred: self
                .tracked
                .iter()
                .filter(|t| t.progress.status == JobStatus::Queued)
                .filter(|t| t.admission.as_ref().is_some_and(Admission::is_deferred))
                .count(),
            awaiting_reset: self.governor.as_ref().map_or(0, |g| g.pending().len()),
            on_battery,
            held: self
                .tracked
                .iter()
                .filter(|t| t.progress.status == JobStatus::Queued)
                .filter(|t| t.admission.as_ref().is_some_and(Admission::is_held))
                .count(),
        }
    }

    /// Tenths of a second since `job` started (for the timeline's "now").
    pub fn tick_now(&self, job: JobId) -> u32 {
        self.find(job).map_or(0, |i| self.tick_of(i))
    }

    fn find(&self, id: JobId) -> Option<usize> {
        self.tracked.iter().position(|t| t.job.id == id)
    }

    fn tick_of(&self, i: usize) -> u32 {
        self.tracked[i]
            .started_at
            .map_or(0, |at| ((self.clock.now() - at).max(0) / 100) as u32)
    }

    fn queued_jobs(&self) -> impl Iterator<Item = &Job> {
        self.tracked
            .iter()
            .filter(|t| t.progress.status == JobStatus::Queued)
            .map(|t| &t.job)
    }

    fn track(&mut self, job: Job, session_name: &str) {
        let mut progress = JobProgress::new(job.id, session_name, &job.model);
        progress.status = JobStatus::Queued;
        if job.wants_review {
            progress.review = Some(ReviewState::Requested);
        }
        if job.review_of.is_some() {
            progress.as_reviewer(0);
        }
        self.tracked.push(Tracked {
            job,
            progress,
            started_at: None,
            breaker: LoopBreaker::new(self.loop_repeats),
            cwd: None,
            activity: JobActivity::default(),
            last_event_at: None,
            finished_at: None,
            outcome: None,
            session_state: None,
            shell: HashMap::new(),
            admission: None,
            refused: None,
        });
    }

    fn requeue(&mut self, id: JobId, fork: bool) -> AppResult<JobId> {
        let job = self.jobs.get(id)?.ok_or(AppError::JobNotFound)?;
        if !job.status.is_finished() {
            return Err(AppError::JobNotFinished(id));
        }
        if let Some(g) = &mut self.governor {
            g.forget(id)?;
        }
        let lineage = Lineage {
            wants_review: job.wants_review,
            review_of: job.review_of,
            inherits: (job.status == JobStatus::Interrupted && !fork).then_some(id),
            cap: self.caps.get(&id).copied(),
        };
        self.enqueue(
            job.session_id,
            &job.prompt,
            job.prompt_id,
            Some(job.model),
            fork,
            lineage,
        )
    }

    fn enqueue(
        &mut self,
        session_id: SessionId,
        prompt: &str,
        prompt_id: Option<PromptId>,
        model: Option<String>,
        fork: bool,
        lineage: Lineage,
    ) -> AppResult<JobId> {
        let Lineage {
            wants_review,
            review_of,
            inherits,
            cap,
        } = lineage;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(DomainError::EmptyPrompt.into());
        }
        let session = self
            .sessions
            .get(session_id)?
            .ok_or(AppError::SessionNotFound)?;
        let now = self.clock.now();
        let new = NewJob {
            session_id,
            prompt_id,
            prompt: prompt.to_string(),
            model: model.unwrap_or(session.model),
            priority: 0,
            fork,
            wants_review,
            review_of,
        };
        let id = self.jobs.insert(&new, now)?;
        if let Some(cap) = cap.filter(|c| c.is_finite() && *c > 0.0) {
            self.caps.insert(id, cap);
        }
        self.track(
            Job {
                id,
                session_id,
                prompt_id,
                prompt: new.prompt,
                model: new.model,
                priority: new.priority,
                fork,
                status: JobStatus::Queued,
                created_at: now,
                wants_review,
                review_of,
            },
            &session.name,
        );
        // Before it can start: the job continues the other one's worktree.
        if let (Some(from), Some(worktrees)) = (inherits, &self.worktrees) {
            worktrees.rebind(from, id)?;
        }
        self.schedule()?;
        Ok(id)
    }

    /// Where the job runs, and with which extra environment.
    fn workdir(&self, i: usize, session: &Session) -> AppResult<(PathBuf, Vec<(String, String)>)> {
        let own = || (session.cwd.clone(), Vec::new());
        let Some(worktrees) = &self.worktrees else {
            return Ok(own());
        };
        let job = &self.tracked[i].job;
        if let Some(audited) = job.review_of {
            // A reviewer reads the worktree of the job it audits.
            return Ok(worktrees
                .record_for(audited)?
                .map_or_else(own, |r| (r.path, Vec::new())));
        }
        Ok(worktrees
            .prepare(job.id, session)?
            .map_or_else(own, |p| (p.cwd, p.env)))
    }

    /// Starts every queued job the scheduling rule allows. True if any started.
    fn schedule(&mut self) -> AppResult<bool> {
        if self.queued_jobs().next().is_none() {
            return Ok(false);
        }
        let free_ram = self.free_ram();
        let on_battery = self.on_battery();
        let limits = self.effective_limits(on_battery);
        self.admit_queued();
        let running: Vec<SessionId> = self
            .tracked
            .iter()
            .filter(|t| t.progress.status == JobStatus::Running)
            .map(|t| t.job.session_id)
            .collect();
        let admitted = self
            .tracked
            .iter()
            .filter(|t| t.progress.status == JobStatus::Queued)
            .filter(|t| {
                !t.admission
                    .as_ref()
                    .is_some_and(|a| a.is_deferred() || a.is_held())
            })
            .map(|t| &t.job);
        let picks = plan(admitted, &running, free_ram, limits);
        for &id in &picks {
            self.start(id)?;
        }
        Ok(!picks.is_empty())
    }

    /// The run limits now: the governor runs fewer at once on battery.
    fn effective_limits(&self, on_battery: bool) -> Limits {
        self.governor
            .as_ref()
            .map_or(self.limits, |g| g.limits(self.limits, on_battery))
    }

    /// The freshest reading of every usage window.
    fn windows(&self) -> Vec<LimitState> {
        self.governor
            .as_ref()
            .map(|g| g.windows(&self.activity.limits()))
            .unwrap_or_default()
    }

    /// Asks the governor about every queued job.
    fn admit_queued(&mut self) {
        let Some(g) = &self.governor else {
            return;
        };
        let now = self.clock.now();
        let load = self.load;
        let windows = g.windows(&self.activity.limits());
        for t in &mut self.tracked {
            if t.progress.status != JobStatus::Queued {
                continue;
            }
            let class = if t.job.review_of.is_some() {
                JobClass::Verify
            } else {
                JobClass::Work
            };
            t.admission = Some(g.admit_under_load(&windows, &t.job.model, class, load, now));
        }
    }

    /// Whether the machine runs on battery, read at most every 30 s.
    fn on_battery(&mut self) -> bool {
        let Some(monitor) = &self.monitor else {
            return false;
        };
        let now = self.clock.now();
        match self.power {
            Some((at, on)) if now - at < POWER_EVERY_MS => on,
            _ => {
                let on = monitor.on_battery().unwrap_or(false);
                self.power = Some((now, on));
                on
            }
        }
    }

    fn free_ram(&mut self) -> Option<u64> {
        let monitor = self.monitor.as_ref()?;
        let now = self.clock.now();
        match self.ram {
            Some((at, free)) if now - at < RAM_EVERY_MS => free,
            _ => {
                let free = monitor.free_memory_bytes();
                self.ram = Some((now, free));
                free
            }
        }
    }

    fn start(&mut self, id: JobId) -> AppResult<()> {
        let Some(i) = self.find(id) else {
            return Ok(());
        };
        let now = self.clock.now();
        let session_id = self.tracked[i].job.session_id;
        let Some(session) = self.sessions.get(session_id)? else {
            if let Some(result) = self.tracked[i].progress.fail("session not found", 0) {
                self.tracked[i].job.status = JobStatus::Failed;
                self.jobs.save_result(id, &result)?;
                self.jobs.set_status(id, JobStatus::Failed, now)?;
            }
            return Ok(());
        };
        let resume = self.sessions.has_started(session_id)?;
        let (cwd, env) = match self.workdir(i, &session) {
            Ok(dir) => dir,
            Err(e) => {
                if let Some(result) = self.tracked[i].progress.fail(&format!("worktree: {e}"), 0) {
                    self.tracked[i].job.status = JobStatus::Failed;
                    self.jobs.save_result(id, &result)?;
                    self.jobs.set_status(id, JobStatus::Failed, now)?;
                }
                return Ok(());
            }
        };
        self.jobs.set_status(id, JobStatus::Running, now)?;
        self.sessions.touch(session_id, now)?;

        let offset = self.tracked[i]
            .job
            .review_of
            .and_then(|of| self.find(of))
            .and_then(|j| self.tracked[j].started_at)
            .map_or(0, |at| ((now - at).max(0) / 100) as u32);
        let cap = self
            .caps
            .get(&id)
            .copied()
            .or_else(|| self.governor.as_ref().and_then(Governor::default_cap));
        let line = match (&self.governor, self.tracked[i].job.review_of) {
            (Some(g), None) => g.line(&self.windows(), cap, now),
            _ => None,
        };
        let downgrade = match (&self.governor, &self.tracked[i].admission) {
            (Some(g), Some(a @ Admission::Downgrade { to, .. })) => Some((to.clone(), g.reason(a))),
            _ => None,
        };
        let t = &mut self.tracked[i];
        if let Some((to, why)) = downgrade {
            t.job.model = to.clone();
            t.progress.model = to;
            t.progress
                .note("governor", why.as_deref().unwrap_or("downgraded"));
        }
        t.started_at = Some(now);
        t.last_event_at = Some(now);
        t.job.status = JobStatus::Running;
        t.progress.status = JobStatus::Running;
        t.cwd = Some(cwd.clone());
        if t.job.review_of.is_some() {
            t.progress.as_reviewer(offset);
        }
        let request = RunRequest {
            job_id: id,
            cwd,
            session_id,
            resume,
            fork: t.job.fork,
            model: t.job.model.clone(),
            permission_mode: session.permission_mode,
            prompt: match &line {
                Some(line) => format!("{}\n\n{line}", t.job.prompt),
                None => t.job.prompt.clone(),
            },
            lean: session.lean,
            review: t.job.review_of.is_some(),
            env,
            max_budget_usd: cap,
        };
        t.progress.command = self.runner.command_line(&request);
        let tx = self.tx.clone();
        let sink: RunSink = Box::new(move |msg| {
            // The service may be gone at shutdown; nothing left to update then.
            let _ = tx.send((id, msg));
        });
        self.runner.start(request, sink);
        Ok(())
    }

    /// Drops the oldest finished jobs beyond [`KEEP_FINISHED`].
    fn prune(&mut self) {
        let finished = self
            .tracked
            .iter()
            .filter(|t| t.progress.status.is_finished())
            .count();
        let mut excess = finished.saturating_sub(KEEP_FINISHED);
        self.tracked.retain(|t| {
            if excess > 0 && t.progress.status.is_finished() {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Applies one runner message: may create a fork's session or trip the
    /// loop breaker, which stops the job.
    fn apply(&mut self, job_id: JobId, msg: RunnerMsg) -> AppResult<Applied> {
        let now = self.clock.now();
        let mut applied = Applied::default();
        let Some(i) = self.find(job_id) else {
            return Ok(applied);
        };
        if self.tracked[i].progress.status == JobStatus::Cancelled {
            return Ok(applied);
        }
        let tick = self.tick_of(i);
        let was_finished = self.tracked[i].progress.status.is_finished();
        self.tracked[i].last_event_at = Some(now);

        let finished = match msg {
            RunnerMsg::Event(ev) => {
                if let AgentEvent::Init { session_id, .. } = &ev
                    && self.tracked[i].job.fork
                {
                    applied.forked = self.adopt_fork(i, session_id)?;
                }
                if !matches!(ev, AgentEvent::Ignored { .. }) {
                    self.jobs.append_event(
                        job_id,
                        ev.kind(),
                        ev.actor().key(),
                        &describe(&ev),
                        now,
                    )?;
                }
                if let AgentEvent::SessionState(state) = &ev {
                    self.tracked[i].session_state = Some(*state);
                }
                if let AgentEvent::RateLimit(limit) = &ev
                    && limit.status == "rejected"
                {
                    self.tracked[i].refused = Some(limit.clone());
                }
                self.observe_shell(i, job_id, &ev);
                self.observe_telemetry(i, job_id, &ev, now)?;
                let finished = self.tracked[i].progress.apply(&ev, tick);
                if let AgentEvent::ToolUse { id, .. } = &ev
                    && ev.starts_subagent()
                {
                    self.resolve_subagent_model(i, id);
                }
                let t = &mut self.tracked[i];
                match t.breaker.observe(&ev) {
                    Some(trip) if finished.is_none() => {
                        self.runner.cancel(job_id);
                        t.progress.record_loop_stop(&trip.reason());
                        let stopped = t.progress.stop(&trip.reason(), tick);
                        applied.trip = Some(trip);
                        stopped
                    }
                    _ => finished,
                }
            }
            RunnerMsg::Exited { code, stderr_tail } => {
                let reason = match code {
                    Some(c) => format!("claude exited with code {c}"),
                    None => "claude was terminated".to_string(),
                };
                let tail = stderr_tail.trim();
                let error = if tail.is_empty() {
                    reason
                } else {
                    format!("{reason}: {tail}")
                };
                self.tracked[i].progress.fail(&error, tick)
            }
            RunnerMsg::SpawnFailed(msg) => self.tracked[i]
                .progress
                .fail(&format!("spawn failed: {msg}"), tick),
        };

        if let Some(result) = finished {
            if was_finished {
                // A later report (after background subagents): new numbers only.
                return self.jobs.save_result(job_id, &result).map(|()| applied);
            }
            let status = self.tracked[i].progress.status;
            self.tracked[i].job.status = status;
            self.tracked[i].finished_at = Some(now);
            self.tracked[i].outcome = Some(JobOutcome {
                error: result.error.clone(),
                text: result.text.clone(),
            });
            self.jobs.save_result(job_id, &result)?;
            self.jobs.set_status(job_id, status, now)?;
            self.after_limit(i, status, &result)?;
            self.after_finish(i, &result)?;
        }
        Ok(applied)
    }

    /// A job a usage limit stopped is resumed by itself once the window resets.
    fn after_limit(&mut self, i: usize, status: JobStatus, result: &JobResult) -> AppResult<()> {
        if status != JobStatus::Failed || self.governor.is_none() {
            return Ok(());
        }
        let t = &self.tracked[i];
        let said =
            result.error.as_deref().is_some_and(is_limit_error) || is_limit_error(&result.text);
        if t.refused.is_none() && !said {
            return Ok(());
        }
        let (job, refused) = (t.job.id, t.refused.clone());
        let now = self.clock.now();
        let windows = self.windows();
        let Some(g) = self.governor.as_mut() else {
            return Ok(());
        };
        let Some((at, window)) = g.reset_for(refused.as_ref(), &windows, now) else {
            return Ok(());
        };
        g.resume_at(job, at, &window)?;
        let text = format!("usage limit: resumes after the {window} reset");
        self.tracked[i].progress.note("governor", &text);
        Ok(())
    }

    /// Pairs shell commands with their results and reports them.
    fn observe_shell(&mut self, i: usize, job_id: JobId, ev: &AgentEvent) {
        let Some(log) = &self.executions else {
            return;
        };
        match ev {
            AgentEvent::ToolUse {
                id, name, summary, ..
            } if matches!(name.as_str(), "Bash" | "PowerShell") => {
                self.tracked[i].shell.insert(id.clone(), summary.clone());
            }
            AgentEvent::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                if let Some(command) = self.tracked[i].shell.remove(tool_use_id) {
                    log.executed(job_id, &command, !is_error);
                }
            }
            _ => {}
        }
    }

    /// Feeds an event to the job's and the run's activity, and stores limit
    /// readings and environments.
    fn observe_telemetry(
        &mut self,
        i: usize,
        job_id: JobId,
        ev: &AgentEvent,
        now: Millis,
    ) -> AppResult<()> {
        let usage = self.tracked[i].activity.observe(job_id, ev, now);
        self.activity.observe(job_id, ev, now, usage);
        match (ev, &self.telemetry) {
            (AgentEvent::RateLimit(limit), Some(store)) => store.save_limit(limit, now),
            (AgentEvent::Environment(env), Some(store)) => store.save_environment(env, now),
            _ => Ok(()),
        }
    }

    /// Sets a new subagent's model from its definition ("inherit" means the lead's).
    fn resolve_subagent_model(&mut self, i: usize, id: &str) {
        let (Some(catalog), Some(cwd)) = (&self.catalog, &self.tracked[i].cwd) else {
            return;
        };
        let p = &self.tracked[i].progress;
        let Some(kind) = p.subagents.iter().find(|s| s.id == id).map(|s| &s.kind) else {
            return;
        };
        let model = catalog
            .profile(cwd, kind)
            .model
            .map(|m| if m == "inherit" { p.model.clone() } else { m });
        self.tracked[i].progress.set_subagent_model(id, model);
    }

    /// Starts the review of a job that asked for one, or files a finished
    /// review on the job it audited.
    fn after_finish(&mut self, i: usize, result: &JobResult) -> AppResult<()> {
        let job = &self.tracked[i].job;
        if let Some(of) = job.review_of {
            return self.file_review(i, of, result);
        }
        if job.wants_review {
            return self.start_review(i, result);
        }
        Ok(())
    }

    /// A fork's `init` names the new Claude session: bind it to a new
    /// swarmux session and move the job there.
    fn adopt_fork(&mut self, i: usize, claude_session: &str) -> AppResult<Option<Session>> {
        let original_id = self.tracked[i].job.session_id;
        let Some(new_id) = SessionId::parse(claude_session).filter(|id| *id != original_id) else {
            return Ok(None);
        };
        let original = self
            .sessions
            .get(original_id)?
            .ok_or(AppError::SessionNotFound)?;
        let mut session = Session::new(
            new_id,
            &original.fork_name(),
            original.cwd.clone(),
            &original.model,
            self.clock.now(),
        )?;
        session.lean = original.lean;
        self.sessions.insert(&session)?;
        let t = &mut self.tracked[i];
        self.jobs.set_session(t.job.id, new_id)?;
        t.job.session_id = new_id;
        t.progress.session_name = session.name.clone();
        Ok(Some(session))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod worktree_tests;
