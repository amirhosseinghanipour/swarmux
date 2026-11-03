use super::*;
use crate::testing::{FakeMonitor, FakeRunner, FixedClock, InMemoryStore};
use swarmux_domain::{Actor, AgentEvent, Session};

const GB: u64 = 1 << 30;

struct Fixture {
    svc: JobService,
    store: Rc<InMemoryStore>,
    runner: Rc<FakeRunner>,
    clock: Rc<FixedClock>,
    monitor: Rc<FakeMonitor>,
    session: Session,
}

fn fixture() -> Fixture {
    let store = Rc::new(InMemoryStore::default());
    let runner = Rc::new(FakeRunner::default());
    let clock = Rc::new(FixedClock::new(10_000));
    let monitor = Rc::new(FakeMonitor::default());
    let session = add_session(&store, "demo");
    let svc = JobService::new(store.clone(), store.clone(), runner.clone(), clock.clone())
        .with_monitor(monitor.clone())
        .with_limits(Limits {
            max_concurrent: 2,
            min_free_ram: GB,
        });
    Fixture {
        svc,
        store,
        runner,
        clock,
        monitor,
        session,
    }
}

fn add_session(store: &InMemoryStore, name: &str) -> Session {
    let s = Session::new(SessionId::random(), name, "/w".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(store, &s).unwrap();
    s
}

fn init(session_id: &str) -> AgentEvent {
    AgentEvent::Init {
        session_id: session_id.into(),
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
        text: "done".into(),
    }
}

impl Fixture {
    /// Finishes the `i`-th started job successfully, as the session's first job.
    fn finish(&mut self, i: usize) {
        let id = self.runner.request(i).session_id.to_string();
        self.runner.emit(i, RunnerMsg::Event(init(&id)));
        self.runner.emit(i, RunnerMsg::Event(result(0.1)));
    }
}

#[test]
fn first_dispatch_creates_the_claude_session() {
    let mut f = fixture();
    let job = f.svc.dispatch(f.session.id, "  hello  ").unwrap();
    let req = f.runner.request(0);
    assert!(!req.resume);
    assert_eq!(req.job_id, job);
    assert_eq!(req.prompt, "hello");
    assert_eq!(req.session_id, f.session.id);
    assert_eq!(req.cwd, f.session.cwd);
    assert!(!req.lean);
    assert_eq!(f.store.status_of(job), JobStatus::Running);
}

#[test]
fn later_dispatch_resumes_once_session_started() {
    let mut f = fixture();
    let job = f.svc.dispatch(f.session.id, "one").unwrap();
    f.finish(0);
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
fn busy_session_queues_the_next_prompt() {
    let mut f = fixture();
    f.svc.dispatch(f.session.id, "one").unwrap();
    let second = f.svc.dispatch(f.session.id, "two").unwrap();
    assert_eq!(f.runner.started(), 1);
    assert_eq!(f.store.status_of(second), JobStatus::Queued);

    f.finish(0);
    let report = f.svc.pump();
    assert!(report.changed && report.errors.is_empty());
    assert_eq!(f.runner.started(), 2);
    assert_eq!(f.runner.request(1).prompt, "two");
    assert!(f.runner.request(1).resume);
    assert_eq!(f.store.status_of(second), JobStatus::Running);
}

#[test]
fn concurrency_cap_holds_jobs_back() {
    let mut f = fixture();
    let b = add_session(&f.store, "b");
    let c = add_session(&f.store, "c");
    f.svc.dispatch(f.session.id, "1").unwrap();
    f.svc.dispatch(b.id, "2").unwrap();
    let third = f.svc.dispatch(c.id, "3").unwrap();
    assert_eq!(f.runner.started(), 2);
    let stats = f.svc.stats();
    assert_eq!(
        (stats.running, stats.queued, stats.max_concurrent),
        (2, 1, 2)
    );

    f.finish(1);
    f.svc.pump();
    assert_eq!(f.runner.started(), 3);
    assert_eq!(f.store.status_of(third), JobStatus::Running);
}

#[test]
fn low_memory_waits_until_memory_frees_up() {
    let mut f = fixture();
    f.monitor.set(Some(GB / 2));
    let job = f.svc.dispatch(f.session.id, "hi").unwrap();
    assert_eq!(f.runner.started(), 0);
    let stats = f.svc.stats();
    assert!(stats.ram_low);
    assert_eq!(stats.free_ram, Some(GB / 2));

    f.monitor.set(Some(2 * GB));
    f.svc.pump();
    assert_eq!(f.runner.started(), 0, "reading is cached for a second");
    f.clock.advance(RAM_EVERY_MS);
    f.svc.pump();
    assert_eq!(f.runner.started(), 1);
    assert_eq!(f.store.status_of(job), JobStatus::Running);
    assert!(!f.svc.stats().ram_low);
}

#[test]
fn unknown_memory_does_not_block() {
    let mut f = fixture();
    f.monitor.set(None);
    f.svc.dispatch(f.session.id, "hi").unwrap();
    assert_eq!(f.runner.started(), 1);
    assert_eq!(f.svc.stats().free_ram, None);
}

#[test]
fn priority_decides_which_queued_job_runs_next() {
    let mut f = fixture();
    f.svc.dispatch(f.session.id, "running").unwrap();
    let low = f.svc.dispatch(f.session.id, "low").unwrap();
    let high = f.svc.dispatch(f.session.id, "high").unwrap();
    assert_eq!(f.svc.change_priority(high, 1).unwrap(), 1);
    assert_eq!(f.store.job_of(high).priority, 1);
    let order: Vec<JobId> = f.svc.queue().iter().map(|e| e.job_id).collect();
    assert_eq!(order[1..], [high, low]);

    f.finish(0);
    f.svc.pump();
    assert_eq!(f.runner.request(1).prompt, "high");
    assert!(matches!(
        f.svc.change_priority(high, 1),
        Err(AppError::JobNotQueued(id)) if id == high
    ));
}

#[test]
fn cancel_queued_job_never_starts_it() {
    let mut f = fixture();
    f.svc.dispatch(f.session.id, "one").unwrap();
    let queued = f.svc.dispatch(f.session.id, "two").unwrap();
    f.svc.cancel(queued).unwrap();
    assert_eq!(f.store.status_of(queued), JobStatus::Cancelled);
    assert!(f.runner.cancelled().is_empty());
    f.finish(0);
    f.svc.pump();
    assert_eq!(f.runner.started(), 1);
}

#[test]
fn cancel_running_job_stops_it_and_frees_the_session() {
    let mut f = fixture();
    let running = f.svc.dispatch(f.session.id, "one").unwrap();
    let next = f.svc.dispatch(f.session.id, "two").unwrap();
    f.clock.advance(500);
    f.svc.cancel(running).unwrap();
    assert_eq!(f.runner.cancelled(), [running]);
    assert_eq!(f.store.status_of(running), JobStatus::Cancelled);
    assert_eq!(f.store.result_of(running).unwrap().outcome, "cancelled");
    let p = f.svc.progress(running).unwrap();
    assert_eq!(p.status, JobStatus::Cancelled);
    assert!(p.log.back().unwrap().contains("cancelled"));

    // A late message from the killed process changes nothing.
    f.runner.emit(
        0,
        RunnerMsg::Exited {
            code: None,
            stderr_tail: String::new(),
        },
    );
    f.svc.pump();
    assert_eq!(f.store.status_of(running), JobStatus::Cancelled);
    assert_eq!(f.store.status_of(next), JobStatus::Running);
}

#[test]
fn cancel_finished_or_unknown_job_fails() {
    let mut f = fixture();
    let job = f.svc.dispatch(f.session.id, "one").unwrap();
    f.finish(0);
    f.svc.pump();
    assert!(matches!(f.svc.cancel(job), Err(AppError::JobFinished(_))));
    assert!(matches!(
        f.svc.cancel(JobId(999)),
        Err(AppError::JobNotFound)
    ));
}

#[test]
fn retry_queues_the_same_prompt_again() {
    let mut f = fixture();
    let prompts = crate::PromptService::new(
        f.store.clone(),
        f.store.clone(),
        Rc::new(crate::testing::FakeWorkspace::default()),
        f.clock.clone(),
    );
    let pid = prompts.create("t", "do it", vec![]).unwrap();
    let rendered = prompts
        .render(pid, f.session.id, &Default::default())
        .unwrap();
    let first = f.svc.dispatch_template(f.session.id, &rendered).unwrap();
    assert!(matches!(
        f.svc.retry(first),
        Err(AppError::JobNotFinished(_))
    ));
    f.runner.emit(0, RunnerMsg::SpawnFailed("x".into()));
    f.svc.pump();

    let again = f.svc.retry(first).unwrap();
    assert_ne!(again, first);
    let job = f.store.job_of(again);
    assert_eq!(
        (job.prompt.as_str(), job.session_id, job.prompt_id, job.fork),
        ("do it", f.session.id, Some(pid), false)
    );
    assert_eq!(f.runner.request(1).prompt, "do it");
}

#[test]
fn fork_creates_a_session_for_the_new_claude_session() {
    let mut f = fixture();
    let first = f.svc.dispatch(f.session.id, "base").unwrap();
    f.finish(0);
    f.svc.pump();

    let fork = f.svc.fork(first).unwrap();
    let req = f.runner.request(1);
    assert!(req.fork && req.resume);
    assert_eq!(req.session_id, f.session.id);

    let new_id = SessionId::random();
    f.runner
        .emit(1, RunnerMsg::Event(init(&new_id.to_string())));
    f.runner.emit(1, RunnerMsg::Event(result(0.2)));
    let report = f.svc.pump();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.new_sessions.len(), 1);
    let s = &report.new_sessions[0];
    assert_eq!((s.id, s.name.as_str()), (new_id, "demo fork"));
    assert_eq!(s.cwd, f.session.cwd);
    assert_eq!(f.store.job_of(fork).session_id, new_id);
    assert_eq!(f.svc.progress(fork).unwrap().session_name, "demo fork");

    // The fork is now a session of its own that later prompts continue.
    f.svc.dispatch(new_id, "next").unwrap();
    let req = f.runner.request(2);
    assert!(req.resume && !req.fork);
    assert_eq!(req.session_id, new_id);
}

#[test]
fn fork_reporting_the_same_session_creates_nothing() {
    let mut f = fixture();
    let first = f.svc.dispatch(f.session.id, "base").unwrap();
    f.runner.emit(0, RunnerMsg::SpawnFailed("x".into()));
    f.svc.pump();
    f.svc.fork(first).unwrap();
    f.finish(1);
    assert!(f.svc.pump().new_sessions.is_empty());
}

#[test]
fn recover_interrupts_running_and_resumes_queued() {
    let mut f = fixture();
    let running = f.svc.dispatch(f.session.id, "hi").unwrap();
    let queued = f.svc.dispatch(f.session.id, "later").unwrap();

    let runner = Rc::new(FakeRunner::default());
    let mut svc = JobService::new(
        f.store.clone(),
        f.store.clone(),
        runner.clone(),
        f.clock.clone(),
    );
    assert_eq!(svc.recover().unwrap(), 1);
    assert_eq!(f.store.status_of(running), JobStatus::Interrupted);
    assert_eq!(f.store.status_of(queued), JobStatus::Queued);
    assert_eq!(svc.stats().queued, 1);
    svc.pump();
    assert_eq!(runner.request(0).prompt, "later");
    assert_eq!(svc.progress(queued).unwrap().session_name, "demo");
}

#[test]
fn queue_lists_running_queued_then_finished() {
    let mut f = fixture();
    let done = f.svc.dispatch(f.session.id, "a").unwrap();
    f.finish(0);
    f.svc.pump();
    let running = f.svc.dispatch(f.session.id, "b").unwrap();
    let queued = f.svc.dispatch(f.session.id, "c").unwrap();
    let rows: Vec<(JobId, JobStatus)> =
        f.svc.queue().iter().map(|e| (e.job_id, e.status)).collect();
    assert_eq!(
        rows,
        [
            (running, JobStatus::Running),
            (queued, JobStatus::Queued),
            (done, JobStatus::Done)
        ]
    );
}

#[test]
fn old_finished_jobs_are_pruned() {
    let mut f = fixture();
    for i in 0..KEEP_FINISHED + 5 {
        let job = f.svc.dispatch(f.session.id, "x").unwrap();
        f.svc.cancel(job).unwrap();
        assert_eq!(f.runner.cancelled().len(), i + 1);
    }
    f.svc.pump();
    assert_eq!(f.svc.tracked.len(), KEEP_FINISHED);
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
    assert!(f.svc.pump().errors.is_empty());
    let p = f.svc.latest().unwrap();
    assert_eq!(p.cost_usd, 0.5);
    assert_eq!(p.status, JobStatus::Done);
    assert_eq!(f.store.status_of(job), JobStatus::Done);
    let r = f.store.result_of(job).unwrap();
    assert_eq!((r.cost_usd, r.text.as_str()), (0.5, "done"));
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

fn failing_call(f: &Fixture, i: usize, id: &str) {
    f.runner.emit(
        i,
        RunnerMsg::Event(AgentEvent::ToolUse {
            id: id.into(),
            name: "Bash".into(),
            summary: "cargo test".into(),
            actor: Actor::Lead,
        }),
    );
    f.runner.emit(
        i,
        RunnerMsg::Event(AgentEvent::ToolResult {
            tool_use_id: id.into(),
            is_error: true,
            preview: "error[E0425]: cannot find value".into(),
            actor: Actor::Lead,
        }),
    );
}

#[test]
fn loop_guard_stops_a_job_repeating_a_failure() {
    let mut f = fixture();
    let job = f.svc.dispatch(f.session.id, "fix it").unwrap();
    failing_call(&f, 0, "t1");
    assert!(f.svc.pump().loop_trips.is_empty());
    failing_call(&f, 0, "t2");
    let report = f.svc.pump();
    assert_eq!(report.loop_trips.len(), 1);
    assert_eq!(report.loop_trips[0].0, job);
    assert_eq!(f.runner.cancelled(), [job]);
    assert_eq!(f.store.status_of(job), JobStatus::Cancelled);
    let error = f.store.result_of(job).unwrap().error.unwrap();
    assert_eq!(error, "loop guard: Bash failed the same way twice");

    // Later messages from the killed process change nothing.
    f.runner.emit(0, RunnerMsg::Event(result(0.1)));
    f.svc.pump();
    assert_eq!(f.store.status_of(job), JobStatus::Cancelled);
}

#[test]
fn loop_guard_can_be_turned_off() {
    let mut f = fixture();
    f.svc = JobService::new(
        f.store.clone(),
        f.store.clone(),
        f.runner.clone(),
        f.clock.clone(),
    )
    .with_loop_repeats(0);
    let job = f.svc.dispatch(f.session.id, "fix it").unwrap();
    for id in ["a", "b", "c"] {
        failing_call(&f, 0, id);
    }
    assert!(f.svc.pump().loop_trips.is_empty());
    assert_eq!(f.store.status_of(job), JobStatus::Running);
}

#[test]
fn lean_sessions_run_lean_jobs() {
    let mut f = fixture();
    f.store.set_lean(f.session.id, true).unwrap();
    f.svc.dispatch(f.session.id, "hi").unwrap();
    assert!(f.runner.request(0).lean);
}

#[test]
fn free_memory_is_read_with_nothing_queued() {
    let mut f = fixture();
    f.monitor.set(Some(3 * GB));
    assert_eq!(f.svc.stats().free_ram, None);
    f.svc.pump();
    assert_eq!(f.svc.stats().free_ram, Some(3 * GB));
    assert_eq!(f.runner.started(), 0);
}

#[test]
fn session_state_is_tracked_while_running_and_dropped_when_done() {
    let mut f = fixture();
    let job = f.svc.dispatch(f.session.id, "a").unwrap();
    let state = |f: &Fixture| {
        f.svc
            .facts()
            .iter()
            .find(|x| x.job_id == job)
            .unwrap()
            .session_state
    };
    assert_eq!(state(&f), None);
    f.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::SessionState(SessionState::RequiresAction)),
    );
    f.svc.pump();
    assert_eq!(state(&f), Some(SessionState::RequiresAction));
    f.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::SessionState(SessionState::Running)),
    );
    f.svc.pump();
    assert_eq!(state(&f), Some(SessionState::Running));
    f.finish(0);
    f.svc.pump();
    assert_eq!(state(&f), None);
}
