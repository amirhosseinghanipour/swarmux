use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::SharedPolicy;
use crate::contracts::{ContractGate, ContractJudgement};
use crate::ports::{HookRequest, NewJob, SessionRepository};
use crate::testing::{FixedClock, InMemoryLedger, InMemoryStore};
use swarmux_domain::gate::{RevertProbe, WorktreeChecks};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{AttentionKind, GuardPolicy, Session, Unmatched, Verdict};

/// One repository at `/w` with a `cargo test` check and protected oracles.
struct FakeSource {
    spec: Mutex<GateSpec>,
    pins: Mutex<Vec<Pin>>,
    /// The repository ships its checks and needs the owner's trust.
    shipped: Mutex<bool>,
}

impl FakeSource {
    fn new(max_blocks: u32) -> FakeSource {
        FakeSource {
            spec: Mutex::new(GateSpec {
                checks: vec![GateCheck::new("cargo test", "cargo", &["test"])],
                max_blocks,
                protected: vec!["tests/oracles/**".into()],
                ..GateSpec::default()
            }),
            pins: Mutex::new(vec![pin("tests/oracles/expected.txt", "42")]),
            shipped: Mutex::new(false),
        }
    }
}

fn pin(path: &str, hash: &str) -> Pin {
    Pin {
        path: path.into(),
        sha256: hash.into(),
    }
}

impl GateSource for FakeSource {
    fn load(&self, _dir: &Path) -> LoadedGate {
        LoadedGate {
            root: PathBuf::from("/w"),
            spec: lock(&self.spec).clone(),
            origin: GateOrigin::File,
            problem: None,
        }
    }

    fn pins(&self, _root: &Path, _globs: &[String]) -> Vec<Pin> {
        lock(&self.pins).clone()
    }

    fn supplied(&self, _gate: &LoadedGate) -> swarmux_domain::trust::Supplied {
        use swarmux_domain::trust::{CommandSource, Supplied, TrustEntry, WantedCommand};
        if !*lock(&self.shipped) {
            return Supplied::default();
        }
        Supplied {
            repo_id: "w.id-000000000000".into(),
            root: "/w".into(),
            entries: vec![TrustEntry::of_bytes(".swarmux/gates.toml", b"[[check]]")],
            commands: vec![WantedCommand {
                name: "cargo test".into(),
                line: "cargo test".into(),
                source: CommandSource::File,
            }],
            loosens: Vec::new(),
        }
    }
}

/// Fails with an assertion until fixed; may fail only every other run.
#[derive(Default)]
struct FakeChecker {
    fixed: Mutex<bool>,
    flaky: Mutex<bool>,
    runs: AtomicU32,
    ran: Mutex<Vec<String>>,
}

impl Checker for FakeChecker {
    fn run(&self, check: &GateCheck, _root: &Path) -> CheckRun {
        let n = self.runs.fetch_add(1, Ordering::SeqCst);
        lock(&self.ran).push(check.name.clone());
        // A rule's hidden check that never passes.
        let pass =
            check.name != "rule R1" && (*lock(&self.fixed) || (*lock(&self.flaky) && n % 2 == 1));
        CheckRun {
            name: check.name.clone(),
            command: check.command_line(),
            exit: Some(if pass { 0 } else { 101 }),
            timed_out: false,
            spawn_error: None,
            duration_ms: 1200,
            output: if pass {
                "test result: ok. 2 passed".into()
            } else {
                "test answer ... FAILED\nthread 'answer' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: 41\n right: 42".into()
            },
            flaky: false,
        }
    }
}

struct Fixture {
    keeper: Arc<GateKeeper>,
    source: Arc<FakeSource>,
    checker: Arc<FakeChecker>,
    service: GateService,
    ledger: Rc<InMemoryLedger>,
    store: Rc<InMemoryStore>,
    job: JobId,
}

fn fixture(max_blocks: u32) -> Fixture {
    fixture_with(max_blocks, None)
}

fn fixture_with(max_blocks: u32, rules: Option<Arc<dyn RuleChecks>>) -> Fixture {
    fixture_full(max_blocks, rules, None)
}

fn fixture_full(
    max_blocks: u32,
    rules: Option<Arc<dyn RuleChecks>>,
    contracts: Option<Arc<dyn ContractGate>>,
) -> Fixture {
    let source = Arc::new(FakeSource::new(max_blocks));
    let checker = Arc::new(FakeChecker::default());
    let (keeper, rx) = GateKeeper::new(source.clone(), checker.clone());
    let keeper = match rules {
        Some(rules) => keeper.with_rules(rules),
        None => keeper,
    };
    let keeper = match contracts {
        Some(c) => keeper.with_contracts(c),
        None => keeper,
    };
    let store = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "demo", "/w".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*store, &session).unwrap();
    let job = JobRepository::insert(
        &*store,
        &NewJob {
            session_id: session.id,
            prompt: "make the answer right\nplease".into(),
            prompt_id: None,
            model: "sonnet".into(),
            priority: 0,
            fork: false,
            wants_review: false,
            review_of: None,
        },
        0,
    )
    .unwrap();
    let ledger = Rc::new(InMemoryLedger::default());
    let clock = Rc::new(FixedClock::new(9_000));
    let service = GateService::new(
        rx,
        Rc::new(LedgerService::new(ledger.clone(), clock.clone())),
        store.clone(),
        clock,
    );
    Fixture {
        keeper: Arc::new(keeper),
        source,
        checker,
        service,
        ledger,
        store,
        job,
    }
}

