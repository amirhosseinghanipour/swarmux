use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::intervene::{Config, DEFAULT_MAX_CHARS, MAX_PENDING};
use swarmux_domain::ledger::{LedgerDraft, LedgerEntry, LedgerKind};
use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract, MissionState};
use swarmux_domain::{AgentEvent, GuardPolicy, JobId, JobStatus, Session, SessionId, Unmatched};

use super::*;
use crate::ledger::{Ledger, LedgerService, verify_all};
use crate::missions::MissionService;
use crate::ports::{JobRepository, NewJob, RunnerMsg, SessionRepository};
use crate::testing::{
    FakeDecisionRepo, FakeHookInbox, FakeJobGates, FakeRunner, FixedClock, InMemoryLedger,
    InMemoryMissions, InMemoryStore,
};
use crate::{AppResult, GuardService, JobService, SharedPolicy};

/// A ledger that can be taken down.
struct Flaky {
    inner: Rc<InMemoryLedger>,
    down: Cell<bool>,
}

impl Ledger for Flaky {
    fn append(&self, draft: LedgerDraft) -> AppResult<LedgerEntry> {
        if self.down.get() {
            return Err(AppError::Storage("disk full".into()));
        }
        self.inner.append(draft)
    }
    fn head(&self) -> AppResult<Option<LedgerEntry>> {
        self.inner.head()
    }
    fn range(&self, from_seq: u64, limit: usize) -> AppResult<Vec<LedgerEntry>> {
        self.inner.range(from_seq, limit)
    }
}

struct Fx {
    engine: Engine,
    db: Rc<InMemoryStore>,
    runner: Rc<FakeRunner>,
    inbox: Rc<FakeHookInbox>,
    ledger: Rc<InMemoryLedger>,
    flaky: Rc<Flaky>,
    ledger_service: Rc<LedgerService>,
    session: Session,
}

fn fixture() -> Fx {
    fixture_with(None)
}

fn fixture_with(limits: Option<swarmux_domain::Limits>) -> Fx {
    let db = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "search", "/repo".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*db, &session).unwrap();
    let runner = Rc::new(FakeRunner::default());
    let clock = Rc::new(FixedClock::new(10_000));
    let mut jobs = JobService::new(db.clone(), db.clone(), runner.clone(), clock.clone());
    if let Some(limits) = limits {
        jobs = jobs.with_limits(limits);
    }
    let inbox = Rc::new(FakeHookInbox::default());
    let policy = SharedPolicy::new(GuardPolicy::new(Vec::new(), Vec::new(), Unmatched::Ask));
    let guard = GuardService::new(
        policy,
        inbox.clone(),
        Rc::new(FakeDecisionRepo::default()),
        clock.clone(),
    )
    .with_timeout_secs(120);
    let ledger = Rc::new(InMemoryLedger::default());
    let flaky = Rc::new(Flaky {
        inner: ledger.clone(),
        down: Cell::new(false),
    });
    let ledger_service = Rc::new(LedgerService::new(flaky.clone(), clock.clone()));
    let gates = Rc::new(FakeJobGates::default());
    let missions = MissionService::new(Rc::new(InMemoryMissions::default()), clock.clone())
        .unwrap()
        .with_gates(gates)
        .with_ledger(ledger_service.clone());
    let mut engine = Engine::new(jobs, guard);
    engine.missions = Some(missions);
    engine.ledger = Some(ledger_service.clone());
    Fx {
        engine,
        db,
        runner,
        inbox,
        ledger,
        flaky,
        ledger_service,
        session,
    }
}

fn init() -> RunnerMsg {
    RunnerMsg::Event(AgentEvent::Init {
        session_id: "s".into(),
        model: "sonnet".into(),
        cwd: "/repo".into(),
        permission_mode: "default".into(),
    })
}

fn new_job(session: SessionId, prompt: &str, review_of: Option<JobId>) -> NewJob {
    NewJob {
        session_id: session,
        prompt_id: None,
        prompt: prompt.into(),
        model: "sonnet".into(),
        priority: 0,
        fork: false,
        wants_review: false,
        review_of,
    }
}

