use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::gate::GateOutcome;
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract, MissionState};
use swarmux_domain::{
    AgentEvent, GuardPolicy, JobId, JobStatus, Session, SessionId, Unmatched, Verdict,
};

use super::*;
use crate::gate::GateState;
use crate::ledger::{LedgerService, verify_all};
use crate::missions::MissionService;
use crate::ports::{Clock, RunnerMsg, SessionRepository};
use crate::testing::{
    FakeDecisionRepo, FakeHookInbox, FakeJobGates, FakeRunner, FixedClock, InMemoryLedger,
    InMemoryMissions, InMemoryStore,
};
use crate::{GuardService, JobService, SharedPolicy};

struct Fx {
    engine: Engine,
    runner: Rc<FakeRunner>,
    inbox: Rc<FakeHookInbox>,
    ledger: Rc<InMemoryLedger>,
    ledger_service: Rc<LedgerService>,
    gates: Rc<FakeJobGates>,
    clock: Rc<FixedClock>,
    session: Session,
}

fn fixture() -> Fx {
    let db = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "search", "/repo".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*db, &session).unwrap();
    let runner = Rc::new(FakeRunner::default());
    let clock = Rc::new(FixedClock::new(10_000));
    let jobs = JobService::new(db.clone(), db.clone(), runner.clone(), clock.clone());
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
    let ledger_service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let gates = Rc::new(FakeJobGates::default());
    let missions = MissionService::new(Rc::new(InMemoryMissions::default()), clock.clone())
        .unwrap()
        .with_gates(gates.clone())
        .with_ledger(ledger_service.clone());
    let mut engine = Engine::new(jobs, guard);
    engine.missions = Some(missions);
    engine.ledger = Some(ledger_service.clone());
    Fx {
        engine,
        runner,
        inbox,
        ledger,
        ledger_service,
        gates,
        clock,
        session,
    }
}

fn contract(usd: Option<f64>) -> Contract {
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
        budget: Budget {
            usd,
            ..Budget::default()
        },
        due: None,
        clarifications: Vec::new(),
        repos: Vec::new(),
    }
}

fn passed(at: i64) -> GateState {
    GateState {
        outcome: GateOutcome::Pass,
        blocks: 0,
        max_blocks: 3,
        checks: vec![("cargo test".into(), true, false)],
        protected_changed: 0,
        evidence_gaps: 0,
        hidden: None,
        tampering: 0,
        checks_weakened: false,
        base_suite: None,
        digest: String::new(),
        problem: None,
        at,
    }
}

