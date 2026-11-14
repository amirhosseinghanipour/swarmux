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
        cache_creation: None,
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
        extra: crate::DecisionExtra::default(),
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

// --- X1: the policy in the ledger -------------------------------------------

use crate::ports::FallbackRecord;
use crate::testing::{FakeRuleStore, FakeSpool};
use swarmux_domain::{RuleScope, RuleSource, StoredRule};

fn policy_entries(ledger: &InMemoryLedger) -> Vec<LedgerEntry> {
    ledger
        .entries()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::Policy)
        .collect()
}

fn stored(text: &str) -> StoredRule {
    StoredRule::new(
        text,
        RuleScope::Repo,
        Some("/repo"),
        RuleSource::AlwaysAllow,
        500,
        Some(900),
    )
}

#[test]
fn rules_added_and_removed_are_in_the_ledger_with_their_hash() {
    let f = fixture();
    let store = LedgeredRules::new(Rc::new(FakeRuleStore::default()), f.service.clone());
    let rule = stored("Bash(make all)");
    store.add(&rule).unwrap();
    assert!(store.remove(&rule.id).unwrap());
    assert!(!store.remove(&rule.id).unwrap(), "nothing more to record");
    let entries = policy_entries(&f.ledger);
    assert_eq!(entries.len(), 2);
    let added = &entries[0].draft.payload_json;
    assert!(added.contains("\"event\":\"added\""), "{added}");
    assert!(added.contains("Bash(make all)"), "{added}");
    assert!(added.contains("\"scope\":\"repo\""), "{added}");
    assert!(added.contains("\"root\":\"/repo\""), "{added}");
    assert!(added.contains("\"source\":\"always_allow\""), "{added}");
    assert!(added.contains("\"expires_at\":900"), "{added}");
    assert!(added.contains("\"rule_sha256\":\""), "{added}");
    assert!(
        entries[1]
            .draft
            .payload_json
            .contains("\"event\":\"removed\"")
    );
    assert_eq!(entries[0].draft.actor, "operator");
    assert!(
        verify_all(&*f.ledger).unwrap().is_ok(),
        "the chain still holds"
    );
}

#[test]
fn expired_rules_are_pruned_and_recorded() {
    let f = fixture();
    let inner = Rc::new(FakeRuleStore::default());
    let store = LedgeredRules::new(inner.clone(), f.service.clone());
    store.add(&stored("Bash(old)")).unwrap();
    let fresh = StoredRule {
        expires_at: None,
        ..stored("Bash(new)")
    };
    store.add(&fresh).unwrap();
    f.clock.advance(10_000);
    let gone = store.prune().unwrap();
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].text, "Bash(old)");
    assert_eq!(inner.rules(), [fresh]);
    let entries = policy_entries(&f.ledger);
    assert!(
        entries
            .last()
            .unwrap()
            .draft
            .payload_json
            .contains("\"event\":\"expired\"")
    );
}

fn fallback(reason: &str, mode: &str, line: &str) -> FallbackRecord {
    FallbackRecord {
        ts: 777,
        job: Some(JobId(4)),
        tool: "Bash".into(),
        input_sha256: "cd".repeat(32),
        reason: reason.into(),
        mode: mode.into(),
        line_id: line.into(),
    }
}

