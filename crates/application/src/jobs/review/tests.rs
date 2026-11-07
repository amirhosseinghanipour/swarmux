use std::rc::Rc;

use swarmux_domain::{
    Actor, AgentEvent, JobId, JobStatus, Phase, ReviewState, ReviewVerdict, Session, SessionId,
};

use crate::JobService;
use crate::ReviewSettings;
use crate::ports::{JobRepository, RunnerMsg, SessionRepository};
use crate::testing::{FakeAgentCatalog, FakeRunner, FakeWorkspace, FixedClock, InMemoryStore};

struct Fixture {
    svc: JobService,
    store: Rc<InMemoryStore>,
    runner: Rc<FakeRunner>,
    clock: Rc<FixedClock>,
    workspace: Rc<FakeWorkspace>,
    catalog: Rc<FakeAgentCatalog>,
    session: Session,
}

fn fixture(review: bool) -> Fixture {
    let store = Rc::new(InMemoryStore::default());
    let runner = Rc::new(FakeRunner::default());
    let clock = Rc::new(FixedClock::new(10_000));
    let workspace = Rc::new(FakeWorkspace::with_diff("diff --git a/x b/x\n+fixed"));
    let catalog = Rc::new(FakeAgentCatalog::default());
    let session = Session::new(SessionId::random(), "demo", "/w".into(), "opus", 0).unwrap();
    SessionRepository::insert(store.as_ref(), &session).unwrap();
    let settings = ReviewSettings {
        enabled: review,
        model: "haiku".into(),
        when: crate::ReviewWhen::Always,
    };
    let svc = JobService::new(store.clone(), store.clone(), runner.clone(), clock.clone())
        .with_review(workspace.clone(), &settings)
        .with_agent_catalog(catalog.clone());
    Fixture {
        svc,
        store,
        runner,
        clock,
        workspace,
        catalog,
        session,
    }
}

fn result(text: &str, cost: f64, is_error: bool) -> RunnerMsg {
    RunnerMsg::Event(AgentEvent::Result {
        is_error,
        subtype: "success".into(),
        cost_usd: cost,
        duration_ms: 900,
        num_turns: 1,
        tokens_in: 1,
        tokens_out: 2,
        cache_read: 0,
        cache_creation: None,
        context_window: None,
        subagents_spawned: 0,
        text: text.into(),
    })
}

impl Fixture {
    /// Dispatches a prompt and finishes it successfully. Returns its id.
    fn run_job(&mut self) -> JobId {
        let job = self.svc.dispatch(self.session.id, "fix the bug").unwrap();
        self.clock.advance(5_000);
        self.runner.emit(0, result("fixed it", 0.3, false));
        self.svc.pump();
        job
    }
}

#[test]
fn a_successful_job_is_reviewed_and_ships_on_pass() {
    let mut f = fixture(true);
    let job = f.run_job();
    assert_eq!(f.runner.started(), 2, "the review started");
    let req = f.runner.request(1);
    assert!(req.review && !f.runner.request(0).review);
    assert_eq!(req.model, "haiku");
    assert_eq!(req.cwd, f.session.cwd);
    assert!(
        req.prompt
            .starts_with(&format!("swarmux review of job #{job}"))
    );
    assert!(req.prompt.contains("fix the bug") && req.prompt.contains("fixed it"));
    assert!(req.prompt.contains("+fixed"));
    assert_eq!(f.workspace.calls(), [f.session.cwd.clone()]);

    let review_job = req.job_id;
    assert_eq!(
        f.store.review(job).unwrap(),
        Some(ReviewState::Running { job: review_job })
    );
    let p = f.svc.progress(job).unwrap();
    assert_eq!(p.phase(), Phase::Audit);
    assert_eq!(
        f.svc.latest().unwrap().job_id,
        job,
        "reviews aren't the latest job"
    );
    assert_eq!(f.svc.subject_of(review_job), job);
    let run = f.svc.review_progress(job).unwrap();
    assert_eq!((run.lead_label.as_str(), run.log_offset), ("reviewer", 50));
    assert!(
        run.command
            .as_deref()
            .unwrap()
            .contains("--tools Read,Grep,Glob")
    );

    f.clock.advance(3_000);
    f.runner.emit(
        1,
        result(
            r#"{"verdict":"pass","concerns":[],"diffs_checked":1}"#,
            0.02,
            false,
        ),
    );
    f.svc.pump();
    let filed = f.store.review(job).unwrap().unwrap();
    let ReviewState::Done {
        review,
        job: rj,
        cost_usd,
    } = &filed
    else {
        panic!("{filed:?}");
    };
    assert_eq!(
        (review.verdict, review.diffs_checked),
        (ReviewVerdict::Pass, 1)
    );
    assert_eq!((*rj, *cost_usd), (review_job, 0.02));
    let p = f.svc.progress(job).unwrap();
    assert_eq!(p.phase(), Phase::Ship);
    assert_eq!(p.phase_ticks[Phase::Ship.index()], Some(80));
    assert_eq!(f.svc.timeline_end(job), 80);
    assert_eq!(f.store.status_of(job), JobStatus::Done);
}

#[test]
fn an_invalid_verdict_fails_the_review_not_the_job() {
    let mut f = fixture(true);
    let job = f.run_job();
    f.runner.emit(1, result("looks fine to me", 0.01, false));
    f.svc.pump();
    let Some(ReviewState::Failed {
        error, cost_usd, ..
    }) = f.store.review(job).unwrap()
    else {
        panic!("not failed");
    };
    assert!(error.contains("no JSON"), "{error}");
    assert_eq!(cost_usd, 0.01);
    assert_eq!(f.store.status_of(job), JobStatus::Done);
    assert_eq!(f.store.result_of(job).unwrap().text, "fixed it");
    assert_eq!(f.svc.progress(job).unwrap().phase(), Phase::Ship);
}