impl Fixture {
    fn stop(&self, event: HookEvent, message: &str) -> StopAnswer {
        self.keeper.on_stop(&StopContext {
            event,
            job: Some(self.job),
            session: "claude-1".into(),
            cwd: "/w".into(),
            last_message: message.into(),
        })
    }

    fn start(&self) {
        self.keeper.observe(&HookNotice {
            event: HookEvent::SessionStart,
            job: Some(self.job),
            session: "claude-1".into(),
            cwd: "/w".into(),
            tool: None,
            summary: "session startup".into(),
            detail: "startup".into(),
            input_sha256: String::new(),
            input_len: 0,
            raw: String::new(),
        });
    }

    /// The agent edits a file, as its PostToolUse hook reports.
    fn edit(&self) {
        self.keeper.observe(&HookNotice {
            event: HookEvent::PostToolUse,
            job: Some(self.job),
            session: "claude-1".into(),
            cwd: "/w".into(),
            tool: Some("Edit".into()),
            summary: "Edit src/lib.rs".into(),
            detail: String::new(),
            input_sha256: String::new(),
            input_len: 0,
            raw: String::new(),
        });
    }

    fn fix(&self) {
        *lock(&self.checker.fixed) = true;
    }

    fn gate_entries(&self) -> Vec<String> {
        self.ledger
            .entries()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::GateVerdict)
            .map(|e| e.draft.payload_json)
            .collect()
    }

    fn facts(&self) -> Vec<JobFact> {
        let job = JobRepository::get(&*self.store, self.job).unwrap().unwrap();
        vec![JobFact {
            job_id: job.id,
            session_id: job.session_id,
            session_name: "demo".into(),
            prompt: job.prompt.clone(),
            model: job.model.clone(),
            status: job.status,
            created_at: 0,
            started_at: None,
            last_event_at: None,
            finished_at: None,
            session_state: None,
            cost_usd: 0.0,
            error: None,
            result: None,
            review: None,
            review_of: None,
            loop_stops: 0,
            agents_running: 0,
            todos_done: 0,
            todos_total: 0,
            context: None,
            cache_ratio: None,
            flow: Vec::new(),
        }]
    }
}

fn reason(answer: &StopAnswer) -> &str {
    match answer {
        StopAnswer::Block { reason } => reason,
        StopAnswer::Allow => panic!("expected a block"),
    }
}