#[test]
fn spooled_fallbacks_reach_the_ledger_once_and_the_spool_is_emptied() {
    let f = fixture();
    let spool = FakeSpool::default();
    spool
        .0
        .borrow_mut()
        .push(fallback("connect failed", "deny", "l1"));
    spool
        .0
        .borrow_mut()
        .push(fallback("no answer", "pass", "l2"));
    assert_eq!(ingest_fallbacks(&spool, &f.service).unwrap(), 2);
    assert!(spool.0.borrow().is_empty());
    assert_eq!(ingest_fallbacks(&spool, &f.service).unwrap(), 0);
    let entries = policy_entries(&f.ledger);
    assert_eq!(entries.len(), 2);
    let first = &entries[0];
    assert_eq!(first.draft.job, Some(JobId(4)));
    assert_eq!(first.draft.actor, "guard-hook");
    let p = &first.draft.payload_json;
    for part in [
        "\"event\":\"guard_fallback\"",
        "\"at\":777",
        "\"tool\":\"Bash\"",
        "\"reason\":\"connect failed\"",
        "\"mode\":\"deny\"",
        "\"line_id\":\"l1\"",
        "\"policy_sha256\":\"",
    ] {
        assert!(p.contains(part), "{part} in {p}");
    }
    assert!(verify_all(&*f.ledger).unwrap().is_ok());
}

#[test]
fn decisions_carry_their_rule_hash_resolver_and_attribution() {
    let f = fixture();
    let record = DecisionRecord {
        job_id: Some(JobId(1)),
        tool: "Bash".into(),
        subject: "make".into(),
        outcome: Outcome::Allow,
        rule: Some("approved in swarmux".into()),
        latency_us: 5,
        ts: 9,
        source: Source::Guard,
        confidence: None,
        extra: crate::DecisionExtra {
            agent_id: Some("agent-7".into()),
            agent_type: Some("Explore".into()),
            resolver: Some("human-key".into()),
            policy_sha256: Some("ab".repeat(32)),
            rule_tier: Some("user".into()),
            rule_source: Some("/home/u/.config/swarmux/config.toml".into()),
        },
    };
    f.service.record_decision(&record).unwrap();
    let last = f.ledger.entries().pop().unwrap();
    let p = &last.draft.payload_json;
    for part in [
        "\"rule_sha256\":\"",
        "\"resolver\":\"human-key\"",
        "\"agent_id\":\"agent-7\"",
        "\"agent_type\":\"Explore\"",
        "\"rule_tier\":\"user\"",
        "\"rule_source\":\"/home/u/.config/swarmux/config.toml\"",
        "\"policy_sha256\":\"abab",
    ] {
        assert!(p.contains(part), "{part} in {p}");
    }
    assert!(
        !p.contains("jev"),
        "nothing about JEV is written any more: {p}"
    );
}

#[test]
fn a_decision_made_by_the_removed_jev_still_records_and_the_chain_holds() {
    // History written while JEV existed names it as the source; such a
    // record is still a record, and the ledger it joins still verifies.
    let f = fixture();
    let legacy = DecisionRecord {
        job_id: Some(JobId(1)),
        tool: "Bash".into(),
        subject: "make".into(),
        outcome: Outcome::Allow,
        rule: Some("jev 0.97".into()),
        latency_us: 5,
        ts: 9,
        source: Source::Jev,
        confidence: Some(0.97),
        extra: crate::DecisionExtra {
            resolver: Some("model".into()),
            ..crate::DecisionExtra::default()
        },
    };
    f.service.record_decision(&legacy).unwrap();
    let last = f.ledger.entries().pop().unwrap();
    assert_eq!(last.draft.kind, LedgerKind::GuardDecision);
    let p = &last.draft.payload_json;
    assert!(p.contains("\"source\":\"jev\""), "{p}");
    assert!(p.contains("\"confidence\":0.97"), "{p}");
    assert!(p.contains("\"resolver\":\"model\""), "{p}");
    assert!(verify_all(&*f.ledger).unwrap().is_ok());
}

// --- X2: the gate's own decisions in the ledger ------------------------------

