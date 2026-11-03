use super::*;
use crate::JobService;
use crate::ports::{RunnerMsg, SessionRepository};
use crate::testing::{FakeDecisionRepo, FakeRunner, FixedClock, InMemoryLedger, InMemoryStore};
use crate::{Outcome, Source};
use swarmux_domain::ledger::BreakReason;
use swarmux_domain::{Actor, AgentEvent, Review, ReviewVerdict, Session};

struct Fixture {
    store: Rc<InMemoryStore>,
    ledger: Rc<InMemoryLedger>,
    service: Rc<LedgerService>,
    jobs: Rc<LedgeredJobs>,
    clock: Rc<FixedClock>,
    session: Session,
}

fn fixture() -> Fixture {
    let store = Rc::new(InMemoryStore::default());
    let ledger = Rc::new(InMemoryLedger::default());
    let clock = Rc::new(FixedClock::new(1_000));
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let jobs = Rc::new(LedgeredJobs::new(store.clone(), service.clone()));
    let session = Session::new(SessionId::random(), "demo", "/w".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*store, &session).unwrap();
    Fixture {
        store,
        ledger,
        service,
        jobs,
        clock,
        session,
    }
}

fn kinds(ledger: &InMemoryLedger) -> Vec<LedgerKind> {
    ledger.entries().iter().map(|e| e.draft.kind).collect()
}

fn text(actor: Actor, text: &str) -> AgentEvent {
    AgentEvent::Text {
        actor,
        text: text.into(),
    }
}