#[test]
fn a_premature_done_is_blocked_then_accepted_after_the_fix() {
    let mut f = fixture(3);
    f.start();
    f.edit();
    let first = f.stop(HookEvent::Stop, "Done. All tests pass.");
    let digest = reason(&first);
    assert!(
        digest.starts_with("swarmux gate: not done yet (block 1 of 3)"),
        "{digest}"
    );
    assert!(
        digest.contains("✗ cargo test: `cargo test` exited 101"),
        "{digest}"
    );
    assert!(digest.contains("    thread 'answer' panicked at src/lib.rs:3:5:"));
    assert!(digest.contains("⚑ you said \"All tests pass\", but no test run succeeded"));
    assert_eq!(
        f.checker.runs.load(Ordering::SeqCst),
        2,
        "a failure is run twice"
    );

    let (any, errors) = f.service.pump();
    assert!(any && errors.is_empty());
    let state = f.service.state(f.job).unwrap();
    assert_eq!((state.outcome, state.blocks), (GateOutcome::Block, 1));
    assert_eq!(state.checks, [("cargo test".to_string(), false, false)]);
    assert_eq!(state.evidence_gaps, 1);

    // The agent fixes the code and runs the tests itself.
    f.fix();
    f.keeper.executed(f.job, "cargo test", true);
    assert_eq!(
        f.stop(HookEvent::Stop, "Fixed the answer; cargo test passes."),
        StopAnswer::Allow
    );
    f.service.pump();
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.outcome, GateOutcome::Pass);
    assert_eq!(state.checks, [("cargo test".to_string(), true, false)]);

    let entries = f.gate_entries();
    assert_eq!(entries.len(), 2);
    assert!(
        entries[0].contains(r#""outcome":"block""#),
        "{}",
        entries[0]
    );
    assert!(entries[0].contains(r#""exit":101"#));
    assert!(entries[0].contains("panicked at src/lib.rs:3:5"));
    assert!(entries[1].contains(r#""outcome":"pass""#));
    let changes = f.service.changes();
    assert_eq!(changes.len(), 2);
    assert!(
        changes[0]
            .text
            .starts_with("gate blocked a stop (1 of 3): cargo test exited 101")
    );
    assert_eq!(changes[1].kind, ChangeKind::Gate);
}

#[test]
fn claims_need_evidence_even_when_the_checks_pass() {
    let f = fixture(3);
    f.fix();
    let answer = f.stop(HookEvent::Stop, "I ran cargo test and it passed.");
    assert!(reason(&answer).contains("you cite `cargo test`, but it never ran"));
    f.keeper.executed(f.job, "cargo test --quiet", true);
    assert_eq!(
        f.stop(HookEvent::Stop, "I ran cargo test and it passed."),
        StopAnswer::Allow
    );
}

#[test]
fn after_k_blocks_the_gate_lets_go_and_asks_the_user() {
    let mut f = fixture(2);
    f.edit();
    assert!(matches!(
        f.stop(HookEvent::Stop, "done"),
        StopAnswer::Block { .. }
    ));
    assert!(matches!(
        f.stop(HookEvent::Stop, "done"),
        StopAnswer::Block { .. }
    ));
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    f.service.pump();
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.outcome, GateOutcome::Escalate);
    let items = f.service.escalations(&f.facts());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, AttentionKind::GateEscalated);
    assert_eq!(items[0].title, "make the answer right");
    assert!(
        items[0].detail.starts_with("✗ cargo test"),
        "{}",
        items[0].detail
    );
    assert!(f.service.changes()[2].text.starts_with("gate gave up"));
    // A later pass clears it.
    f.fix();
    f.edit();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    f.service.pump();
    assert!(f.service.escalations(&f.facts()).is_empty());
}

#[test]
fn a_flaky_check_passes_on_its_rerun_and_is_marked() {
    let mut f = fixture(3);
    *lock(&f.checker.flaky) = true;
    f.edit();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    f.service.pump();
    assert_eq!(
        f.service.state(f.job).unwrap().checks,
        [("cargo test".to_string(), true, true)]
    );
    assert!(f.gate_entries()[0].contains(r#""flaky":true"#));
}

#[test]
fn a_protected_file_changed_behind_the_guards_back_blocks() {
    let f = fixture(3);
    f.fix();
    f.start();
    *lock(&f.source.pins) = vec![pin("tests/oracles/expected.txt", "41")];
    let answer = f.stop(HookEvent::Stop, "done");
    assert!(
        reason(&answer).contains("⛔ protected file changed: tests/oracles/expected.txt"),
        "{answer:?}"
    );
    *lock(&f.source.pins) = vec![pin("tests/oracles/expected.txt", "42")];
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
}

#[test]
fn subagents_skip_the_checks_unless_asked() {
    let f = fixture(3);
    assert_eq!(
        f.stop(HookEvent::SubagentStop, "explored"),
        StopAnswer::Allow
    );
    assert_eq!(f.checker.runs.load(Ordering::SeqCst), 0);
    // Their claims still need evidence.
    assert!(matches!(
        f.stop(HookEvent::SubagentStop, "All tests pass."),
        StopAnswer::Block { .. }
    ));
    lock(&f.source.spec).subagent_checks = true;
    f.edit();
    assert!(matches!(
        f.stop(HookEvent::SubagentStop, "explored"),
        StopAnswer::Block { .. }
    ));
}

#[test]
fn nothing_to_check_is_not_recorded() {
    let mut f = fixture(3);
    f.edit();
    lock(&f.source.spec).checks.clear();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    assert!(!f.service.pump().0);
    assert!(f.gate_entries().is_empty());
}

#[test]
fn the_fence_denies_protected_writes_through_the_policy() {
    let f = fixture(3);
    let policy = SharedPolicy::new(GuardPolicy::new(vec![], vec![], Unmatched::Pass))
        .with_fence(f.keeper.clone());
    let decide = |tool: &str, subject: &str| {
        policy
            .decide(&HookRequest {
                request_id: 1,
                job_id: Some(f.job),
                tool_name: tool.into(),
                subject: subject.into(),
                input: String::new(),
                cwd: "/w".into(),
                content: String::new(),
                ..HookRequest::default()
            })
            .0
    };
    let edit = decide("Edit", "/w/tests/oracles/expected.txt");
    assert_eq!(edit.verdict, Verdict::Deny);
    assert_eq!(
        edit.rule.as_deref(),
        Some("protected path tests/oracles/** (gate)")
    );
    let sh = decide("Bash", "echo 41 > tests/oracles/expected.txt");
    assert_eq!(sh.verdict, Verdict::Deny);
    assert_eq!(
        decide("Edit", "/w/.swarmux/gates.toml").verdict,
        Verdict::Deny
    );
    assert_eq!(decide("Edit", "/w/src/lib.rs").verdict, Verdict::Pass);
    assert_eq!(
        decide("Read", "/w/tests/oracles/expected.txt").verdict,
        Verdict::Allow
    );
    assert_eq!(decide("Bash", "cargo test").verdict, Verdict::Pass);
}

#[test]
fn run_checks_reruns_only_plain_failures() {
    struct Slow(AtomicU32);
    impl Checker for Slow {
        fn run(&self, check: &GateCheck, _: &Path) -> CheckRun {
            self.0.fetch_add(1, Ordering::SeqCst);
            CheckRun {
                name: check.name.clone(),
                command: check.command_line(),
                exit: None,
                timed_out: true,
                spawn_error: None,
                duration_ms: 1,
                output: String::new(),
                flaky: false,
            }
        }
    }
    let slow = Slow(AtomicU32::new(0));
    let runs = run_checks(&slow, &[GateCheck::new("t", "x", &[])], Path::new("/w"));
    assert!(runs[0].timed_out);
    assert_eq!(
        slow.0.load(Ordering::SeqCst),
        1,
        "a timeout is not run again"
    );
}

#[test]
fn checks_run_only_after_something_could_change_files() {
    let mut f = fixture(3);
    // A read-only session: nothing to judge.
    assert_eq!(
        f.stop(HookEvent::Stop, "The parser lives in src/parse.rs."),
        StopAnswer::Allow
    );
    assert_eq!(f.checker.runs.load(Ordering::SeqCst), 0);
    assert!(!f.service.pump().0);
    // A blocked stop is checked again even if nothing changed since.
    f.edit();
    assert!(matches!(
        f.stop(HookEvent::Stop, "done"),
        StopAnswer::Block { .. }
    ));
    let runs = f.checker.runs.load(Ordering::SeqCst);
    assert!(matches!(
        f.stop(HookEvent::Stop, "done"),
        StopAnswer::Block { .. }
    ));
    assert!(f.checker.runs.load(Ordering::SeqCst) > runs);
    // After a pass, the next stop with no change runs nothing.
    f.fix();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    let runs = f.checker.runs.load(Ordering::SeqCst);
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    assert_eq!(f.checker.runs.load(Ordering::SeqCst), runs);
    // A shell command reported from the stream counts as a change.
    f.keeper.executed(f.job, "touch x", true);
    f.stop(HookEvent::Stop, "done");
    assert!(f.checker.runs.load(Ordering::SeqCst) > runs);
}

fn record(job: JobId, event: Option<HookEvent>, outcome: GateOutcome) -> GateRecord {
    GateRecord {
        job: Some(job),
        session: "claude-9".into(),
        event,
        root: "/repo".into(),
        origin: GateOrigin::File,
        outcome,
        blocks: 0,
        max_blocks: 3,
        findings: GateFindings::default(),
        digest: "all checks passed".into(),
        problem: None,
    }
}

#[test]
fn a_passed_stop_takes_a_checkpoint_and_nothing_else_does() {
    let rig = crate::testing::WorktreeRig::new();
    let job = rig.job("make it pass");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut service = GateService::new(
        rx,
        rig.ledger_service.clone(),
        rig.store.clone(),
        rig.clock.clone(),
    )
    .with_checkpoints(rig.checkpoints.clone());

    for (event, outcome) in [
        (Some(HookEvent::Stop), GateOutcome::Block),
        (Some(HookEvent::Stop), GateOutcome::Escalate),
        (Some(HookEvent::SubagentStop), GateOutcome::Pass),
        (None, GateOutcome::Pass),
    ] {
        tx.send(record(job, event, outcome)).unwrap();
    }
    let (_, errors) = service.pump();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(rig.checkpoints.list(job).unwrap().is_empty());

    tx.send(record(job, Some(HookEvent::Stop), GateOutcome::Pass))
        .unwrap();
    let (_, errors) = service.pump();
    assert!(errors.is_empty(), "{errors:?}");
    let all = rig.checkpoints.list(job).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].reason, swarmux_domain::CheckpointReason::GatePass);
    assert_eq!(all[0].session_id, "claude-9");
    // The checkpoint is recorded after the verdict it follows.
    let kinds: Vec<LedgerKind> = rig
        .ledger
        .entries()
        .iter()
        .map(|e| e.draft.kind)
        .filter(|k| matches!(k, LedgerKind::GateVerdict | LedgerKind::Checkpoint))
        .collect();
    assert_eq!(kinds.last(), Some(&LedgerKind::Checkpoint));
    assert_eq!(kinds[kinds.len() - 2], LedgerKind::GateVerdict);
}

fn mission_gate() -> MissionGate {
    MissionGate {
        checks: vec![
            GateCheck::new("cargo test", "cargo", &["test"]),
            GateCheck::new("bench", "sh", &["scripts/bench.sh"]),
        ],
        protected: vec!["scripts/bench.sh".into()],
    }
}

#[test]
fn a_job_given_a_mission_gate_is_judged_and_fenced_with_it() {
    let mut f = fixture(3);
    f.keeper.assign(f.job, mission_gate());
    f.start();
    f.edit();
    f.fix();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    f.service.pump();
    let state = f.service.state(f.job).unwrap();
    let names: Vec<&str> = state.checks.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(
        names,
        ["cargo test", "bench"],
        "the repository's check is not repeated"
    );

    let refuses = |job: Option<JobId>| {
        f.keeper
            .refuses(job, "Edit", "/w/scripts/bench.sh", "/w")
            .map(|r| r.to_string())
    };
    assert_eq!(
        refuses(Some(f.job)).as_deref(),
        Some("protected path scripts/bench.sh (gate)")
    );
    assert_eq!(refuses(Some(JobId(99))), None, "other jobs are not fenced");
    assert_eq!(refuses(None), None);
}

/// Waits for the record a check run on another thread sends.
fn next_record(f: &mut Fixture) {
    for _ in 0..500 {
        if f.service.pump().0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("no verdict arrived");
}

#[test]
fn verify_runs_the_checks_with_no_agent_waiting() {
    let mut f = fixture(3);
    f.keeper.assign(f.job, mission_gate());
    f.keeper.verify(f.job, Path::new("/w")).unwrap();
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap().clone();
    assert_eq!(
        state.outcome,
        GateOutcome::Escalate,
        "failing checks go to the user"
    );
    assert_eq!(state.blocks, 0);
    f.fix();
    // This fixture has no clean copies: the checks run in the worktree,
    // which a mission only accepts when the repository says so.
    lock(&f.source.spec).verifier.worktree_checks = WorktreeChecks::Allow;
    f.keeper.verify(f.job, Path::new("/w")).unwrap();
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.outcome, GateOutcome::Pass);
    assert_eq!(state.checks.len(), 2);
    assert_eq!(f.gate_entries().len(), 2, "both runs are in the ledger");
}

#[test]
fn a_mission_whose_checks_ran_in_the_worktree_is_escalated_not_passed() {
    let mut f = fixture(3);
    f.keeper.assign(f.job, mission_gate());
    f.fix();
    f.keeper.verify(f.job, Path::new("/w")).unwrap();
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap().clone();
    assert!(state.checks.iter().all(|c| c.1), "every check passed");
    assert_eq!(
        state.outcome,
        GateOutcome::Escalate,
        "but not in a clean copy"
    );
    assert_eq!(state.notes.ran_in, Some(RanIn::Worktree));
    assert_eq!(
        state.notes.incomplete,
        ["the checks ran in the worktree, not in a clean copy of the work"]
    );
    assert!(state.digest.contains("cannot vouch"), "{}", state.digest);
    let entry = f.gate_entries().pop().unwrap();
    assert!(entry.contains("\"ran_in\":\"worktree\""), "{entry}");
    assert!(entry.contains("\"outcome\":\"escalate\""), "{entry}");
    assert!(entry.contains("\"policy_sha256\":\""), "{entry}");
}

#[test]
fn verify_with_nothing_to_run_does_not_pass() {
    let mut f = fixture(3);
    lock(&f.source.spec).checks.clear();
    f.keeper.verify(f.job, Path::new("/w")).unwrap();
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.outcome, GateOutcome::Escalate);
    assert_eq!(state.digest, "swarmux gate: nothing to check");
}

/// One hidden gate rule and one shadow lint rule; records what was reported.
#[derive(Default)]
struct FakeRules {
    reports: Mutex<Vec<(String, bool, bool)>>,
}

impl RuleChecks for FakeRules {
    fn checks(&self, _job: JobId, mission: bool) -> Vec<RuleCheck> {
        let id = |s: &str| swarmux_domain::knowledge::ItemId::parse(s).unwrap();
        let mut out = vec![RuleCheck {
            rule: id("R2"),
            check: GateCheck::new("rule R2: lint", "lint", &[]),
            hidden: false,
            shadow: true,
        }];
        if mission {
            out.push(RuleCheck {
                rule: id("R1"),
                check: GateCheck::new("rule R1", "psql", &["-f", "anon.sql"]),
                hidden: true,
                shadow: false,
            });
        }
        out
    }

    fn report(
        &self,
        _job: JobId,
        rule: &swarmux_domain::knowledge::ItemId,
        passed: bool,
        shadow: bool,
    ) {
        lock(&self.reports).push((rule.to_string(), passed, shadow));
    }
}

#[test]
fn a_failing_gate_rule_blocks_a_mission_without_naming_its_check() {
    let rules = Arc::new(FakeRules::default());
    let f = fixture_with(3, Some(rules.clone()));
    f.start();
    f.edit();
    // Not a mission's job: only the shadow lint runs, and passes nothing.
    f.fix();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);
    assert_eq!(lock(&rules.reports).len(), 1);
    assert!(lock(&rules.reports)[0].2, "shadow");

    // A mission's job: the hidden rule check runs after the visible ones.
    f.keeper.assign(f.job, MissionGate::default());
    f.edit();
    let answer = f.stop(HookEvent::Stop, "done");
    let StopAnswer::Block { reason } = answer else {
        panic!("{answer:?}")
    };
    assert!(reason.contains("hidden"), "{reason}");
    assert!(
        !reason.contains("psql") && !reason.contains("anon.sql"),
        "{reason}"
    );
    assert!(
        lock(&rules.reports)
            .iter()
            .any(|(r, passed, shadow)| r == "R1" && !passed && !shadow)
    );
}