impl Fx {
    /// The `i`-th started job ends with `code`, costing `cost`.
    fn finish(&mut self, i: usize, cost: f64, code: i32) {
        self.runner.emit(
            i,
            RunnerMsg::Event(AgentEvent::Result {
                is_error: code != 0,
                subtype: "success".into(),
                cost_usd: cost,
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

    fn pump_missions(&mut self) {
        let facts = self.engine.jobs.facts();
        self.engine.pump_missions(&facts, &HashMap::new());
    }

    fn mission(&mut self, usd: Option<f64>) -> (String, JobId) {
        let session = Some(self.session.id);
        let missions = self.engine.missions.as_mut().unwrap();
        let slug = missions.create(contract(usd), "/repo", session).unwrap();
        let job = missions.launch(&slug, &mut self.engine.jobs).unwrap();
        self.engine.jobs.pump();
        (slug, job)
    }

    fn state(&self, slug: &str) -> MissionState {
        self.engine
            .missions
            .as_ref()
            .unwrap()
            .get(slug)
            .unwrap()
            .state
    }

    fn operator_entries(&self) -> Vec<String> {
        self.ledger
            .entries()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::Operator)
            .map(|e| e.draft.payload_json)
            .collect()
    }
}

#[test]
fn the_prompt_carries_the_note_in_the_operators_words() {
    let p = send_back_prompt("  use the staging bucket  ");
    assert!(p.contains("\n\nuse the staging bucket\n\n"), "{p}");
    assert!(p.starts_with("The operator read your work and sent it back"));
    assert!(p.contains("checks run again on your next claim"));
}

#[test]
fn a_failed_plain_job_resumes_with_the_note_and_the_note_is_recorded() {
    let mut f = fixture();
    let job = f
        .engine
        .jobs
        .dispatch(f.session.id, "migrate the schema")
        .unwrap();
    f.engine.jobs.pump();
    f.finish(0, 0.2, 1);
    assert_eq!(
        f.engine.jobs.stored(job).unwrap().unwrap().status,
        JobStatus::Failed
    );

    let sent = f
        .engine
        .send_back(job, "  keep the old column until the app ships  ")
        .unwrap();
    assert_eq!((sent.from, sent.mission.clone()), (job, None));
    assert_ne!(sent.job, job);
    // The agent is told the note, in the same session.
    f.engine.jobs.pump();
    assert_eq!(f.runner.started(), 2);
    let prompt = f.runner.request(1).prompt;
    assert!(
        prompt.contains("keep the old column until the app ships"),
        "{prompt}"
    );
    assert_eq!(f.runner.request(1).session_id, f.session.id);

    // The ledger names who, what, which job and what became of it.
    let entries = f.operator_entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    for part in [
        r#""action":"send_back""#.to_string(),
        r#""note":"keep the old column until the app ships""#.to_string(),
        format!(r#""job":{}"#, job.0),
        r#""outcome":"failed""#.to_string(),
        format!(r#""resumed_as":{}"#, sent.job.0),
        r#""gate_reruns":true"#.to_string(),
    ] {
        assert!(e.contains(&part), "{part} in {e}");
    }
    let entry = f
        .ledger
        .entries()
        .into_iter()
        .find(|e| e.draft.kind == LedgerKind::Operator)
        .unwrap();
    assert_eq!(entry.draft.actor, "operator");
    assert_eq!(entry.draft.job, Some(job));
    assert_eq!(entry.draft.ts, 10_000);
    assert_eq!(Some(entry.seq), sent.seq);
    // The chain still verifies with the note in it.
    assert!(verify_all(f.ledger_service.ledger()).unwrap().is_ok());
}

#[test]
fn a_finished_job_continues_the_same_way() {
    let mut f = fixture();
    let job = f.engine.jobs.dispatch(f.session.id, "add a test").unwrap();
    f.engine.jobs.pump();
    f.finish(0, 0.1, 0);
    let sent = f
        .engine
        .send_back(job, "also cover the empty case")
        .unwrap();
    f.engine.jobs.pump();
    assert!(
        f.runner
            .request(1)
            .prompt
            .contains("also cover the empty case")
    );
    assert!(f.operator_entries()[0].contains(r#""outcome":"done""#));
    assert_ne!(sent.job, job);
}

#[test]
fn a_note_needs_words_and_a_running_job_is_not_sent_back() {
    let mut f = fixture();
    let job = f.engine.jobs.dispatch(f.session.id, "x").unwrap();
    f.engine.jobs.pump();
    assert!(f.engine.send_back(job, "   ").is_err());
    assert!(matches!(
        f.engine.send_back(job, "stop that"),
        Err(AppError::JobNotFinished(_))
    ));
    assert!(
        f.operator_entries().is_empty(),
        "nothing was sent, nothing is recorded"
    );
    assert_eq!(f.runner.started(), 1);
}

#[test]
fn an_escalated_mission_follows_the_new_job_and_the_gate_is_given_back() {
    let mut f = fixture();
    let (slug, job) = f.mission(Some(5.0));
    f.finish(0, 0.1, 1);
    f.pump_missions();
    assert_eq!(f.state(&slug), MissionState::Escalated);

    let sent = f
        .engine
        .send_back(job, "the bench needs the warm cache")
        .unwrap();
    assert_eq!(sent.mission.as_deref(), Some(slug.as_str()));
    assert_eq!(f.state(&slug), MissionState::Queued);
    let m = f.engine.missions.as_ref().unwrap().get(&slug).unwrap();
    assert_eq!(m.job, Some(sent.job));
    assert_eq!(m.note, None);
    // The new job is judged as the first was.
    assert_eq!(
        f.gates.assigned.borrow().last().map(|(j, _)| *j),
        Some(sent.job)
    );
    assert!(f.operator_entries()[0].contains(&format!(r#""mission":"{slug}""#)));
    assert!(f.operator_entries()[0].contains(r#""outcome":"escalated""#));
    // Its next claim is judged again: the mission goes on to verifying,
    // then delivered when the gate passes.
    f.engine.jobs.pump();
    f.finish(1, 0.1, 0);
    f.pump_missions();
    assert_eq!(f.state(&slug), MissionState::Verifying);
    f.clock.advance(5);
    let facts = f.engine.jobs.facts();
    let gates = HashMap::from([(sent.job, passed(f.clock.now() + 1))]);
    f.engine.pump_missions(&facts, &gates);
    assert_eq!(f.state(&slug), MissionState::Delivered);
}

#[test]
fn a_spent_budget_is_not_sent_back_to_work() {
    let mut f = fixture();
    let (slug, job) = f.mission(Some(0.5));
    f.finish(0, 0.9, 1);
    f.pump_missions();
    assert_eq!(f.state(&slug), MissionState::Escalated);
    let why = f.engine.send_back(job, "again").unwrap_err().to_string();
    assert!(why.contains("raise the budget"), "{why}");
    assert!(f.operator_entries().is_empty());
    assert_eq!(f.runner.started(), 1, "no new job was started");
}

#[test]
fn delivered_work_is_not_sent_back() {
    let mut f = fixture();
    let (slug, job) = f.mission(Some(5.0));
    f.finish(0, 0.1, 0);
    f.pump_missions();
    f.clock.advance(20_000);
    let facts = f.engine.jobs.facts();
    let gates = HashMap::from([(job, passed(f.clock.now()))]);
    f.engine.pump_missions(&facts, &gates);
    assert_eq!(f.state(&slug), MissionState::Delivered);
    let why = f
        .engine
        .send_back(job, "one more thing")
        .unwrap_err()
        .to_string();
    assert!(why.contains("only escalated work"), "{why}");
    assert!(f.operator_entries().is_empty());
}

#[test]
fn a_held_call_is_denied_with_the_note_and_the_note_is_recorded() {
    let mut f = fixture();
    let job = f.engine.jobs.dispatch(f.session.id, "tidy").unwrap();
    f.engine.jobs.pump();
    let request = f.inbox.push(Some(job), "Bash", "rm -rf build");
    f.engine.guard.pump();
    assert_eq!(f.engine.guard.pending_count(), 1);

    let seq = f
        .engine
        .send_back_held(request, "use cargo clean instead")
        .unwrap();
    // The agent's answer is a denial that carries the words.
    let answer = f.inbox.answer_to(request).unwrap();
    assert_eq!(answer.verdict, Verdict::Deny);
    assert_eq!(answer.note.as_deref(), Some("use cargo clean instead"));
    assert_eq!(f.engine.guard.pending_count(), 0);
    // And the ledger holds it.
    let entries = f.operator_entries();
    assert_eq!(entries.len(), 1);
    for part in [
        r#""action":"deny_with_note""#.to_string(),
        r#""note":"use cargo clean instead""#.to_string(),
        format!(r#""job":{}"#, job.0),
        r#""outcome":"held""#.to_string(),
        r#""call":"Bash rm -rf build""#.to_string(),
        format!(r#""request":{request}"#),
        r#""gate_reruns":false"#.to_string(),
    ] {
        assert!(entries[0].contains(&part), "{part} in {}", entries[0]);
    }
    assert!(seq.is_some());
    // A call answered is gone: it cannot be answered again, and no second
    // entry is written.
    assert!(matches!(
        f.engine.send_back_held(request, "again"),
        Err(AppError::ApprovalNotFound)
    ));
    assert_eq!(f.operator_entries().len(), 1);
    // A note with no words is refused and the call stays held.
    let second = f.inbox.push(Some(job), "Bash", "make");
    f.engine.guard.pump();
    assert!(f.engine.send_back_held(second, "  ").is_err());
    assert_eq!(f.engine.guard.pending_count(), 1);
}

#[test]
fn without_a_ledger_the_note_is_still_delivered() {
    let mut f = fixture();
    f.engine.ledger = None;
    let job = f.engine.jobs.dispatch(f.session.id, "x").unwrap();
    f.engine.jobs.pump();
    f.finish(0, 0.0, 1);
    let sent = f.engine.send_back(job, "try again").unwrap();
    assert_eq!(sent.seq, None);
    assert!(f.operator_entries().is_empty());
}