#[test]
fn a_selftest_and_a_holdout_migration_are_ledgered_and_the_chain_holds() {
    let f = fixture();
    f.service
        .record_gate_selftest(
            "/repo",
            &[
                (
                    "planted conftest.py".to_string(),
                    true,
                    "conftest.py: harness file created".to_string(),
                ),
                (
                    "deleted an assertion".to_string(),
                    false,
                    "the gate found nothing wrong".to_string(),
                ),
            ],
            &[("another".to_string(), "no test".to_string())],
            &"cd".repeat(32),
            2_000,
        )
        .unwrap();
    f.service
        .record_holdout_migrated("shop", "shop.id-0123456789ab", 3_000)
        .unwrap();
    let entries = policy_entries(&f.ledger);
    assert_eq!(entries.len(), 2);
    let selftest = &entries[0].draft.payload_json;
    for says in [
        "\"event\":\"gate_selftest\"",
        "\"outcome\":\"fail\"",
        "\"root\":\"/repo\"",
        "\"caught\":true",
        "\"caught\":false",
        "\"skipped\":[{\"name\":\"another\"",
        "\"policy_sha256\":\"cdcd",
    ] {
        assert!(selftest.contains(says), "{says} in {selftest}");
    }
    let moved = &entries[1].draft.payload_json;
    assert!(moved.contains("\"event\":\"holdout_migrated\""), "{moved}");
    assert!(moved.contains("\"from\":\"shop\""), "{moved}");
    assert!(moved.contains("\"to\":\"shop.id-0123456789ab\""), "{moved}");
    assert!(
        verify_all(&*f.ledger).unwrap().is_ok(),
        "the chain still holds"
    );

    // Every probe refused: the outcome is a pass.
    f.service
        .record_gate_selftest(
            "/repo",
            &[("planted".to_string(), true, "found".to_string())],
            &[],
            "ab",
            4_000,
        )
        .unwrap();
    assert!(
        policy_entries(&f.ledger)[2]
            .draft
            .payload_json
            .contains("\"outcome\":\"pass\"")
    );
    // None made is not a pass.
    f.service
        .record_gate_selftest("/repo", &[], &[], "ab", 5_000)
        .unwrap();
    assert!(
        policy_entries(&f.ledger)[3]
            .draft
            .payload_json
            .contains("\"outcome\":\"fail\"")
    );
}

#[test]
fn a_manual_run_that_found_nothing_wrong_but_could_not_vouch_is_never_recorded_as_a_pass() {
    use crate::gate::GateRecord;
    use swarmux_domain::gate::{GateFindings, GateOrigin, GateOutcome, RanIn};
    let f = fixture();
    let findings = GateFindings {
        ran_in: Some(RanIn::Worktree),
        unattended: true,
        ..GateFindings::default()
    };
    assert!(findings.clean() && !findings.incomplete().is_empty());
    let record = |outcome| GateRecord {
        job: None,
        session: String::new(),
        event: None,
        root: "/r".into(),
        origin: GateOrigin::File,
        outcome,
        blocks: 0,
        max_blocks: 0,
        findings: findings.clone(),
        digest: String::new(),
        problem: None,
    };
    f.service
        .record_gate(&record(GateOutcome::Escalate), None, 1_000)
        .unwrap();
    let entries: Vec<String> = f
        .ledger
        .entries()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::GateVerdict)
        .map(|e| e.draft.payload_json)
        .collect();
    assert!(
        entries[0].contains("\"outcome\":\"escalate\""),
        "{}",
        entries[0]
    );
}

// ---- the launch profile, the canaries and the runtime record ----

mod runtime {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::*;
    use crate::runtime::{Clearance, ConfiguredTrust, LaunchIntent, RuntimeGate, RuntimeSettings};
    use swarmux_domain::JobStatus;
    use swarmux_domain::runtime::{
        CanaryCheck, CanaryReport, CheckStatus, HostPolicy, RuntimeLaunch, RuntimeObserved,
        RuntimeProfile, VersionStanding,
    };
    use swarmux_domain::telemetry::Environment;

    /// A gate that answers what the test says and remembers what it was asked.
    struct FakeGate {
        answer: RefCell<Clearance>,
        asked: RefCell<Vec<RuntimeProfile>>,
    }