/// Judges every change as breaking `app/lib/orders.dart:42`.
struct BreaksApp;

impl ContractGate for BreaksApp {
    fn judge(&self, _job: JobId) -> Option<ContractJudgement> {
        Some(ContractJudgement {
            checks: vec![GateCheck::new("app: flutter analyze", "flutter", &["analyze"]).in_dir("app")],
            run: CheckRun {
                name: "contracts".into(),
                command: "swarmux contracts check".into(),
                exit: Some(1),
                timed_out: false,
                spawn_error: None,
                duration_ms: 3,
                output: "BROKEN: table orders is gone but still used by: app/lib/orders.dart:42, admin/src/api.ts:17".into(),
                flaky: false,
            },
        })
    }
}

#[test]
fn a_mission_that_breaks_a_consumer_is_blocked_with_its_places() {
    let f = fixture_full(3, None, Some(Arc::new(BreaksApp)));
    f.start();
    f.fix();
    // Not a mission's job: the contracts are not its gate's business.
    f.edit();
    assert_eq!(f.stop(HookEvent::Stop, "done"), StopAnswer::Allow);

    f.keeper.assign(f.job, MissionGate::default());
    f.edit();
    let answer = f.stop(HookEvent::Stop, "done");
    let StopAnswer::Block { reason } = answer else {
        panic!("{answer:?}")
    };
    assert!(reason.contains("app/lib/orders.dart:42"), "{reason}");
    assert!(reason.contains("admin/src/api.ts:17"), "{reason}");
    assert!(
        lock(&f.checker.ran)
            .iter()
            .any(|n| n == "app: flutter analyze"),
        "the consumer's check ran too"
    );
}