#[test]
fn a_review_that_cannot_run_is_filed_as_failed() {
    let mut f = fixture(true);
    let job = f.run_job();
    f.runner
        .emit(1, RunnerMsg::SpawnFailed("claude not found".into()));
    f.svc.pump();
    let state = f.store.review(job).unwrap().unwrap();
    assert!(
        matches!(&state, ReviewState::Failed { error, .. } if error.contains("claude not found")),
        "{state:?}"
    );
}

#[test]
fn failed_jobs_and_unrequested_jobs_get_no_review() {
    let mut f = fixture(true);
    let job = f.svc.dispatch(f.session.id, "x").unwrap();
    f.runner.emit(0, result("boom", 0.1, true));
    f.svc.pump();
    assert_eq!(f.runner.started(), 1);
    assert_eq!(f.svc.progress(job).unwrap().review, None);
    assert_eq!(f.store.review(job).unwrap(), None);

    assert!(!f.svc.set_review_next(false));
    let quiet = f.svc.dispatch(f.session.id, "y").unwrap();
    f.runner.emit(1, result("ok", 0.1, false));
    f.svc.pump();
    assert_eq!(f.runner.started(), 2);
    assert_eq!(f.svc.progress(quiet).unwrap().phase(), Phase::Ship);
}

#[test]
fn retry_keeps_the_review_request() {
    let mut f = fixture(true);
    let job = f.run_job();
    f.svc.set_review_next(false);
    let again = f.svc.retry(job).unwrap();
    assert!(f.store.job_of(again).wants_review);
}

#[test]
fn without_a_workspace_reviews_stay_off() {
    let store = Rc::new(InMemoryStore::default());
    let mut svc = JobService::new(
        store.clone(),
        store,
        Rc::new(FakeRunner::default()),
        Rc::new(FixedClock::new(0)),
    );
    assert!(!svc.set_review_next(true));
    assert_eq!(svc.review_model(), "opus");
}

#[test]
fn subagent_models_come_from_the_catalog() {
    let mut f = fixture(false);
    f.catalog.define("worker", "sonnet");
    f.catalog.define("twin", "inherit");
    let job = f.svc.dispatch(f.session.id, "go").unwrap();
    for (id, kind) in [("A", "worker"), ("B", "twin"), ("C", "Explore")] {
        f.runner.emit(
            0,
            RunnerMsg::Event(AgentEvent::ToolUse {
                id: id.into(),
                name: "Task".into(),
                summary: format!("{kind}: do it"),
                actor: Actor::Lead,
            }),
        );
    }
    f.svc.pump();
    let p = f.svc.progress(job).unwrap();
    let models: Vec<Option<&str>> = p.subagents.iter().map(|s| s.model.as_deref()).collect();
    assert_eq!(models, [Some("sonnet"), Some("opus"), None]);
    assert_eq!(f.catalog.asked()[0].0, f.session.cwd);
    assert_eq!(p.phase(), Phase::Parallel);
    assert!(
        p.command
            .as_deref()
            .unwrap()
            .starts_with("claude -p --model opus")
    );
}

#[test]
fn decisions_move_a_job_to_fork() {
    let mut f = fixture(false);
    let job = f.svc.dispatch(f.session.id, "go").unwrap();
    f.clock.advance(1_200);
    f.svc.observe_decision(job);
    f.svc.observe_decision(JobId(99));
    let p = f.svc.progress(job).unwrap();
    assert_eq!(
        (p.phase(), p.decision_marks.as_slice()),
        (Phase::Fork, &[12][..])
    );
}

#[test]
fn a_second_result_does_not_start_a_second_review() {
    let mut f = fixture(true);
    let job = f.run_job();
    // Claude Code reports again once a background subagent ends.
    f.runner
        .emit(0, result("fixed it, scout done", 0.35, false));
    f.svc.pump();
    assert_eq!(f.runner.started(), 2, "one review only");
    assert_eq!(f.store.result_of(job).unwrap().cost_usd, 0.35);
    let review_job = f.runner.request(1).job_id;
    assert_eq!(f.svc.review_progress(job).unwrap().job_id, review_job);
}

fn risk_fixture(diff: &str) -> Fixture {
    let mut f = fixture(true);
    let settings = ReviewSettings {
        enabled: true,
        model: "haiku".into(),
        when: crate::ReviewWhen::Risk,
    };
    f.workspace = Rc::new(FakeWorkspace::with_diff(diff));
    f.svc = JobService::new(
        f.store.clone(),
        f.store.clone(),
        f.runner.clone(),
        f.clock.clone(),
    )
    .with_review(f.workspace.clone(), &settings);
    f
}

#[test]
fn a_routine_change_is_not_worth_a_reviewer() {
    let mut f = risk_fixture(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-    41\n+    42\n",
    );
    let job = f.run_job();
    assert_eq!(f.runner.started(), 1, "no reviewer ran");
    let Some(ReviewState::Skipped { why }) = f.store.review(job).unwrap() else {
        panic!("the review should be skipped");
    };
    assert!(why.starts_with("risk routine"), "{why}");
}

#[test]
fn a_risky_change_gets_the_reviewer() {
    let mut f = risk_fixture(
        "diff --git a/Cargo.toml b/Cargo.toml\n--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1 +1,2 @@\n [dependencies]\n+left-pad = \"9\"\n",
    );
    let job = f.run_job();
    assert_eq!(f.runner.started(), 2, "the reviewer ran");
    assert!(matches!(
        f.store.review(job).unwrap(),
        Some(ReviewState::Running { .. })
    ));
}
