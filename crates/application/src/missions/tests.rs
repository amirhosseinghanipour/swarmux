use super::*;
use crate::evidence::EvidenceStore;
use crate::gate::GateState;
use crate::ports::{RunnerMsg, SessionRepository};
use crate::testing::{
    FakeJobGates, FakeLinter, FakeRunner, FixedClock, InMemoryLedger, InMemoryMissions,
    InMemoryStore, asks,
};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::mission::{Acceptance, Budget, Constraints, FindingKind};
use swarmux_domain::{AgentEvent, Session};

struct Fixture {
    svc: MissionService,
    jobs: JobService,
    store: Rc<InMemoryMissions>,
    linter: Rc<FakeLinter>,
    gates: Rc<FakeJobGates>,
    runner: Rc<FakeRunner>,
    ledger: Rc<InMemoryLedger>,
    ledger_service: Rc<LedgerService>,
    db: Rc<InMemoryStore>,
    clock: Rc<FixedClock>,
    session: Session,
}

fn fixture() -> Fixture {
    let db = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "search", "/repo".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*db, &session).unwrap();
    let runner = Rc::new(FakeRunner::default());
    let clock = Rc::new(FixedClock::new(10_000));
    let jobs = JobService::new(db.clone(), db.clone(), runner.clone(), clock.clone());
    let store = Rc::new(InMemoryMissions::default());
    let linter = Rc::new(FakeLinter::default());
    let gates = Rc::new(FakeJobGates::default());
    let ledger = Rc::new(InMemoryLedger::default());
    let ledger_service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let svc = MissionService::new(store.clone(), clock.clone())
        .unwrap()
        .with_linter(linter.clone())
        .with_gates(gates.clone())
        .with_ledger(ledger_service.clone());
    Fixture {
        svc,
        jobs,
        store,
        linter,
        gates,
        runner,
        ledger,
        ledger_service,
        db,
        clock,
        session,
    }
}

