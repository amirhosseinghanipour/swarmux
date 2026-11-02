use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::{
    DomainError, JobId, JobProgress, JobStatus, Millis, PromptId, SessionId, describe,
};

use crate::ports::{
    AgentRunner, Clock, JobRepository, RunRequest, RunSink, RunnerMsg, SessionRepository,
};
use crate::{AppError, AppResult, RenderedPrompt};

struct Tracked {
    progress: JobProgress,
    session_id: SessionId,
    started_at: Millis,
}

/// Use cases for jobs: dispatching prompts and following them to the end.
pub struct JobService {
    sessions: Rc<dyn SessionRepository>,
    jobs: Rc<dyn JobRepository>,
    runner: Rc<dyn AgentRunner>,
    clock: Rc<dyn Clock>,
    tx: Sender<(JobId, RunnerMsg)>,
    rx: Receiver<(JobId, RunnerMsg)>,
    tracked: Vec<Tracked>,
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
            tx,
            rx,
            tracked: Vec::new(),
        }
    }

    /// Marks jobs a previous run left `running` as `interrupted`.
    pub fn recover(&self) -> AppResult<usize> {
        self.jobs.interrupt_running(self.clock.now())
    }

    /// Sends `prompt` to the session as a new job. A session runs one job at a time.
    pub fn dispatch(&mut self, session_id: SessionId, prompt: &str) -> AppResult<JobId> {
        self.start(session_id, prompt, None)
    }

    /// Like [`JobService::dispatch`], and records which template the job came from.
    pub fn dispatch_template(
        &mut self,
        session_id: SessionId,
        rendered: &RenderedPrompt,
    ) -> AppResult<JobId> {
        self.start(session_id, rendered.text(), Some(rendered.prompt_id()))
    }

    fn start(
        &mut self,
        session_id: SessionId,
        prompt: &str,
        prompt_id: Option<PromptId>,
    ) -> AppResult<JobId> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(DomainError::EmptyPrompt.into());
        }
        let session = self
            .sessions
            .get(session_id)?
            .ok_or(AppError::SessionNotFound)?;
        if self
            .tracked
            .iter()
            .any(|t| t.session_id == session_id && !t.progress.status.is_finished())
        {
            return Err(AppError::SessionBusy(session.name));
        }

        let now = self.clock.now();
        let resume = self.sessions.has_started(session_id)?;
        let job_id = self
            .jobs
            .insert(session_id, prompt_id, prompt, &session.model, now)?;
        self.jobs.set_status(job_id, JobStatus::Running, now)?;
        self.sessions.touch(session_id, now)?;
        self.tracked.push(Tracked {
            progress: JobProgress::new(job_id, &session.name, &session.model),
            session_id,
            started_at: now,
        });

        let tx = self.tx.clone();
        let sink: RunSink = Box::new(move |msg| {
            // The service may be gone at shutdown; nothing left to update then.
            let _ = tx.send((job_id, msg));
        });
        self.runner.start(
            RunRequest {
                cwd: session.cwd,
                session_id,
                resume,
                fork: false,
                model: session.model,
                permission_mode: session.permission_mode,
                prompt: prompt.to_string(),
            },
            sink,
        );
        Ok(job_id)
    }

    /// Applies every pending runner message. Returns errors to show the user.
    pub fn pump(&mut self) -> Vec<AppError> {
        let mut errors = Vec::new();
        while let Ok((job_id, msg)) = self.rx.try_recv() {
            if let Err(e) = self.apply(job_id, msg) {
                errors.push(e);
            }
        }
        errors
    }

    /// The most recently dispatched job.
    pub fn latest(&self) -> Option<&JobProgress> {
        self.tracked.last().map(|t| &t.progress)
    }

    /// Tenths of a second since `job` started (for the timeline's "now").
    pub fn tick_now(&self, job: JobId) -> u32 {
        self.tracked
            .iter()
            .find(|t| t.progress.job_id == job)
            .map_or(0, |t| self.tick(t.started_at))
    }

    fn tick(&self, started_at: Millis) -> u32 {
        ((self.clock.now() - started_at).max(0) / 100) as u32
    }

    fn apply(&mut self, job_id: JobId, msg: RunnerMsg) -> AppResult<()> {
        let now = self.clock.now();
        let Some(i) = self
            .tracked
            .iter()
            .position(|t| t.progress.job_id == job_id)
        else {
            return Ok(());
        };
        let tick = self.tick(self.tracked[i].started_at);
        let progress = &mut self.tracked[i].progress;

        let finished = match msg {
            RunnerMsg::Event(ev) => {
                if !matches!(ev, swarmux_domain::AgentEvent::Ignored { .. }) {
                    self.jobs.append_event(
                        job_id,
                        ev.kind(),
                        ev.actor().key(),
                        &describe(&ev),
                        now,
                    )?;
                }
                progress.apply(&ev, tick)
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
                progress.fail(&error, tick)
            }
            RunnerMsg::SpawnFailed(msg) => progress.fail(&format!("spawn failed: {msg}"), tick),
        };

        if let Some(result) = finished {
            self.jobs.save_result(job_id, &result)?;
            self.jobs.set_status(job_id, progress.status, now)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeRunner, FixedClock, InMemoryStore};
    use swarmux_domain::{Actor, AgentEvent, Session};

    struct Fixture {
        svc: JobService,
        store: Rc<InMemoryStore>,
        runner: Rc<FakeRunner>,
        clock: Rc<FixedClock>,
        session: Session,
    }

    fn fixture() -> Fixture {
        let store = Rc::new(InMemoryStore::default());
        let runner = Rc::new(FakeRunner::default());
        let clock = Rc::new(FixedClock::new(10_000));
        let session = Session::new(SessionId::random(), "demo", "/w".into(), "sonnet", 0).unwrap();
        crate::ports::SessionRepository::insert(store.as_ref(), &session).unwrap();
        let svc = JobService::new(store.clone(), store.clone(), runner.clone(), clock.clone());
        Fixture {
            svc,
            store,
            runner,
            clock,
            session,
        }
    }

    fn init() -> AgentEvent {
        AgentEvent::Init {
            session_id: "s".into(),
            model: "claude-sonnet".into(),
            cwd: "/w".into(),
            permission_mode: "default".into(),
        }
    }

    fn result(cost: f64) -> AgentEvent {
        AgentEvent::Result {
            is_error: false,
            subtype: "success".into(),
            cost_usd: cost,
            duration_ms: 900,
            num_turns: 1,
            tokens_in: 1,
            tokens_out: 2,
            cache_read: 0,
            context_window: Some(200_000),
            subagents_spawned: 0,
        }
    }

    #[test]
    fn first_dispatch_creates_the_claude_session() {
        let mut f = fixture();
        f.svc.dispatch(f.session.id, "  hello  ").unwrap();
        let req = f.runner.request(0);
        assert!(!req.resume);
        assert_eq!(req.prompt, "hello");
        assert_eq!(req.session_id, f.session.id);
        assert_eq!(req.cwd, f.session.cwd);
    }

    #[test]
    fn later_dispatch_resumes_once_session_started() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "one").unwrap();
        f.runner.emit(0, RunnerMsg::Event(init()));
        f.runner.emit(0, RunnerMsg::Event(result(0.1)));
        f.svc.pump();
        assert_eq!(f.store.status_of(job), JobStatus::Done);
        f.svc.dispatch(f.session.id, "two").unwrap();
        assert!(f.runner.request(1).resume);
    }

    #[test]
    fn failed_spawn_does_not_mark_session_started() {
        let mut f = fixture();
        f.svc.dispatch(f.session.id, "one").unwrap();
        f.runner.emit(0, RunnerMsg::SpawnFailed("not found".into()));
        f.svc.pump();
        f.svc.dispatch(f.session.id, "two").unwrap();
        assert!(!f.runner.request(1).resume);
    }

    #[test]
    fn busy_session_rejects_second_prompt() {
        let mut f = fixture();
        f.svc.dispatch(f.session.id, "one").unwrap();
        assert!(matches!(
            f.svc.dispatch(f.session.id, "two"),
            Err(AppError::SessionBusy(name)) if name == "demo"
        ));
    }

    #[test]
    fn empty_prompt_and_unknown_session_are_rejected() {
        let mut f = fixture();
        assert!(matches!(
            f.svc.dispatch(f.session.id, "   "),
            Err(AppError::Domain(DomainError::EmptyPrompt))
        ));
        assert!(matches!(
            f.svc.dispatch(SessionId::random(), "x"),
            Err(AppError::SessionNotFound)
        ));
    }

    #[test]
    fn result_is_persisted_with_progress() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "hi").unwrap();
        f.clock.advance(1_500);
        f.runner.emit(0, RunnerMsg::Event(result(0.5)));
        assert!(f.svc.pump().is_empty());
        let p = f.svc.latest().unwrap();
        assert_eq!(p.cost_usd, 0.5);
        assert_eq!(p.status, JobStatus::Done);
        assert_eq!(f.store.status_of(job), JobStatus::Done);
        assert_eq!(f.store.result_of(job).unwrap().cost_usd, 0.5);
        assert_eq!(f.svc.tick_now(job), 15);
    }

    #[test]
    fn exit_without_result_fails_with_stderr() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "hi").unwrap();
        f.runner.emit(
            0,
            RunnerMsg::Exited {
                code: Some(1),
                stderr_tail: "bad flag\n".into(),
            },
        );
        f.svc.pump();
        assert_eq!(f.store.status_of(job), JobStatus::Failed);
        assert_eq!(
            f.store.result_of(job).unwrap().error.as_deref(),
            Some("claude exited with code 1: bad flag")
        );
    }

    #[test]
    fn exit_after_result_keeps_done() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "hi").unwrap();
        f.runner.emit(0, RunnerMsg::Event(result(0.1)));
        f.runner.emit(
            0,
            RunnerMsg::Exited {
                code: Some(0),
                stderr_tail: String::new(),
            },
        );
        f.svc.pump();
        assert_eq!(f.store.status_of(job), JobStatus::Done);
    }

    #[test]
    fn events_are_stored_except_noise() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "hi").unwrap();
        f.runner.emit(
            0,
            RunnerMsg::Event(AgentEvent::Ignored {
                kind: "system/hook_started".into(),
            }),
        );
        f.runner.emit(
            0,
            RunnerMsg::Event(AgentEvent::Text {
                text: "hello".into(),
                actor: Actor::Subagent("toolu_A".into()),
            }),
        );
        f.svc.pump();
        assert_eq!(
            f.store.events_of(job),
            [(
                "text".to_string(),
                "toolu_A".to_string(),
                "hello".to_string()
            )]
        );
    }

    #[test]
    fn plain_dispatch_has_no_template() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "hi").unwrap();
        assert_eq!(f.store.prompt_of(job), None);
    }

    #[test]
    fn template_dispatch_records_the_prompt() {
        let mut f = fixture();
        let prompts = crate::PromptService::new(
            f.store.clone(),
            f.store.clone(),
            Rc::new(crate::testing::FakeWorkspace::default()),
            f.clock.clone(),
        );
        let id = prompts.create("t", "fix {{what}}", vec![]).unwrap();
        let values = [("what".to_string(), "bug".to_string())].into();
        let rendered = prompts.render(id, f.session.id, &values).unwrap();
        let job = f.svc.dispatch_template(f.session.id, &rendered).unwrap();
        assert_eq!(f.runner.request(0).prompt, "fix bug");
        assert_eq!(f.store.prompt_of(job), Some(id));
    }

    #[test]
    fn recover_interrupts_running_jobs() {
        let mut f = fixture();
        let job = f.svc.dispatch(f.session.id, "hi").unwrap();
        assert_eq!(f.svc.recover().unwrap(), 1);
        assert_eq!(f.store.status_of(job), JobStatus::Interrupted);
    }
}