// --- X2: flake history, plain jobs that end, what the ledger holds ----------

/// A verdict whose one check, `unit`, failed and passed when run again.
fn flaky_record(job: JobId, outcome: GateOutcome) -> GateRecord {
    let mut r = record(job, Some(HookEvent::Stop), outcome);
    r.root = "/w".into();
    r.findings = GateFindings {
        runs: vec![CheckRun {
            name: "unit".into(),
            command: "cargo test".into(),
            exit: Some(0),
            timed_out: false,
            spawn_error: None,
            duration_ms: 10,
            output: String::new(),
            flaky: true,
        }],
        flake_policy: (3, 10),
        ran_in: Some(RanIn::CleanCopy),
        ..GateFindings::default()
    };
    r
}

#[test]
fn a_check_that_flakes_three_times_in_ten_is_called_flaky_and_the_history_outlives_a_restart() {
    let mut f = fixture(3);
    for n in 1..=2 {
        f.keeper.send(flaky_record(f.job, GateOutcome::Pass));
        next_record(&mut f);
        let state = f.service.state(f.job).unwrap();
        assert!(state.notes.flaky.is_empty(), "{n} is not enough");
    }
    f.keeper.send(flaky_record(f.job, GateOutcome::Pass));
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.notes.flaky, [("unit".to_string(), 3, 3)]);
    assert_eq!(f.service.detail(f.job).unwrap().flaky, state.notes.flaky);

    // A new run of the app: the ledger knows what happened before it.
    let (tx, rx) = std::sync::mpsc::channel();
    let clock = Rc::new(FixedClock::new(10_000));
    let mut again = GateService::new(
        rx,
        Rc::new(LedgerService::new(f.ledger.clone(), clock.clone())),
        f.store.clone(),
        clock,
    );
    tx.send(flaky_record(f.job, GateOutcome::Pass)).unwrap();
    let (_, errors) = again.pump();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        again.state(f.job).unwrap().notes.flaky,
        [("unit".to_string(), 4, 4)],
        "seeded from the ledger"
    );
}