fn result_event() -> AgentEvent {
    AgentEvent::Result {
        is_error: false,
        subtype: "success".into(),
        cost_usd: 0.25,
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

#[test]
fn a_job_run_is_mirrored_into_the_ledger() {
    let f = fixture();
    let runner = Rc::new(FakeRunner::default());
    let mut svc = JobService::new(
        f.store.clone(),
        f.jobs.clone(),
        runner.clone(),
        f.clock.clone(),
    );
    let job = svc.dispatch(f.session.id, "fix the bug").unwrap();
    let sid = f.session.id.to_string();
    runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::Init {
            session_id: sid.clone(),
            model: "claude-sonnet".into(),
            cwd: "/w".into(),
            permission_mode: "default".into(),
        }),
    );
    runner.emit(0, RunnerMsg::Event(text(Actor::Lead, "working")));
    runner.emit(
        0,
        RunnerMsg::Event(text(Actor::Subagent("toolu_1".into()), "helping")),
    );
    runner.emit(0, RunnerMsg::Event(result_event()));
    let report = svc.pump();
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    assert_eq!(
        kinds(&f.ledger),
        [
            LedgerKind::Genesis,
            LedgerKind::JobQueued,
            LedgerKind::JobStarted,
            LedgerKind::AgentEvent,
            LedgerKind::AgentEvent,
            LedgerKind::AgentEvent,
            LedgerKind::AgentEvent,
            LedgerKind::JobFinished,
        ]
    );
    let entries = f.ledger.entries();
    // Every stored job event has exactly one ledger entry, in order.
    let stored = f.store.events_of(job);
    let mirrored: Vec<(String, String)> = entries
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::AgentEvent)
        .map(|e| {
            let p = event_of(&e.draft.payload_json);
            (p, e.draft.actor.clone())
        })
        .collect();
    let expected: Vec<(String, String)> = stored
        .iter()
        .map(|(kind, actor, _)| (kind.clone(), actor.clone()))
        .collect();
    assert_eq!(mirrored, expected);
    assert!(
        entries[1..]
            .iter()
            .all(|e| e.draft.job == Some(job) && e.draft.session.as_deref() == Some(&*sid))
    );
    let finished = &entries.last().unwrap().draft.payload_json;
    assert!(finished.contains(r#""status":"done""#), "{finished}");
    assert!(finished.contains(r#""cost_usd":0.25"#), "{finished}");
    assert!(verify_all(&*f.ledger).unwrap().is_ok());
}

/// The `event` field of an agent-event payload.
fn event_of(payload: &str) -> String {
    let start = payload.find(r#""event":""#).unwrap() + 9;
    let end = payload[start..].find('"').unwrap();
    payload[start..start + end].to_string()
}

#[test]
fn cancel_and_late_results_are_recorded() {
    let f = fixture();
    let runner = Rc::new(FakeRunner::default());
    let mut svc = JobService::new(f.store.clone(), f.jobs.clone(), runner, f.clock.clone());
    let job = svc.dispatch(f.session.id, "x").unwrap();
    svc.cancel(job).unwrap();
    let last = f.ledger.entries().last().unwrap().clone();
    assert_eq!(last.draft.kind, LedgerKind::JobFinished);
    assert!(last.draft.payload_json.contains(r#""status":"cancelled""#));
    assert!(!last.draft.payload_json.contains("revised"));

    let r = f.store.result_of(job).unwrap();
    f.jobs.save_result(job, &r).unwrap();
    let revised = f
        .ledger
        .entries()
        .last()
        .unwrap()
        .draft
        .payload_json
        .clone();
    assert!(revised.contains(r#""revised":true"#), "{revised}");
}

#[test]
fn interrupted_jobs_are_recorded_once() {
    let f = fixture();
    let job = f
        .jobs
        .insert(
            &NewJob {
                session_id: f.session.id,
                prompt_id: None,
                prompt: "p".into(),
                model: "sonnet".into(),
                priority: 0,
                fork: false,
                wants_review: false,
                review_of: None,
            },
            5,
        )
        .unwrap();
    f.jobs.set_status(job, JobStatus::Running, 6).unwrap();
    assert_eq!(f.jobs.interrupt_running(7).unwrap(), 1);
    assert_eq!(f.jobs.interrupt_running(8).unwrap(), 0);
    let last = f.ledger.entries().last().unwrap().clone();
    assert_eq!(last.draft.kind, LedgerKind::JobFinished);
    assert_eq!(
        last.draft.payload_json,
        r#"{"status":"interrupted","count":1}"#
    );
    // Genesis, queued, started, interrupted.
    assert_eq!(f.ledger.entries().len(), 4);
}

#[test]
fn reviews_are_recorded_with_their_verdict() {
    let f = fixture();
    let runner = Rc::new(FakeRunner::default());
    let mut svc = JobService::new(f.store.clone(), f.jobs.clone(), runner, f.clock.clone());
    let job = svc.dispatch(f.session.id, "x").unwrap();
    let state = ReviewState::Done {
        review: Review {
            verdict: ReviewVerdict::Concerns,
            concerns: vec!["no test".into()],
            diffs_checked: 1,
        },
        job: JobId(99),
        cost_usd: 0.01,
    };
    f.jobs.set_review(job, &state).unwrap();
    let last = f.ledger.entries().last().unwrap().clone();
    assert_eq!(last.draft.kind, LedgerKind::ReviewVerdict);
    assert_eq!(last.draft.actor, "reviewer");
    assert_eq!(
        last.draft.payload_json,
        format!(r#"{{"review":{}}}"#, state.encode())
    );
}

#[test]
fn decisions_are_recorded_after_they_are_stored() {
    let f = fixture();
    let repo = Rc::new(FakeDecisionRepo::default());
    let decisions = LedgeredDecisions::new(repo.clone(), f.service.clone());
    let d = DecisionRecord {
        job_id: Some(JobId(3)),
        tool: "Bash".into(),
        subject: "rm -rf /".into(),
        outcome: Outcome::Deny,
        rule: Some("Bash(rm -rf:*)".into()),
        latency_us: 12,
        ts: 77,
        source: Source::Guard,
        confidence: None,
    };
    decisions.record(&d).unwrap();
    assert_eq!(repo.rows(), std::slice::from_ref(&d));
    let e = f.ledger.entries().last().unwrap().clone();
    assert_eq!(
        (
            e.draft.kind,
            e.draft.ts,
            e.draft.actor.as_str(),
            e.draft.job
        ),
        (LedgerKind::GuardDecision, 77, "guard", Some(JobId(3)))
    );
    assert!(e.draft.payload_json.contains(r#""outcome":"deny""#));

    // A decision that couldn't be stored isn't recorded either.
    repo.fail(true);
    assert!(decisions.record(&d).is_err());
    assert_eq!(f.ledger.entries().len(), 2);
}

#[test]
fn verify_walks_pages_and_reports_the_first_break() {
    let f = fixture();
    for i in 0..25 {
        f.service
            .record_agent_event(JobId(1), None, "text", "lead", &format!("e{i}"), i)
            .unwrap();
    }
    let ok = verify_in_pages(&*f.ledger, 7).unwrap();
    assert_eq!(ok.entries, 26);
    assert_eq!(ok.head.map(|(seq, _)| seq), Some(26));
    assert!(ok.is_ok());

    f.ledger
        .tamper(12, r#"{"event":"text","summary":"forged"}"#);
    let bad = verify_in_pages(&*f.ledger, 7).unwrap();
    assert_eq!(
        bad.broken,
        Some(ChainBreak {
            seq: 12,
            reason: BreakReason::HashMismatch
        })
    );
    assert_eq!(bad.entries, 11);

    f.ledger.remove(20);
    assert_eq!(verify_all(&*f.ledger).unwrap().broken.unwrap().seq, 12);
}

#[test]
fn deleted_entries_are_a_gap() {
    let f = fixture();
    for i in 0..5 {
        f.service
            .record_agent_event(JobId(1), None, "text", "lead", "x", i)
            .unwrap();
    }
    f.ledger.remove(3);
    assert_eq!(
        verify_all(&*f.ledger).unwrap().broken,
        Some(ChainBreak {
            seq: 4,
            reason: BreakReason::Gap { expected: 3 }
        })
    );
}

#[test]
fn empty_ledger_verifies_and_tail_is_bounded() {
    let f = fixture();
    let r = verify_all(&*f.ledger).unwrap();
    assert_eq!((r.entries, r.head, r.broken), (0, None, None));
    assert!(tail(&*f.ledger, 5).unwrap().is_empty());
    for i in 0..9 {
        f.service
            .record_agent_event(JobId(1), None, "text", "lead", "x", i)
            .unwrap();
    }
    let seqs: Vec<u64> = tail(&*f.ledger, 3).unwrap().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [8, 9, 10]);
    assert_eq!(tail(&*f.ledger, 100).unwrap().len(), 10);
    assert!(tail(&*f.ledger, 0).unwrap().is_empty());
}