impl Fx {
    /// A running job whose agent has reported.
    fn running(&mut self) -> JobId {
        let job = self.engine.jobs.dispatch(self.session.id, "work").unwrap();
        self.engine.jobs.pump();
        self.runner.emit(self.runner.started() - 1, init());
        self.engine.jobs.pump();
        job
    }

    /// The `i`-th started job ends with `code`.
    fn finish(&mut self, i: usize, code: i32) {
        self.runner.emit(
            i,
            RunnerMsg::Event(AgentEvent::Result {
                is_error: code != 0,
                subtype: "success".into(),
                cost_usd: 0.1,
                duration_ms: 900,
                num_turns: 1,
                tokens_in: 1,
                tokens_out: 1,
                cache_read: 0,
                cache_creation: None,
                context_window: None,
                subagents_spawned: 0,
                text: "done".into(),
            }),
        );
        self.runner.emit(
            i,
            RunnerMsg::Exited {
                code: Some(code),
                stderr_tail: String::new(),
            },
        );
        self.engine.jobs.pump();
    }

    fn operator_entries(&self) -> Vec<String> {
        self.ledger
            .entries()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::Operator)
            .map(|e| e.draft.payload_json)
            .collect()
    }

    fn pump_missions(&mut self) {
        let facts = self.engine.jobs.facts();
        self.engine.pump_missions(&facts, &HashMap::new());
    }

    fn state_of(&self, id: u64) -> State {
        self.engine.interventions.get(id).unwrap().state.clone()
    }
}