#[test]
fn flaky_tracking_can_be_turned_off_and_never_names_a_check_that_did_not_flake() {
    let mut f = fixture(3);
    let mut off = flaky_record(f.job, GateOutcome::Pass);
    off.findings.flake_policy = (0, 0);
    for _ in 0..5 {
        f.keeper.send(off.clone());
        next_record(&mut f);
    }
    assert!(f.service.state(f.job).unwrap().notes.flaky.is_empty());
    // The ledger remembers the flakes of `unit` even with tracking off;
    // another check that never flaked is not named.
    let mut steady = flaky_record(f.job, GateOutcome::Pass);
    steady.findings.runs[0].name = "steady".into();
    steady.findings.runs[0].flaky = false;
    for _ in 0..5 {
        f.keeper.send(steady.clone());
        next_record(&mut f);
    }
    assert!(f.service.state(f.job).unwrap().notes.flaky.is_empty());
}

#[test]
fn the_inbox_item_of_a_gave_up_job_says_which_check_is_flaky_and_a_harness_change_is_p1() {
    let mut f = fixture(3);
    for _ in 0..3 {
        f.keeper.send(flaky_record(f.job, GateOutcome::Pass));
        next_record(&mut f);
    }
    let mut gave_up = flaky_record(f.job, GateOutcome::Escalate);
    gave_up.digest = "swarmux gate: still failing after 3 blocks.\n✗ unit failed".into();
    f.keeper.send(gave_up);
    next_record(&mut f);
    let items = f.service.escalations(&f.facts());
    assert_eq!(items.len(), 1);
    assert!(
        items[0]
            .detail
            .contains("unit is flaky (4 of its last 4 runs)"),
        "{}",
        items[0].detail
    );
    assert_eq!(items[0].priority, Priority::P2);

    let mut harness = flaky_record(f.job, GateOutcome::Escalate);
    harness.findings.substrate = vec!["conftest.py: harness file created".into()];
    f.keeper.send(harness);
    next_record(&mut f);
    let items = f.service.escalations(&f.facts());
    assert_eq!(
        items[0].priority,
        Priority::P1,
        "the harness is the referee's"
    );
}

