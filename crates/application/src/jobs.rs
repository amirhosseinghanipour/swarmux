use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::{
    AgentEvent, DomainError, Job, JobId, JobProgress, JobStatus, Limits, LoopBreaker, LoopTrip,
    Millis, PromptId, Session, SessionId, describe, plan, run_order,
};

use crate::ports::{
    AgentRunner, Clock, JobRepository, NewJob, RunRequest, RunSink, RunnerMsg, SessionRepository,
    SystemMonitor,
};
use crate::{AppError, AppResult, RenderedPrompt, Settings};

/// Finished jobs kept in memory for the queue and dashboard.
const KEEP_FINISHED: usize = 50;
/// Finished jobs listed on the queue screen.
const RECENT_FINISHED: usize = 10;
/// How often free memory is read.
const RAM_EVERY_MS: Millis = 1_000;

struct Tracked {
    job: Job,
    progress: JobProgress,
    started_at: Option<Millis>,
    breaker: LoopBreaker,
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
}

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
        }
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
        self.enqueue(session_id, prompt, None, None, false)
    }

    /// Like [`JobService::dispatch`], and records which template the job came from.
    pub fn dispatch_template(
        &mut self,
        session_id: SessionId,
        rendered: &RenderedPrompt,
    ) -> AppResult<JobId> {
        self.enqueue(
            session_id,
            rendered.text(),
            Some(rendered.prompt_id()),
            None,
            false,
        )
    }

    /// Queues a finished job's prompt again: same session, model and template.
    pub fn retry(&mut self, id: JobId) -> AppResult<JobId> {
        self.requeue(id, false)
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
        match self.schedule() {
            Ok(started) => report.changed |= started,
            Err(e) => report.errors.push(e),
        }
        self.prune();
        report
    }

    /// The most recently dispatched job.
    pub fn latest(&self) -> Option<&JobProgress> {
        self.tracked.last().map(|t| &t.progress)
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

    pub fn stats(&self) -> QueueStats {
        let count = |s: JobStatus| {
            self.tracked
                .iter()
                .filter(|t| t.progress.status == s)
                .count()
        };
        let free_ram = self.ram.and_then(|(_, free)| free);
        QueueStats {
            running: count(JobStatus::Running),
            queued: count(JobStatus::Queued),
            max_concurrent: self.limits.max_concurrent,
            free_ram,
            ram_low: free_ram.is_some_and(|f| f < self.limits.min_free_ram),
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
        self.tracked.push(Tracked {
            job,
            progress,
            started_at: None,
            breaker: LoopBreaker::new(self.loop_repeats),
        });
    }

    fn requeue(&mut self, id: JobId, fork: bool) -> AppResult<JobId> {
        let job = self.jobs.get(id)?.ok_or(AppError::JobNotFound)?;
        if !job.status.is_finished() {
            return Err(AppError::JobNotFinished(id));
        }
        self.enqueue(
            job.session_id,
            &job.prompt,
            job.prompt_id,
            Some(job.model),
            fork,
        )
    }

    fn enqueue(
        &mut self,
        session_id: SessionId,
        prompt: &str,
        prompt_id: Option<PromptId>,
        model: Option<String>,
        fork: bool,
    ) -> AppResult<JobId> {
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
        };
        let id = self.jobs.insert(&new, now)?;
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
            },
            &session.name,
        );
        self.schedule()?;
        Ok(id)
    }

    /// Starts every queued job the scheduling rule allows. True if any started.
    fn schedule(&mut self) -> AppResult<bool> {
        if self.queued_jobs().next().is_none() {
            return Ok(false);
        }
        let free_ram = self.free_ram();
        let running: Vec<SessionId> = self
            .tracked
            .iter()
            .filter(|t| t.progress.status == JobStatus::Running)
            .map(|t| t.job.session_id)
            .collect();
        let picks = plan(self.queued_jobs(), &running, free_ram, self.limits);
        for &id in &picks {
            self.start(id)?;
        }
        Ok(!picks.is_empty())
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
        self.jobs.set_status(id, JobStatus::Running, now)?;
        self.sessions.touch(session_id, now)?;

        let t = &mut self.tracked[i];
        t.started_at = Some(now);
        t.job.status = JobStatus::Running;
        t.progress.status = JobStatus::Running;
        let request = RunRequest {
            job_id: id,
            cwd: session.cwd,
            session_id,
            resume,
            fork: t.job.fork,
            model: t.job.model.clone(),
            permission_mode: session.permission_mode,
            prompt: t.job.prompt.clone(),
        };
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
                let t = &mut self.tracked[i];
                let finished = t.progress.apply(&ev, tick);
                match t.breaker.observe(&ev) {
                    Some(trip) if finished.is_none() => {
                        self.runner.cancel(job_id);
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
            let status = self.tracked[i].progress.status;
            self.tracked[i].job.status = status;
            self.jobs.save_result(job_id, &result)?;
            self.jobs.set_status(job_id, status, now)?;
        }
        Ok(applied)
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
        let session = Session::new(
            new_id,
            &original.fork_name(),
            original.cwd.clone(),
            &original.model,
            self.clock.now(),
        )?;
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