#[track_caller]
fn refused(r: &Receipt) -> Refusal {
    match &r.state {
        State::Refused(why) => why.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn an_empty_or_oversized_message_is_refused_and_nothing_is_recorded() {
    let mut f = fixture();
    let job = f.running();
    for text in ["", "   \n\t "] {
        let r = f.engine.intervene(job, text, Mode::Queue, false);
        assert_eq!(refused(&r), Refusal::Empty);
    }
    let long = "x".repeat(DEFAULT_MAX_CHARS + 1);
    let r = f.engine.intervene(job, &long, Mode::Queue, false);
    assert!(matches!(refused(&r), Refusal::TooLong { .. }));
    assert!(f.operator_entries().is_empty());
    assert_eq!(f.engine.interventions.waiting(job), 0);
}

#[test]
fn the_size_cap_is_the_users_setting() {
    let mut f = fixture();
    f.engine.interventions.configure(Config {
        max_chars: 10,
        ..Config::default()
    });
    let job = f.running();
    assert!(matches!(
        refused(
            &f.engine
                .intervene(job, "this is too long", Mode::Queue, false)
        ),
        Refusal::TooLong { chars: 16, max: 10 }
    ));
    assert_eq!(
        f.engine.intervene(job, "short", Mode::Queue, false).state,
        State::Queued
    );
}

#[test]
fn only_a_running_job_this_run_follows_can_be_messaged() {
    let mut f = fixture_with(Some(swarmux_domain::Limits {
        max_concurrent: 1,
        min_free_ram: 0,
    }));
    let busy = f.running();
    let r = f.engine.intervene(JobId(999), "hello", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::NoSuchJob);

    // Queued: the runner has not started it.
    let queued = f.engine.jobs.dispatch(f.session.id, "later").unwrap();
    let r = f.engine.intervene(queued, "hello", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::NotRunning(JobStatus::Queued));
    f.finish(0, 0);
    let r = f.engine.intervene(busy, "hello", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::NotRunning(JobStatus::Done));

    // Running in the store, but not started by this run.
    let foreign = JobRepository::insert(&*f.db, &new_job(f.session.id, "old", None), 0).unwrap();
    f.db.set_status(foreign, JobStatus::Running, 0).unwrap();
    let r = f.engine.intervene(foreign, "hello", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::NotControlled);

    // A reviewer is swarmux's own.
    let reviewer =
        JobRepository::insert(&*f.db, &new_job(f.session.id, "audit", Some(queued)), 0).unwrap();
    f.db.set_status(reviewer, JobStatus::Running, 0).unwrap();
    let r = f.engine.intervene(reviewer, "hello", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::Reviewer);
    assert!(f.operator_entries().is_empty());
}

#[test]
fn a_stopping_swarmux_takes_nothing_new() {
    let mut f = fixture();
    let job = f.running();
    f.engine.jobs.set_draining(true);
    let r = f.engine.intervene(job, "hello", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::Stopping);
}

#[test]
fn at_most_five_messages_wait_on_a_job() {
    let mut f = fixture();
    let job = f.running();
    for i in 0..MAX_PENDING {
        let r = f
            .engine
            .intervene(job, &format!("note {i}"), Mode::Queue, false);
        assert_eq!(r.state, State::Queued, "{i}");
    }
    let r = f.engine.intervene(job, "one more", Mode::Queue, false);
    assert_eq!(refused(&r), Refusal::TooMany(MAX_PENDING));
}

#[test]
fn a_queued_message_waits_for_the_turn_then_the_session_continues_with_it() {
    let mut f = fixture();
    let job = f.running();
    let r = f
        .engine
        .intervene(job, "  use the staging bucket  ", Mode::Queue, false);
    assert_eq!(r.state, State::Queued);
    assert_eq!(r.text, "use the staging bucket");
    assert!(r.summary().contains("queued"));

    // While the turn runs, nothing is sent and nothing is stopped.
    for _ in 0..3 {
        assert!(f.engine.pump_interventions().is_empty());
    }
    assert_eq!(f.state_of(r.id), State::Queued);
    assert_eq!(f.runner.started(), 1);
    assert!(f.runner.cancelled().is_empty());

    // The turn ends: the next frame hands the message over.
    f.finish(0, 0);
    f.pump_missions();
    let State::Handed { job: new } = f.state_of(r.id) else {
        panic!("{:?}", f.state_of(r.id));
    };
    assert_ne!(new, job);
    f.engine.jobs.pump();
    assert_eq!(f.runner.started(), 2);
    let req = f.runner.request(1);
    assert!(
        req.prompt.contains("use the staging bucket"),
        "{}",
        req.prompt
    );
    assert!(req.prompt.contains("while you were working"));
    assert_eq!(req.session_id, f.session.id);

    // Handed over is not delivered: the agent has said nothing yet.
    f.pump_missions();
    assert!(matches!(f.state_of(r.id), State::Handed { .. }));
    assert!(
        f.engine
            .interventions
            .get(r.id)
            .unwrap()
            .summary()
            .contains("not confirmed")
    );

    // Its first report is the proof the turn started.
    f.runner.emit(1, init());
    f.engine.jobs.pump();
    f.pump_missions();
    assert_eq!(
        f.state_of(r.id),
        State::Delivered {
            job: new,
            at: 10_000
        }
    );
}

#[test]
fn the_ledger_records_the_message_first_and_how_it_ended() {
    let mut f = fixture();
    let job = f.running();
    let r = f
        .engine
        .intervene(job, "use the staging bucket", Mode::Queue, false);
    let entries = f.operator_entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    for part in [
        r#""action":"intervene""#.to_string(),
        r#""mode":"queue""#.to_string(),
        r#""note":"use the staging bucket""#.to_string(),
        format!(r#""job":{}"#, job.0),
        r#""outcome":"queued""#.to_string(),
        format!(r#""message":{}"#, r.id),
        r#""delivered_at":null"#.to_string(),
    ] {
        assert!(entries[0].contains(&part), "{part} in {}", entries[0]);
    }
    let entry = f
        .ledger
        .entries()
        .into_iter()
        .find(|e| e.draft.kind == LedgerKind::Operator)
        .unwrap();
    assert_eq!(entry.draft.actor, "operator");
    assert_eq!(entry.draft.job, Some(job));
    assert_eq!(Some(entry.seq), r.seq);

    f.finish(0, 0);
    f.pump_missions();
    f.engine.jobs.pump();
    f.runner.emit(1, init());
    f.engine.jobs.pump();
    f.pump_missions();
    let entries = f.operator_entries();
    assert_eq!(entries.len(), 2, "{entries:?}");
    let last = &entries[1];
    assert!(last.contains(r#""outcome":"delivered""#), "{last}");
    assert!(last.contains(r#""delivered_at":10000"#), "{last}");
    assert!(
        last.contains(&format!(r#""queued_seq":{}"#, r.seq.unwrap())),
        "{last}"
    );
    assert!(
        !last.contains(r#""note""#),
        "the text is in the first entry only"
    );
    assert!(verify_all(f.ledger_service.ledger()).unwrap().is_ok());
}

#[test]
fn a_long_message_is_cut_in_the_ledger_like_a_send_back_note() {
    let mut f = fixture();
    f.engine.interventions.configure(Config {
        max_chars: swarmux_domain::intervene::MAX_MAX_CHARS,
        ..Config::default()
    });
    let job = f.running();
    let long = "w".repeat(swarmux_domain::ledger::PAYLOAD_TEXT_MAX + 500);
    let r = f.engine.intervene(job, &long, Mode::Queue, false);
    assert_eq!(r.state, State::Queued);
    let e = &f.operator_entries()[0];
    assert!(e.contains("note_sha256"), "{e}");
    assert!(e.contains(&format!(r#""note_len":{}"#, long.len())));
    // The agent still gets the whole message.
    f.finish(0, 0);
    f.pump_missions();
    f.engine.jobs.pump();
    assert!(f.runner.request(1).prompt.contains(&long));
}

#[test]
fn a_message_that_cannot_be_recorded_is_not_taken_or_sent() {
    let mut f = fixture();
    let job = f.running();
    f.flaky.down.set(true);
    for mode in [Mode::Queue, Mode::Steer] {
        let r = f.engine.intervene(job, "do it", mode, true);
        let why = refused(&r);
        assert!(matches!(why, Refusal::Ledger(_)), "{why:?}");
        assert!(why.reason().contains("nothing was sent"));
    }
    assert_eq!(f.engine.interventions.waiting(job), 0);
    assert!(
        f.runner.cancelled().is_empty(),
        "a steer must not stop the turn"
    );
    // The job goes on, and a later message is taken once the ledger is back.
    f.flaky.down.set(false);
    assert_eq!(
        f.engine.intervene(job, "do it", Mode::Queue, false).state,
        State::Queued
    );
    f.finish(0, 0);
    f.pump_missions();
    f.engine.jobs.pump();
    assert_eq!(f.runner.started(), 2);
    assert!(verify_all(f.ledger_service.ledger()).unwrap().is_ok());
}

#[test]
fn messages_queued_together_go_as_one_continuation_in_order() {
    let mut f = fixture();
    let job = f.running();
    let a = f.engine.intervene(job, "first", Mode::Queue, false);
    let b = f.engine.intervene(job, "second", Mode::Queue, false);
    f.finish(0, 0);
    f.pump_missions();
    f.engine.jobs.pump();
    assert_eq!(f.runner.started(), 2, "one continuation, not two");
    let prompt = f.runner.request(1).prompt;
    assert!(prompt.contains("these messages"), "{prompt}");
    assert!(prompt.find("first").unwrap() < prompt.find("second").unwrap());
    let (State::Handed { job: x }, State::Handed { job: y }) = (f.state_of(a.id), f.state_of(b.id))
    else {
        panic!("both are handed over");
    };
    assert_eq!(x, y);
}

#[test]
fn killing_the_job_drops_a_queued_message_and_says_so() {
    let mut f = fixture();
    let job = f.running();
    let r = f.engine.intervene(job, "hello", Mode::Queue, false);
    f.engine.jobs.cancel(job).unwrap();
    f.pump_missions();
    let State::Undelivered(why) = f.state_of(r.id) else {
        panic!("{:?}", f.state_of(r.id));
    };
    assert!(why.contains("stopped before its turn ended"), "{why}");
    assert_eq!(f.runner.started(), 1, "nothing was sent");
    let last = f.operator_entries().pop().unwrap();
    assert!(last.contains(r#""outcome":"undelivered""#), "{last}");
    assert!(last.contains("stopped before its turn ended"));
}

#[test]
fn a_continuation_that_ends_before_the_agent_reported_is_not_called_delivered() {
    let mut f = fixture();
    let job = f.running();
    let r = f.engine.intervene(job, "hello", Mode::Queue, false);
    f.finish(0, 0);
    f.pump_missions();
    f.engine.jobs.pump();
    // The new process dies without a word.
    f.runner.emit(
        1,
        RunnerMsg::Exited {
            code: Some(1),
            stderr_tail: "boom".into(),
        },
    );
    f.engine.jobs.pump();
    f.pump_missions();
    let State::Undelivered(why) = f.state_of(r.id) else {
        panic!("{:?}", f.state_of(r.id));
    };
    assert!(why.contains("before the agent reported"), "{why}");
}

#[test]
fn steering_asks_for_confirmation_when_the_settings_say_so() {
    let mut f = fixture();
    let job = f.running();
    let r = f
        .engine
        .intervene(job, "stop and listen", Mode::Steer, false);
    assert_eq!(refused(&r), Refusal::ConfirmSteer);
    assert!(f.runner.cancelled().is_empty());
    assert!(f.operator_entries().is_empty());

    f.engine.interventions.configure(Config {
        confirm_steer: false,
        ..Config::default()
    });
    let r = f
        .engine
        .intervene(job, "stop and listen", Mode::Steer, false);
    assert!(matches!(r.state, State::Handed { .. }), "{:?}", r.state);
}

#[test]
fn steering_stops_the_turn_and_continues_the_session_at_once() {
    let mut f = fixture();
    let job = f.running();
    let r = f
        .engine
        .intervene(job, "wrong table: use orders_v2", Mode::Steer, true);
    // No wait for a frame: the turn was stopped and the session continued.
    assert_eq!(f.runner.cancelled(), vec![job]);
    let State::Handed { job: new } = r.state else {
        panic!("{:?}", r.state);
    };
    f.engine.jobs.pump();
    assert_eq!(f.runner.started(), 2);
    let req = f.runner.request(1);
    assert_eq!(req.session_id, f.session.id);
    assert!(req.prompt.contains("wrong table: use orders_v2"));
    assert!(req.prompt.contains("stopped your turn"));
    assert!(req.prompt.contains("may not have finished"));
    assert_eq!(
        f.engine.jobs.stored(job).unwrap().unwrap().status,
        JobStatus::Cancelled
    );
    f.runner.emit(1, init());
    f.engine.jobs.pump();
    f.pump_missions();
    assert!(matches!(f.state_of(r.id), State::Delivered { job: j, .. } if j == new));
    let entries = f.operator_entries();
    assert!(entries[0].contains(r#""mode":"steer""#));
    assert!(entries[0].contains(r#""outcome":"stopping""#));
    assert!(entries[1].contains(r#""outcome":"delivered""#));
    assert!(verify_all(f.ledger_service.ledger()).unwrap().is_ok());
}

#[test]
fn a_held_call_blocks_steering_but_not_queueing() {
    let mut f = fixture();
    let job = f.running();
    f.inbox.push(Some(job), "Bash", "rm -rf build");
    f.engine.guard.pump();
    assert_eq!(f.engine.guard.pending_count(), 1);

    let r = f.engine.intervene(job, "wait", Mode::Steer, true);
    assert_eq!(refused(&r), Refusal::ApprovalPending);
    assert!(f.runner.cancelled().is_empty());
    assert_eq!(f.engine.guard.pending_count(), 1, "the call is still held");
    assert!(f.operator_entries().is_empty());

    // Queueing leaves the call as it is: the message does not approve anything.
    let q = f.engine.intervene(job, "wait", Mode::Queue, false);
    assert_eq!(q.state, State::Queued);
    assert_eq!(f.engine.guard.pending_count(), 1);
    assert!(f.runner.cancelled().is_empty());
}

fn contract() -> Contract {
    Contract {
        title: "Fast search".into(),
        goal: "make search answer in under 50 ms on the bench corpus".into(),
        acceptance: Acceptance {
            checks: vec!["cargo test".into()],
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/search/**".into()],
            protected: vec!["tests/**".into()],
        },
        budget: Budget::default(),
        due: None,
        clarifications: Vec::new(),
        repos: Vec::new(),
    }
}

#[test]
fn a_mission_follows_the_message_once_it_has_taken_the_ended_job_in() {
    let mut f = fixture();
    let session = Some(f.session.id);
    let missions = f.engine.missions.as_mut().unwrap();
    let slug = missions.create(contract(), "/repo", session).unwrap();
    let job = missions.launch(&slug, &mut f.engine.jobs).unwrap();
    f.engine.jobs.pump();
    f.runner.emit(0, init());
    f.engine.jobs.pump();

    let r = f
        .engine
        .intervene(job, "the bench needs the warm cache", Mode::Queue, false);
    assert_eq!(r.state, State::Queued);
    assert!(f.operator_entries()[0].contains(&format!(r#""mission":"{slug}""#)));

    // The job fails: until the mission has taken that in, the message waits.
    f.finish(0, 1);
    assert_eq!(f.engine.pump_interventions().len(), 0);
    assert_eq!(f.state_of(r.id), State::Queued);
    f.pump_missions();
    assert!(
        matches!(f.state_of(r.id), State::Handed { .. }),
        "{:?}",
        f.state_of(r.id)
    );
    let m = f.engine.missions.as_ref().unwrap().get(&slug).unwrap();
    assert_ne!(m.job, Some(job), "the mission follows the new job");
    assert_eq!(m.state, MissionState::Queued);
}

#[test]
fn the_receipt_lists_are_per_job_and_survive_settling() {
    let mut f = fixture();
    let job = f.running();
    let a = f.engine.intervene(job, "one", Mode::Queue, false);
    assert_eq!(f.engine.interventions.for_job(job).count(), 1);
    assert_eq!(f.engine.interventions.for_job(JobId(77)).count(), 0);
    assert_eq!(f.engine.interventions.get(a.id).unwrap().text, "one");
    f.engine.jobs.cancel(job).unwrap();
    f.pump_missions();
    assert_eq!(f.engine.interventions.for_job(job).count(), 1);
    assert!(f.engine.interventions.get(a.id).unwrap().state.is_settled());
}

#[test]
fn the_receipt_view_tells_a_client_what_happened_without_guessing() {
    use crate::console::ReceiptView;
    let mut f = fixture();
    let job = f.running();
    let r = f.engine.intervene(job, "hello", Mode::Queue, false);
    let v = ReceiptView::from(&r);
    assert_eq!((v.state.as_str(), v.mode.as_str()), ("queued", "queue"));
    assert_eq!((v.resumed_as, v.delivered_at), (None, None));
    assert_eq!(v.seq, r.seq);

    f.finish(0, 0);
    f.pump_missions();
    let handed = ReceiptView::from(f.engine.interventions.get(r.id).unwrap());
    assert_eq!(handed.state, "handed_over");
    assert!(handed.resumed_as.is_some());
    assert_eq!(handed.delivered_at, None, "not delivered until it started");

    f.engine.jobs.pump();
    f.runner.emit(1, init());
    f.engine.jobs.pump();
    f.pump_missions();
    let done = ReceiptView::from(f.engine.interventions.get(r.id).unwrap());
    assert_eq!(done.state, "delivered");
    assert_eq!(done.delivered_at, Some(10_000));
    // It round-trips as JSON for a daemon API.
    let json = serde_json::to_string(&done).unwrap();
    assert_eq!(serde_json::from_str::<ReceiptView>(&json).unwrap(), done);

    let refused = ReceiptView::from(&f.engine.intervene(JobId(404), "x", Mode::Queue, false));
    assert_eq!((refused.state.as_str(), refused.id), ("refused", 0));
    assert!(refused.reason.unwrap().contains("no such job"));
}