#[test]
fn what_the_verdict_could_not_say_is_in_the_state_the_detail_and_the_ledger() {
    let mut f = fixture(3);
    let mut r = record(f.job, Some(HookEvent::Stop), GateOutcome::Escalate);
    r.findings = GateFindings {
        hidden_not_run: Some(HiddenNotRun {
            planned: 2,
            reason: "no clean copy was made (git broke)".into(),
        }),
        ran_in: Some(RanIn::Worktree),
        substrate: vec!["Makefile: harness file changed (make runs what it says)".into()],
        dependencies: vec!["Cargo.lock: leftpad".into()],
        planned: vec!["unit".into(), "soak".into()],
        left_out: vec![("soak".into(), "level deep is above the gate's full".into())],
        revert_probe: Some(RevertProbe {
            proved: false,
            detail: "every check still passes".into(),
        }),
        policy_sha256: "ab".repeat(32),
        unattended: true,
        warnings: vec!["verifier dependencies is one of flag, block, not \"x\"".into()],
        ..GateFindings::default()
    };
    f.keeper.send(r);
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap();
    assert_eq!(
        state.notes.hidden_not_run.as_deref(),
        Some("hidden checks: not run (no clean copy was made (git broke))")
    );
    assert_eq!(state.notes.ran_in, Some(RanIn::Worktree));
    assert_eq!((state.notes.substrate, state.notes.dependencies), (1, 1));
    assert_eq!(
        state.notes.incomplete.len(),
        2,
        "{:?}",
        state.notes.incomplete
    );
    let detail = f.service.detail(f.job).unwrap();
    assert_eq!(detail.planned, ["unit", "soak"]);
    assert_eq!(detail.left_out.len(), 1);
    assert_eq!(
        detail.revert_probe,
        Some((false, "every check still passes".into()))
    );
    assert_eq!(detail.policy_sha256.len(), 64);

    let entry = f.gate_entries().pop().unwrap();
    for says in [
        "\"substrate\":[\"Makefile: harness file changed",
        "\"dependencies\":[\"Cargo.lock: leftpad\"]",
        "\"ran_in\":\"worktree\"",
        "\"planned\":[\"unit\",\"soak\"]",
        "\"left_out\":[{\"name\":\"soak\"",
        "\"hidden_not_run\":{\"planned\":2,\"reason\":\"no clean copy was made (git broke)\"}",
        "\"revert_probe\":{\"proved\":false",
        "\"unattended\":true",
        "\"policy_sha256\":\"abab",
        "\"warnings\":[\"verifier dependencies is one of flag, block",
        "\"incomplete\":[\"hidden checks: not run",
    ] {
        assert!(entry.contains(says), "{says} in {entry}");
    }
    // A record the bundle can be sealed from reads all of it back.
    let facts = swarmux_domain::evidence::parse_verdict(&entry).unwrap();
    assert_eq!(facts.how.ran_in, "worktree");
    assert_eq!(facts.how.hidden_not_run.unwrap().0, 2);
    assert_eq!(facts.how.substrate.len(), 1);
    assert_eq!(facts.how.policy_sha256, "ab".repeat(32));
}

#[test]
fn a_plain_job_that_ends_is_judged_again_unless_its_last_stop_passed_on_the_final_tree() {
    let mut f = fixture(3);
    // It never opened a session in a repository: nothing to judge.
    assert_eq!(f.keeper.finish(f.job), Ok(false));
    f.start();
    f.edit();
    f.fix();
    // It changed files and never stopped: judged now, attended, so a run in
    // the worktree is a note, not an escalation.
    assert_eq!(f.keeper.finish(f.job), Ok(true));
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap().clone();
    assert_eq!(state.outcome, GateOutcome::Pass, "{}", state.digest);
    assert_eq!(state.notes.ran_in, Some(RanIn::Worktree));
    assert_eq!(f.stop(HookEvent::Stop, "Done."), StopAnswer::Allow);
    next_record(&mut f);

    // Its last stop passed and nothing changed since: not judged again.
    assert_eq!(f.keeper.finish(f.job), Ok(false));
    // It changed a file after the pass: judged again.
    f.edit();
    assert_eq!(f.keeper.finish(f.job), Ok(true));
    next_record(&mut f);

    // A mission has its own verification.
    f.keeper.assign(f.job, mission_gate());
    f.edit();
    assert_eq!(f.keeper.finish(f.job), Ok(false));
}

#[test]
fn a_plain_job_in_a_repository_with_nothing_to_check_is_not_judged_at_all() {
    let f = fixture(3);
    lock(&f.source.spec).checks.clear();
    f.start();
    f.edit();
    assert_eq!(f.keeper.finish(f.job), Ok(false));
}