fn contract(checks: &[&str], protected: &[&str]) -> Contract {
    Contract {
        title: "Fast search".into(),
        goal: "make search answer in under 50 ms on the bench corpus".into(),
        acceptance: Acceptance {
            checks: checks.iter().map(|c| c.to_string()).collect(),
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/search/**".into()],
            protected: protected.iter().map(|c| c.to_string()).collect(),
        },
        budget: Budget {
            usd: Some(2.0),
            ..Budget::default()
        },
        clarifications: Vec::new(),
    }
}

impl Fixture {
    fn create(&mut self, c: Contract) -> String {
        let session = Some(self.session.id);
        self.svc.create(c, "/repo", session).unwrap()
    }

    fn state(&self, slug: &str) -> MissionState {
        self.svc.get(slug).unwrap().state
    }

    fn pump(&mut self) -> MissionReport {
        self.jobs.pump();
        let facts = self.jobs.facts();
        self.svc.pump(&facts, &HashMap::new())
    }

    fn pump_with(&mut self, job: JobId, outcome: GateOutcome, at: Millis) -> MissionReport {
        self.jobs.pump();
        let facts = self.jobs.facts();
        let gates = HashMap::from([(job, gate(outcome, at))]);
        self.svc.pump(&facts, &gates)
    }

    fn finish(&self, i: usize, cost: f64, code: i32) {
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
    }

    fn mission_entries(&self) -> Vec<String> {
        self.ledger
            .entries()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::Mission)
            .map(|e| e.draft.payload_json)
            .collect()
    }
}

fn gate(outcome: GateOutcome, at: Millis) -> GateState {
    GateState {
        outcome,
        blocks: 0,
        max_blocks: 3,
        checks: vec![("cargo test".into(), outcome == GateOutcome::Pass, false)],
        protected_changed: 0,
        evidence_gaps: 0,
        hidden: None,
        tampering: 0,
        digest: String::new(),
        problem: None,
        at,
    }
}

#[test]
fn a_new_mission_is_linted_at_once_and_questions_follow() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &[]));
    assert_eq!(slug, "fast-search");
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Linted);
    let lint = m.lint.as_ref().unwrap();
    assert_eq!(lint.source, "local");
    assert!(
        lint.findings
            .iter()
            .any(|f| f.kind == FindingKind::UnprotectedOracle),
        "{lint:?}"
    );
    assert!(f.svc.linting(&slug));
    let asked = &f.linter.requests()[0];
    assert_eq!(asked.contract.title, "Fast search");
    assert!(
        !asked.findings.is_empty(),
        "the model sees the local findings"
    );

    f.linter.reply(
        0,
        asks(&["Which corpus?", "p50 or p99?", "Cold cache?", "A fourth?"]),
    );
    assert!(f.pump().changed);
    let m = f.svc.get(&slug).unwrap();
    let lint = m.lint.as_ref().unwrap();
    assert_eq!(lint.source, "haiku");
    assert_eq!(lint.questions.len(), 3, "at most three");
    let asked: Vec<&str> = m
        .contract
        .clarifications
        .iter()
        .map(|c| c.question.as_str())
        .collect();
    assert_eq!(asked, ["Which corpus?", "p50 or p99?", "Cold cache?"]);
    assert_eq!(m.spend.plan, 0.0005);
    assert!(!f.svc.linting(&slug));
    // Recorded: drafted, then linted.
    let entries = f.mission_entries();
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(entries[1].contains(r#""from":"draft""#) && entries[1].contains(r#""to":"linted""#));
}

#[test]
fn answers_survive_a_new_lint_and_reach_the_prompt() {
    let mut f = fixture();
    f.linter.answer_with(asks(&["Which corpus?"]));
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    f.pump();
    f.svc.answer(&slug, 0, "  the bench corpus ").unwrap();
    assert!(f.svc.answer(&slug, 5, "x").is_err());
    f.svc.lint(&slug).unwrap();
    f.pump();
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.contract.clarifications[0].answer, "the bench corpus");
    f.svc.launch(&slug, &mut f.jobs).unwrap();
    let prompt = f.runner.request(0).prompt;
    assert!(
        prompt.contains("- Which corpus? the bench corpus"),
        "{prompt}"
    );
}

#[test]
fn a_reply_to_an_older_lint_is_dropped() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    f.svc.lint(&slug).unwrap();
    f.linter.reply(0, asks(&["stale?"]));
    f.pump();
    assert!(f.svc.linting(&slug), "the newer lint is still out");
    assert!(f.svc.get(&slug).unwrap().contract.clarifications.is_empty());
    f.linter.reply(1, asks(&["fresh?"]));
    f.pump();
    assert_eq!(
        f.svc.get(&slug).unwrap().contract.clarifications[0].question,
        "fresh?"
    );
}

#[test]
fn a_linter_failure_keeps_the_local_lint() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    f.linter.reply(
        0,
        LintReply {
            result: Err("claude exited with code 1".into()),
            cost_usd: 0.0,
            source: "haiku".into(),
        },
    );
    f.pump();
    let lint = f.svc.get(&slug).unwrap().lint.clone().unwrap();
    assert_eq!(lint.source, "local");
    assert_eq!(lint.problem.as_deref(), Some("claude exited with code 1"));
}

#[test]
fn fix_protects_the_oracles_and_lints_again() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test", "sh scripts/bench.sh"], &[]));
    assert_eq!(f.svc.fix(&slug).unwrap(), 2);
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(
        m.contract.constraints.protected,
        ["tests/**", "scripts/bench.sh"]
    );
    assert_eq!(m.state, MissionState::Linted);
    assert!(
        !m.lint
            .as_ref()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::UnprotectedOracle)
    );
    assert_eq!(f.svc.fix(&slug).unwrap(), 0);
}

#[test]
fn a_mission_nothing_can_check_is_refused() {
    let mut f = fixture();
    let slug = f.create(contract(&[], &[]));
    assert!(f.svc.get(&slug).unwrap().lint.as_ref().unwrap().blocking());
    let e = f.svc.launch(&slug, &mut f.jobs).unwrap_err();
    assert!(e.to_string().contains("no acceptance checks"), "{e}");
    assert_eq!(f.state(&slug), MissionState::Refused);
    assert_eq!(f.runner.started(), 0);
    // Editing brings it back.
    f.svc
        .edit(&slug, contract(&["cargo test"], &["tests/**"]))
        .unwrap();
    assert_eq!(f.state(&slug), MissionState::Linted);
    assert!(f.svc.get(&slug).unwrap().note.is_none());
}

#[test]
fn launch_compiles_a_job_and_its_gate() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test -p search"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    let m = f.svc.get(&slug).unwrap();
    assert_eq!((m.state, m.job), (MissionState::Queued, Some(job)));
    let req = f.runner.request(0);
    assert_eq!(req.session_id, f.session.id);
    // Of the $2.00 budget the job may spend plan and implement shares
    // (70%); verify and review stay reserved.
    let cap = req.max_budget_usd.unwrap();
    assert!((cap - 1.4).abs() < 1e-9, "{cap}");
    assert!(
        req.prompt.contains("- `cargo test -p search`"),
        "{}",
        req.prompt
    );
    let assigned = f.gates.assigned.borrow();
    assert_eq!(assigned[0].0, job);
    assert_eq!(assigned[0].1.checks[0].program, "cargo");
    assert_eq!(assigned[0].1.protected, ["tests/**"]);
    let e = f.svc.launch(&slug, &mut f.jobs).unwrap_err();
    assert!(e.to_string().contains("only a linted mission"), "{e}");
    assert!(
        f.svc.edit(&slug, contract(&["x y"], &[])).is_err(),
        "not while out"
    );
}

#[test]
fn delivered_only_after_the_job_ends_and_the_gate_passes() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    assert_eq!(f.state(&slug), MissionState::Running);
    // A pass at a stop while the job still runs is not a delivery.
    f.clock.advance(1_000);
    f.pump_with(job, GateOutcome::Pass, f.clock.now());
    assert_eq!(f.state(&slug), MissionState::Running);
    f.finish(0, 0.4, 0);
    f.pump_with(job, GateOutcome::Pass, f.clock.now());
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Delivered);
    assert_eq!(m.spend.implement, 0.4);
    assert!(
        f.gates.verified.borrow().is_empty(),
        "the stop's pass was enough"
    );
    let entries = f.mission_entries();
    let path: Vec<&str> = ["queued", "running", "verifying", "delivered"]
        .into_iter()
        .filter(|s| {
            entries
                .iter()
                .any(|e| e.contains(&format!(r#""to":"{s}""#)))
        })
        .collect();
    assert_eq!(path.len(), 4, "{entries:?}");
}

#[test]
fn without_a_fresh_verdict_the_checks_run_once_by_hand() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    f.clock.advance(1_000);
    f.finish(0, 0.1, 0);
    // A pass from before the job started is not about this job's work.
    f.pump_with(job, GateOutcome::Pass, 0);
    assert_eq!(f.state(&slug), MissionState::Verifying);
    f.pump();
    assert_eq!(
        *f.gates.verified.borrow(),
        [(job, std::path::PathBuf::from("/repo"))],
        "asked once"
    );
    f.pump_with(job, GateOutcome::Escalate, f.clock.now());
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Escalated);
    assert!(m.note.as_deref().unwrap().contains("gate gave up"));
}

#[test]
fn checks_that_cannot_start_escalate() {
    let mut f = fixture();
    *f.gates.broken.borrow_mut() = Some("cannot start the checks: no threads".into());
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    f.finish(0, 0.1, 0);
    f.pump();
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Escalated);
    assert_eq!(
        m.note.as_deref(),
        Some("cannot start the checks: no threads")
    );
}

#[test]
fn a_failed_job_escalates() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    f.finish(0, 0.1, 1);
    f.pump();
    assert_eq!(f.state(&slug), MissionState::Escalated);
}

#[test]
fn running_out_of_budget_escalates_and_stops_the_job() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    // An expensive lint counts against the same budget as the job.
    f.svc.missions[0].spend.plan = 2.5;
    let report = f.pump();
    assert_eq!(report.cancel, [job]);
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Escalated);
    assert!(
        m.note
            .as_deref()
            .unwrap()
            .starts_with("spent $2.50 of $2.00")
    );
}

#[test]
fn missions_reload_from_the_store_in_order() {
    let mut f = fixture();
    let a = f.create(contract(&["cargo test"], &["tests/**"]));
    let b = f.create(contract(&["cargo test"], &["tests/**"]));
    assert_eq!(b, "fast-search-2");
    let again = MissionService::new(f.store.clone(), f.clock.clone()).unwrap();
    let slugs: Vec<&str> = again.missions().iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(slugs, [a.as_str(), b.as_str()]);
    assert!(
        f.svc.create(Contract::default(), "/r", None).is_err(),
        "a goal is needed"
    );
}

/// A fixture whose missions are sealed by an evidence service on the same
/// ledger; `sealed` says whether the repository can be read.
fn evidenced(f: &mut Fixture, sealed: bool) -> Rc<crate::evidence::testing::InMemoryEvidence> {
    use crate::evidence::testing::{FakeVcs, InMemoryEvidence};
    let bundles = Rc::new(InMemoryEvidence::default());
    let vcs = Rc::new(FakeVcs::at("cafe0000"));
    if !sealed {
        *vcs.facts.borrow_mut() = None;
    }
    let evidence = Rc::new(EvidenceService::new(
        bundles.clone(),
        f.ledger_service.clone(),
        f.db.clone(),
        vcs,
        f.clock.clone(),
    ));
    let svc = std::mem::replace(
        &mut f.svc,
        MissionService::new(f.store.clone(), f.clock.clone()).unwrap(),
    );
    f.svc = svc.with_evidence(evidence);
    bundles
}

/// Records a passing stop of `job` on the ledger, as the gate would.
fn pass_on_ledger(f: &Fixture, job: JobId) {
    use swarmux_domain::gate::{CheckRun, GateFindings, GateOrigin};
    let run = CheckRun {
        name: "cargo test".into(),
        command: "cargo test".into(),
        exit: Some(0),
        timed_out: false,
        spawn_error: None,
        duration_ms: 10,
        output: String::new(),
        flaky: false,
    };
    let record = crate::gate::GateRecord {
        job: Some(job),
        session: String::new(),
        event: Some(swarmux_domain::HookEvent::Stop),
        root: "/repo".into(),
        origin: GateOrigin::File,
        outcome: GateOutcome::Pass,
        blocks: 0,
        max_blocks: 3,
        findings: GateFindings {
            runs: vec![run],
            ..GateFindings::default()
        },
        digest: String::new(),
        problem: None,
    };
    f.ledger_service
        .record_gate(&record, Some(f.session.id), f.clock.now())
        .unwrap();
}

#[test]
fn a_delivered_mission_has_a_bundle() {
    let mut f = fixture();
    let bundles = evidenced(&mut f, true);
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    f.finish(0, 0.4, 0);
    pass_on_ledger(&f, job);
    f.pump_with(job, GateOutcome::Pass, f.clock.now());
    assert_eq!(f.state(&slug), MissionState::Delivered);
    let stored = bundles.get(&slug).unwrap().expect("a bundle");
    assert_eq!(stored.bundle.job, job.0);
    assert_eq!(stored.bundle.head_sha, "cafe0000");
    assert_eq!(stored.bundle.checks[0].name, "cargo test");
}

#[test]
fn without_a_bundle_a_mission_is_not_delivered() {
    let mut f = fixture();
    let bundles = evidenced(&mut f, false);
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    f.finish(0, 0.4, 0);
    pass_on_ledger(&f, job);
    f.pump_with(job, GateOutcome::Pass, f.clock.now());
    let m = f.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Escalated);
    assert!(
        m.note.as_deref().unwrap().contains("no evidence bundle"),
        "{:?}",
        m.note
    );
    assert!(bundles.list().unwrap().is_empty());
}

#[test]
fn missions_delivered_before_bundles_existed_get_one() {
    let mut f = fixture();
    let slug = f.create(contract(&["cargo test"], &["tests/**"]));
    let job = f.svc.launch(&slug, &mut f.jobs).unwrap();
    f.pump();
    f.finish(0, 0.4, 0);
    pass_on_ledger(&f, job);
    f.pump_with(job, GateOutcome::Pass, f.clock.now());
    assert_eq!(f.state(&slug), MissionState::Delivered);
    let bundles = Rc::new(crate::evidence::testing::InMemoryEvidence::default());
    // The new service starts from the stored missions, like a restart.
    f.svc = MissionService::new(f.store.clone(), f.clock.clone())
        .unwrap()
        .with_evidence(Rc::new(EvidenceService::new(
            bundles.clone(),
            f.ledger_service.clone(),
            f.db.clone(),
            Rc::new(crate::evidence::testing::FakeVcs::at("beef0000")),
            f.clock.clone(),
        )));
    let report = f.pump();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        bundles.get(&slug).unwrap().unwrap().bundle.head_sha,
        "beef0000"
    );
}
