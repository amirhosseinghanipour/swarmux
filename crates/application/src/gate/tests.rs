use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::SharedPolicy;
use crate::ports::{HookRequest, NewJob, SessionRepository};
use crate::testing::{FixedClock, InMemoryLedger, InMemoryStore};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{AttentionKind, GuardPolicy, Session, Unmatched, Verdict};

/// One repository at `/w` with a `cargo test` check and protected oracles.
struct FakeSource {
    spec: Mutex<GateSpec>,
    pins: Mutex<Vec<Pin>>,
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
}

/// Fails with an assertion until fixed; may fail only every other run.
#[derive(Default)]
struct FakeChecker {
    fixed: Mutex<bool>,
    flaky: Mutex<bool>,
    runs: AtomicU32,
}

impl Checker for FakeChecker {
    fn run(&self, check: &GateCheck, _root: &Path) -> CheckRun {
        let n = self.runs.fetch_add(1, Ordering::SeqCst);
        let pass = *lock(&self.fixed) || (*lock(&self.flaky) && n % 2 == 1);
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
    let source = Arc::new(FakeSource::new(max_blocks));
    let checker = Arc::new(FakeChecker::default());
    let (keeper, rx) = GateKeeper::new(source.clone(), checker.clone());
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
    f.keeper.verify(f.job, Path::new("/w")).unwrap();
    next_record(&mut f);
    let state = f.service.state(f.job).unwrap();
    assert_eq!(state.outcome, GateOutcome::Pass);
    assert_eq!(state.checks.len(), 2);
    assert_eq!(f.gate_entries().len(), 2, "both runs are in the ledger");
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