/// Says a job that ended could not be verified.
struct Refuses;

impl JobGates for Refuses {
    fn assign(&self, _job: JobId, _gate: MissionGate) {}

    fn verify(&self, _job: JobId, _dir: &Path) -> Result<(), String> {
        Ok(())
    }

    fn finish(&self, _job: JobId) -> Result<bool, String> {
        Err("no clean copy: git is gone".into())
    }
}

#[test]
fn a_job_that_ended_and_could_not_be_verified_shows_as_unverified_with_why() {
    let mut f = fixture(3);
    f.service = {
        let (_tx, rx) = std::sync::mpsc::channel();
        let clock = Rc::new(FixedClock::new(9_000));
        GateService::new(
            rx,
            Rc::new(LedgerService::new(f.ledger.clone(), clock.clone())),
            f.store.clone(),
            clock,
        )
        .with_finisher(Rc::new(Refuses))
    };
    let mut running = f.facts();
    running[0].status = swarmux_domain::JobStatus::Running;
    assert!(f.service.pump_finished(&running).is_empty());
    assert!(
        f.service.state(f.job).is_none(),
        "still running: nothing yet"
    );
    let mut done = f.facts();
    done[0].status = swarmux_domain::JobStatus::Done;
    let errors = f.service.pump_finished(&done);
    assert!(errors.is_empty(), "{errors:?}");
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.outcome, GateOutcome::Escalate);
    assert!(
        state
            .digest
            .contains("unverified: no clean copy: git is gone"),
        "{}",
        state.digest
    );
    assert_eq!(
        state.notes.incomplete,
        ["unverified: no clean copy: git is gone"]
    );
    // Asked once: the job is no longer running.
    f.service.pump_finished(&done);
    assert_eq!(f.gate_entries().len(), 1);
    // And a job that was already finished when swarmux started is history.
    let (_tx2, rx2) = std::sync::mpsc::channel();
    let clock = Rc::new(FixedClock::new(9_000));
    let mut fresh = GateService::new(
        rx2,
        Rc::new(LedgerService::new(f.ledger.clone(), clock.clone())),
        f.store.clone(),
        clock,
    )
    .with_finisher(Rc::new(Refuses));
    assert!(fresh.pump_finished(&done).is_empty());
    assert!(fresh.state(f.job).is_none());
}

struct Grants(Mutex<Vec<swarmux_domain::trust::TrustEvent>>);

impl crate::trust::TrustReader for Grants {
    fn events(&self) -> Vec<swarmux_domain::trust::TrustEvent> {
        lock(&self.0).clone()
    }
}

#[test]
fn a_stop_in_a_repository_nobody_trusted_runs_no_check_and_escalates_until_it_is_granted() {
    use swarmux_domain::trust::{TrustEvent, TrustRecord};
    let source = Arc::new(FakeSource::new(2));
    *lock(&source.shipped) = true;
    let checker = Arc::new(FakeChecker::default());
    let grants = Arc::new(Grants(Mutex::new(Vec::new())));
    let (keeper, rx) = GateKeeper::new(source.clone(), checker.clone());
    let keeper = Arc::new(keeper.with_trust(Arc::new(crate::trust::TrustGuard::new(
        grants.clone(),
        FixedClock::new(1).reader(),
    ))));
    let notice = |event, tool: Option<&str>| HookNotice {
        event,
        job: Some(JobId(1)),
        session: "claude-1".into(),
        cwd: "/w".into(),
        tool: tool.map(str::to_string),
        summary: "x".into(),
        detail: String::new(),
        input_sha256: String::new(),
        input_len: 0,
        raw: String::new(),
    };
    let stop = |message: &str| {
        keeper.on_stop(&StopContext {
            event: HookEvent::Stop,
            job: Some(JobId(1)),
            session: "claude-1".into(),
            cwd: "/w".into(),
            last_message: message.into(),
        })
    };
    keeper.observe(&notice(HookEvent::SessionStart, None));
    keeper.observe(&notice(HookEvent::PostToolUse, Some("Edit")));
    stop("done");
    assert_eq!(
        checker.runs.load(Ordering::SeqCst),
        0,
        "no command of the repository ran"
    );
    let record = rx.try_recv().expect("the gate still says something");
    assert_ne!(record.outcome, GateOutcome::Pass);
    assert!(
        record
            .problem
            .as_deref()
            .is_some_and(|p| p.contains("not trusted")),
        "{:?}",
        record.problem
    );

    // The owner grants exactly these files: the next stop runs the check.
    let supplied = source.supplied(&source.load(Path::new("/w")));
    lock(&grants.0).push(TrustEvent::Granted(TrustRecord {
        repo_id: supplied.repo_id,
        root: supplied.root,
        entries: supplied.entries,
        at: 2,
    }));
    keeper.observe(&notice(HookEvent::PostToolUse, Some("Edit")));
    stop("done");
    // A failing check is run once more by the gate itself.
    assert!(checker.runs.load(Ordering::SeqCst) >= 1, "now it runs");
}