    impl FakeGate {
        fn new(answer: Clearance) -> Rc<FakeGate> {
            Rc::new(FakeGate {
                answer: RefCell::new(answer),
                asked: RefCell::default(),
            })
        }
    }

    impl RuntimeGate for FakeGate {
        fn clearance(&self, profile: RuntimeProfile) -> Clearance {
            self.asked.borrow_mut().push(profile);
            self.answer.borrow().clone()
        }

        fn launch(&self, intent: &LaunchIntent) -> Option<RuntimeLaunch> {
            Some(RuntimeLaunch {
                runtime: "fake".into(),
                profile: intent.profile,
                profile_reason: String::new(),
                version: Some("1.2.3".into()),
                standing: VersionStanding::Tested,
                sources: "user".into(),
                strict_mcp: intent.profile == RuntimeProfile::Controlled,
                on_failure_block: true,
                settings_hash: "abc".into(),
                host: HostPolicy::default(),
                policy_sha256: "fake".into(),
                canary_seq: None,
            })
        }

        fn observe(&self, _: Option<&RuntimeLaunch>, env: &Environment) -> RuntimeObserved {
            RuntimeObserved {
                version: env.claude_code_version.clone(),
                capabilities: env.capabilities.clone(),
                mismatches: vec!["a server nobody asked for".into()],
                ..RuntimeObserved::default()
            }
        }
    }

