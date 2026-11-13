use super::*;
use crate::testing::{FakeRunner, WorktreeRig};
use crate::{Notes, WorktreeSettings};
use swarmux_domain::{CheckpointReason, PortPlan};

struct Rig {
    wt: WorktreeRig,
    runner: Rc<FakeRunner>,
    svc: JobService,
}

fn service(wt: &WorktreeRig, runner: &Rc<FakeRunner>) -> JobService {
    JobService::new(
        wt.store.clone(),
        wt.store.clone(),
        runner.clone(),
        wt.clock.clone(),
    )
    .with_limits(Limits {
        max_concurrent: 8,
        min_free_ram: 0,
    })
    .with_worktrees(wt.worktrees.clone())
}

fn rig_with(settings: &WorktreeSettings) -> Rig {
    let wt = WorktreeRig::with_settings(settings);
    let runner = Rc::new(FakeRunner::default());
    let svc = service(&wt, &runner);
    Rig { wt, runner, svc }
}

fn rig() -> Rig {
    rig_with(&WorktreeSettings::default())
}

fn env_of(req: &RunRequest, key: &str) -> Option<String> {
    req.env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn init(id: &str) -> AgentEvent {
    AgentEvent::Init {
        session_id: id.into(),
        model: "claude-sonnet".into(),
        cwd: "/w".into(),
        permission_mode: "default".into(),
    }
}

fn done() -> AgentEvent {
    AgentEvent::Result {
        is_error: false,
        subtype: "success".into(),
        cost_usd: 0.1,
        duration_ms: 900,
        num_turns: 1,
        tokens_in: 1,
        tokens_out: 2,
        cache_read: 0,
        cache_creation: None,
        context_window: None,
        subagents_spawned: 0,
        text: "done".into(),
    }
}

#[test]
fn a_job_runs_in_its_own_worktree_with_its_port_block() {
    let mut r = rig();
    let job = r.svc.dispatch(r.wt.session.id, "build it").unwrap();
    let req = r.runner.request(0);
    let record = r.wt.worktrees.record_for(job).unwrap().expect("worktree");
    assert_eq!(req.cwd, record.path);
    assert_ne!(req.cwd, r.wt.session.cwd);
    assert_eq!(env_of(&req, "SWARMUX_PORT_BASE").as_deref(), Some("20000"));
    assert_eq!(
        env_of(&req, "SWARMUX_BRANCH").as_deref(),
        Some(record.branch.as_str())
    );
}

#[test]
fn eight_jobs_run_at_once_on_eight_port_blocks() {
    let mut r = rig();
    // One session runs one job at a time; eight sessions run eight.
    let mut sessions = vec![r.wt.session.clone()];
    for n in 1..8 {
        let mut s = r.wt.session.clone();
        s.id = SessionId::random();
        s.name = format!("app {n}");
        crate::ports::SessionRepository::insert(&*r.wt.store, &s).unwrap();
        sessions.push(s);
    }
    for s in &sessions {
        r.svc.dispatch(s.id, "work").unwrap();
    }
    assert_eq!(r.runner.started(), 8);
    let mut bases: Vec<u16> = (0..8)
        .map(|i| {
            env_of(&r.runner.request(i), "SWARMUX_PORT_BASE")
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    let mut cwds: Vec<_> = (0..8).map(|i| r.runner.request(i).cwd).collect();
    bases.sort_unstable();
    bases.dedup();
    cwds.sort();
    cwds.dedup();
    assert_eq!(bases.len(), 8);
    assert_eq!(cwds.len(), 8);
}

#[test]
fn a_session_without_worktrees_runs_where_it_always_did() {
    let mut r = rig();
    r.wt.store
        .set_worktree(r.wt.session.id, Some(false))
        .unwrap();
    r.svc.dispatch(r.wt.session.id, "x").unwrap();
    let req = r.runner.request(0);
    assert_eq!(req.cwd, r.wt.session.cwd);
    // Only what the Guard's hook is told: this job's owner is watching.
    assert_eq!(
        req.env,
        [("SWARMUX_GUARD_FAIL".to_string(), "open".to_string())]
    );
}

#[test]
fn a_worktree_that_cannot_be_made_fails_the_job_and_runs_nothing() {
    let mut r = rig_with(&WorktreeSettings {
        ports: PortPlan {
            base: 30_000,
            block: 10,
            slots: 1,
        },
        ..WorktreeSettings::default()
    });
    r.svc.dispatch(r.wt.session.id, "first").unwrap();
    // The only block is taken; the same session has to wait for its turn, so
    // use a second session.
    let mut other = r.wt.session.clone();
    other.id = SessionId::random();
    other.name = "other".into();
    crate::ports::SessionRepository::insert(&*r.wt.store, &other).unwrap();
    let job = r.svc.dispatch(other.id, "second").unwrap();
    assert_eq!(r.runner.started(), 1);
    assert_eq!(r.wt.store.status_of(job), JobStatus::Failed);
    let error = r.wt.store.result_of(job).unwrap().error.unwrap();
    assert!(error.starts_with("worktree:"), "{error}");
}

/// Starts a job, lets it report `init`, then "quits": a second service over
/// the same stores recovers.
fn quit_and_restart(r: &mut Rig) -> (JobId, JobService, Rc<FakeRunner>) {
    let job = r.svc.dispatch(r.wt.session.id, "long task").unwrap();
    let id = r.runner.request(0).session_id.to_string();
    r.runner.emit(0, RunnerMsg::Event(init(&id)));
    r.svc.pump();
    let runner = Rc::new(FakeRunner::default());
    let mut next = service(&r.wt, &runner);
    assert_eq!(next.recover().unwrap(), 1);
    (job, next, runner)
}

#[test]
fn resuming_after_a_quit_continues_in_the_same_worktree_and_session() {
    let mut r = rig();
    let (job, mut next, runner) = quit_and_restart(&mut r);
    let before = r.wt.worktrees.record_for(job).unwrap().unwrap();
    let resumed = next.resume(job).unwrap();
    let req = runner.request(0);
    assert!(req.resume, "continues the Claude session");
    assert_eq!(req.session_id, r.wt.session.id);
    assert_eq!(req.cwd, before.path);
    assert_eq!(
        env_of(&req, "SWARMUX_PORT_BASE").unwrap(),
        before.port_base.to_string()
    );
    assert_eq!(req.prompt, RESUME_PROMPT);
    // The worktree now belongs to the new job.
    assert!(r.wt.worktrees.record_for(job).unwrap().is_none());
    assert_eq!(
        r.wt.worktrees.record_for(resumed).unwrap().unwrap().path,
        before.path
    );
}

#[test]
fn retrying_an_interrupted_job_keeps_its_worktree_but_a_done_one_gets_a_new_one() {
    let mut r = rig();
    let (job, mut next, runner) = quit_and_restart(&mut r);
    let before = r.wt.worktrees.record_for(job).unwrap().unwrap();
    next.retry(job).unwrap();
    assert_eq!(runner.request(0).cwd, before.path);

    // Finish it; retrying a done job starts from a clean worktree.
    let id = runner.request(0).session_id.to_string();
    runner.emit(0, RunnerMsg::Event(init(&id)));
    runner.emit(0, RunnerMsg::Event(done()));
    next.pump();
    let finished = runner.request(0).job_id;
    next.retry(finished).unwrap();
    assert_ne!(runner.request(1).cwd, before.path);
}

#[test]
fn only_jobs_that_stopped_early_can_be_resumed() {
    let mut r = rig();
    let job = r.svc.dispatch(r.wt.session.id, "x").unwrap();
    assert!(matches!(r.svc.resume(job), Err(AppError::NotResumable(_))));
    let id = r.runner.request(0).session_id.to_string();
    r.runner.emit(0, RunnerMsg::Event(init(&id)));
    r.runner.emit(0, RunnerMsg::Event(done()));
    r.svc.pump();
    assert!(matches!(r.svc.resume(job), Err(AppError::NotResumable(_))));
}

#[test]
fn the_reviewer_reads_the_worktree_of_the_job_it_audits() {
    let mut r = rig();
    let job = r.svc.dispatch(r.wt.session.id, "x").unwrap();
    let wt = r.wt.worktrees.record_for(job).unwrap().unwrap();
    r.svc.set_review_next(false);
    let review = r
        .svc
        .jobs
        .insert(
            &NewJob {
                session_id: r.wt.session.id,
                prompt_id: None,
                prompt: "review".into(),
                model: "opus".into(),
                priority: 0,
                fork: false,
                wants_review: false,
                review_of: Some(job),
            },
            0,
        )
        .unwrap();
    let i = r.svc.tracked.len();
    r.svc.track(
        Job {
            id: review,
            session_id: r.wt.session.id,
            prompt_id: None,
            prompt: "review".into(),
            model: "opus".into(),
            priority: 0,
            fork: false,
            status: JobStatus::Queued,
            created_at: 0,
            wants_review: false,
            review_of: Some(job),
        },
        "demo",
    );
    assert_eq!(i, 1);
    // The audited job is running, so the same session cannot start the
    // review yet; finish it first.
    let id = r.runner.request(0).session_id.to_string();
    r.runner.emit(0, RunnerMsg::Event(init(&id)));
    r.runner.emit(0, RunnerMsg::Event(done()));
    r.svc.pump();
    assert_eq!(r.runner.started(), 2);
    let req = r.runner.request(1);
    assert!(req.review);
    assert_eq!(req.cwd, wt.path);
    // Only what the Guard's hook is told: nobody watches a reviewer.
    assert_eq!(
        req.env,
        [("SWARMUX_GUARD_FAIL".to_string(), "closed".to_string())]
    );
}

#[test]
fn stale_worktrees_are_reclaimed_when_the_app_pumps() {
    let mut r = rig();
    let job = r.svc.dispatch(r.wt.session.id, "x").unwrap();
    let id = r.runner.request(0).session_id.to_string();
    r.runner.emit(0, RunnerMsg::Event(init(&id)));
    r.runner.emit(0, RunnerMsg::Event(done()));
    r.svc.pump();
    assert!(r.wt.worktrees.record_for(job).unwrap().is_some());
    r.wt.clock.advance(13 * 3_600_000);
    r.svc.pump();
    assert!(r.wt.worktrees.record_for(job).unwrap().is_none());
    assert_eq!(r.wt.git.removed.borrow().len(), 1);
}

#[test]
fn checkpoints_survive_a_quit_and_follow_the_resumed_job() {
    let mut r = rig();
    let (job, mut next, _runner) = quit_and_restart(&mut r);
    r.wt.checkpoints
        .take(
            job,
            "claude-1",
            CheckpointReason::GatePass,
            &Notes::default(),
        )
        .unwrap()
        .unwrap();
    let resumed = next.resume(job).unwrap();
    assert_eq!(r.wt.checkpoints.list(resumed).unwrap().len(), 1);
}