    fn report(passed: bool) -> CanaryReport {
        CanaryReport {
            version: Some("1.2.3".into()),
            profile: Some(RuntimeProfile::Controlled),
            policy_sha256: "fake".into(),
            checks: vec![CanaryCheck {
                name: "deny".into(),
                status: if passed {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: if passed {
                    "the hook ran and the call was stopped".into()
                } else {
                    "the call ran although the hook denied it".into()
                },
                required: true,
            }],
        }
    }

    struct Rig {
        f: Fixture,
        runner: Rc<FakeRunner>,
        svc: JobService,
        gate: Rc<FakeGate>,
    }

    fn rig(answer: Clearance, trusted: &[&str]) -> Rig {
        let f = fixture();
        let runner = Rc::new(FakeRunner::default());
        let gate = FakeGate::new(answer);
        let trust = ConfiguredTrust::new(trusted.iter().map(PathBuf::from).collect());
        let svc = JobService::new(
            f.store.clone(),
            f.jobs.clone(),
            runner.clone(),
            f.clock.clone(),
        )
        .with_runtime(
            gate.clone(),
            f.service.clone(),
            RuntimeSettings::default(),
            Rc::new(trust),
        );
        Rig {
            f,
            runner,
            svc,
            gate,
        }
    }

    fn payloads(f: &Fixture, kind: LedgerKind) -> Vec<String> {
        f.ledger
            .entries()
            .into_iter()
            .filter(|e| e.draft.kind == kind)
            .map(|e| e.draft.payload_json)
            .collect()
    }

    #[test]
    fn an_unattended_mission_is_refused_when_the_canary_fails() {
        let mut r = rig(
            Clearance::Refused {
                why: "deny fail: the call ran although the hook denied it".into(),
                report: report(false),
            },
            &[],
        );
        let job = r
            .svc
            .dispatch_capped(r.f.session.id, "fix the bug", Some(1.0))
            .unwrap();
        let rep = r.svc.pump();
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        // Nothing was started: the runner never saw it, and no job_started.
        assert_eq!(r.runner.started(), 0);
        assert!(payloads(&r.f, LedgerKind::JobStarted).is_empty());
        assert_eq!(r.f.store.status(job).unwrap(), JobStatus::Failed);
        let finished = payloads(&r.f, LedgerKind::JobFinished);
        assert!(
            finished[0].contains("refused before starting: deny fail"),
            "{}",
            finished[0]
        );
        // The failed canary is in the ledger, with its report.
        let canary = payloads(&r.f, LedgerKind::Runtime);
        assert_eq!(canary.len(), 1);
        assert!(canary[0].contains(r#""event":"canary""#), "{}", canary[0]);
        assert!(canary[0].contains(r#""passed":false"#), "{}", canary[0]);
        assert!(canary[0].contains(r#""status":"fail""#), "{}", canary[0]);
    }

    #[test]
    fn a_job_waits_while_the_canary_runs_and_starts_when_it_passes() {
        let mut r = rig(Clearance::Pending, &[]);
        let job = r
            .svc
            .dispatch_capped(r.f.session.id, "fix the bug", None)
            .unwrap();
        r.svc.pump();
        r.svc.pump();
        assert_eq!(r.runner.started(), 0, "still waiting");
        assert_eq!(r.f.store.status(job).unwrap(), JobStatus::Queued);
        assert!(payloads(&r.f, LedgerKind::JobStarted).is_empty());

        *r.gate.answer.borrow_mut() = Clearance::Cleared(Some(report(true)));
        r.svc.pump();
        assert_eq!(r.runner.started(), 1);
        let canary_seq =
            r.f.ledger
                .entries()
                .iter()
                .find(|e| e.draft.kind == LedgerKind::Runtime)
                .map(|e| e.seq)
                .unwrap();
        let started = &payloads(&r.f, LedgerKind::JobStarted)[0];
        assert!(
            started.contains(&format!(r#""canary_seq":{canary_seq}"#)),
            "{started}"
        );
        // Cleared once: the next job that shares the result adds no entry.
        *r.gate.answer.borrow_mut() = Clearance::Cleared(None);
        r.svc
            .dispatch_capped(r.f.session.id, "second", None)
            .unwrap();
        r.svc.pump();
        assert_eq!(payloads(&r.f, LedgerKind::Runtime).len(), 1);
    }

    #[test]
    fn job_started_carries_the_runtime_record_and_why_the_profile() {
        let mut r = rig(Clearance::Cleared(Some(report(true))), &[]);
        r.svc
            .dispatch_capped(r.f.session.id, "mission work", None)
            .unwrap();
        r.svc.pump();
        let started = &payloads(&r.f, LedgerKind::JobStarted)[0];
        assert!(started.starts_with(r#"{"status":"running","runtime":{"runtime":"fake""#));
        assert!(started.contains(r#""profile":"controlled""#), "{started}");
        assert!(
            started
                .contains(r#""profile_reason":"unattended in a repository that is not trusted""#),
            "{started}"
        );
        assert!(started.contains(r#""version":"1.2.3""#), "{started}");
        assert!(started.contains(r#""settings_hash":"abc""#), "{started}");
        assert_eq!(r.runner.request(0).profile, RuntimeProfile::Controlled);
    }

    #[test]
    fn an_attended_job_inherits_and_needs_no_canary() {
        let mut r = rig(Clearance::Pending, &[]);
        r.svc.dispatch(r.f.session.id, "by hand").unwrap();
        r.svc.pump();
        assert_eq!(r.runner.started(), 1);
        assert!(
            r.gate.asked.borrow().is_empty(),
            "a hand-started job asks nobody"
        );
        assert_eq!(r.runner.request(0).profile, RuntimeProfile::Inherit);
        let started = &payloads(&r.f, LedgerKind::JobStarted)[0];
        assert!(started.contains(r#""profile":"inherit""#), "{started}");
        assert!(
            started.contains(r#""profile_reason":"attended""#),
            "{started}"
        );
    }

    #[test]
    fn a_trusted_folder_keeps_its_own_settings_for_unattended_work() {
        // The session's folder is /w.
        let mut r = rig(Clearance::Cleared(None), &["/w"]);
        r.svc
            .dispatch_capped(r.f.session.id, "mission work", None)
            .unwrap();
        r.svc.pump();
        assert_eq!(r.runner.request(0).profile, RuntimeProfile::Inherit);
        let started = &payloads(&r.f, LedgerKind::JobStarted)[0];
        assert!(
            started.contains(r#""profile_reason":"unattended in a trusted repository""#),
            "{started}"
        );
    }

    #[test]
    fn a_profile_chosen_for_one_job_beats_the_setting() {
        let mut r = rig(Clearance::Cleared(None), &[]);
        r.svc
            .dispatch_with_profile(r.f.session.id, "by hand", RuntimeProfile::Controlled)
            .unwrap();
        r.svc.pump();
        assert_eq!(r.runner.request(0).profile, RuntimeProfile::Controlled);
        let started = &payloads(&r.f, LedgerKind::JobStarted)[0];
        assert!(
            started.contains(r#""profile_reason":"chosen for the job or its mission""#),
            "{started}"
        );
    }

    #[test]
    fn what_the_agent_reports_is_recorded_next_to_what_was_asked() {
        let mut r = rig(Clearance::Cleared(None), &[]);
        r.svc.dispatch(r.f.session.id, "by hand").unwrap();
        r.svc.pump();
        let env = Environment {
            session_id: r.f.session.id.to_string(),
            claude_code_version: Some("1.2.3".into()),
            capabilities: vec!["interrupt_receipt_v1".into()],
            ..Environment::default()
        };
        r.runner
            .emit(0, RunnerMsg::Event(AgentEvent::Environment(Box::new(env))));
        r.svc.pump();
        let observed: Vec<String> = payloads(&r.f, LedgerKind::Runtime)
            .into_iter()
            .filter(|p| p.contains(r#""event":"observed""#))
            .collect();
        assert_eq!(observed.len(), 1);
        assert!(
            observed[0].contains(r#""version":"1.2.3""#),
            "{}",
            observed[0]
        );
        assert!(
            observed[0].contains(r#""capabilities":["interrupt_receipt_v1"]"#),
            "{}",
            observed[0]
        );
        assert!(
            observed[0].contains(r#""mismatches":["a server nobody asked for"]"#),
            "{}",
            observed[0]
        );
    }

    #[test]
    fn a_job_started_without_a_gate_has_no_runtime_record() {
        let f = fixture();
        let runner = Rc::new(FakeRunner::default());
        let mut svc = JobService::new(
            f.store.clone(),
            f.jobs.clone(),
            runner.clone(),
            f.clock.clone(),
        );
        svc.dispatch_capped(f.session.id, "mission work", None)
            .unwrap();
        svc.pump();
        assert_eq!(runner.started(), 1);
        let started = &payloads(&f, LedgerKind::JobStarted)[0];
        assert_eq!(started, r#"{"status":"running"}"#);
        // The profile still resolves: a mission job nobody trusted is controlled.
        assert_eq!(runner.request(0).profile, RuntimeProfile::Controlled);
    }
}

#[test]
fn a_mission_start_records_the_effective_config_and_its_layers() {
    use swarmux_domain::layers::Tier;
    let f = fixture();
    let config = crate::Provenance {
        config_sha256: "c0ffee".repeat(10),
        layers: vec![
            (Tier::Managed, "/etc/swarmux/managed.toml".into(), false),
            (
                Tier::User,
                "/home/u/.config/swarmux/config.toml".into(),
                true,
            ),
        ],
        ..crate::Provenance::default()
    };
    f.service
        .record_config_loaded(&config, Some("fast-search"), Some(JobId(3)), 1_000)
        .unwrap();
    let last = f.ledger.entries().pop().unwrap();
    assert_eq!(last.draft.kind, LedgerKind::Policy);
    assert_eq!(last.draft.job, Some(JobId(3)));
    let p = &last.draft.payload_json;
    for part in [
        "\"event\":\"config_loaded\"",
        "\"mission\":\"fast-search\"",
        "\"config_sha256\":\"c0ffee",
        "\"tier\":\"user\"",
        "\"found\":true",
        "\"found\":false",
    ] {
        assert!(p.contains(part), "{part} in {p}");
    }
}
