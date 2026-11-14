//! I1 end to end against the fake `claude`: a message sent while a turn
//! runs is not written into it. A queued one reaches the agent when the
//! turn has ended, as the next prompt of the same session; a steered one
//! stops the turn at once and reaches the agent as the prompt of the
//! session's next start. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use swarmux_application::ledger::{LedgerService, LedgeredJobs, verify_all};
use swarmux_application::ports::{
    DecisionRepository, HookInbox, InboxItem, JobRepository, SessionRepository,
};
use swarmux_application::{
    AppResult, DecisionRecord, Engine, GuardService, JobService, SharedPolicy,
};
use swarmux_domain::intervene::{Mode, State};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{Decision, GuardPolicy, JobId, JobStatus, Session, SessionId, Unmatched};
use swarmux_infrastructure::{ClaudeCli, SqliteLedger, SqliteStore, SystemClock};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

struct NoInbox;
impl HookInbox for NoInbox {
    fn poll(&self) -> Vec<InboxItem> {
        Vec::new()
    }
    fn respond(&self, _: u64, _: &Decision) {}
}

struct NoDecisions;
impl DecisionRepository for NoDecisions {
    fn record(&self, _: &DecisionRecord) -> AppResult<()> {
        Ok(())
    }
    fn recent_decisions(&self, _: usize) -> AppResult<Vec<DecisionRecord>> {
        Ok(Vec::new())
    }
}

struct Rig {
    engine: Engine,
    store: Rc<SqliteStore>,
    ledger: Rc<SqliteLedger>,
    session: Session,
    stdin_file: PathBuf,
    args_file: PathBuf,
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

/// A real job service over the fake `claude`, which plays the fixture one
/// line each `line_ms` and then idles `sleep_ms` before it exits.
fn rig(line_ms: u64, sleep_ms: u64) -> Rig {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swarmux.db");
    let store = Rc::new(SqliteStore::open(&db).unwrap());
    let ledger = Rc::new(SqliteLedger::open(&db).unwrap());
    let clock = Rc::new(SystemClock);
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let jobs_log = Rc::new(LedgeredJobs::new(store.clone(), service.clone()));
    let stdin_file = dir.path().join("stdin.log");
    let args_file = dir.path().join("args.log");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream_basic.jsonl");
    let cli = ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(vec![
        (
            "FAKE_CLAUDE_FIXTURE".into(),
            fixture.to_string_lossy().into_owned(),
        ),
        ("FAKE_CLAUDE_LINE_DELAY_MS".into(), line_ms.to_string()),
        ("FAKE_CLAUDE_SLEEP_MS".into(), sleep_ms.to_string()),
        (
            "FAKE_CLAUDE_STDIN_FILE".into(),
            stdin_file.to_string_lossy().into_owned(),
        ),
        (
            "FAKE_CLAUDE_ARGS_FILE".into(),
            args_file.to_string_lossy().into_owned(),
        ),
    ]);
    let session = Session::new(SessionId::random(), "s", dir.path().into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*store, &session).unwrap();
    let jobs = JobService::new(store.clone(), jobs_log, Rc::new(cli), clock.clone());
    let guard = GuardService::new(
        SharedPolicy::new(GuardPolicy::new(Vec::new(), Vec::new(), Unmatched::Ask)),
        Rc::new(NoInbox),
        Rc::new(NoDecisions),
        clock,
    );
    let mut engine = Engine::new(jobs, guard);
    engine.ledger = Some(service);
    Rig {
        engine,
        store,
        ledger,
        session,
        stdin_file,
        args_file,
        _rt: rt,
        _dir: dir,
    }
}

impl Rig {
    /// What each start of `claude` was told, oldest first.
    fn told(&self) -> Vec<String> {
        std::fs::read_to_string(&self.stdin_file)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn started_with(&self) -> Vec<String> {
        std::fs::read_to_string(&self.args_file)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// One frame: what the app does every update.
    fn frame(&mut self) {
        let report = self.engine.pump();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let facts = self.engine.jobs.facts();
        let missions = self.engine.pump_missions(&facts, &HashMap::new());
        assert!(missions.errors.is_empty(), "{:?}", missions.errors);
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Rig) -> bool) {
        let end = Instant::now() + Duration::from_secs(30);
        while !done(self) {
            assert!(Instant::now() < end, "timed out waiting for: {what}");
            self.frame();
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn running_and_reported(&mut self) -> JobId {
        let job = self.engine.jobs.dispatch(self.session.id, "first").unwrap();
        self.until("the agent reports", |r| r.engine.jobs.has_reported(job));
        job
    }
}

#[test]
fn a_queued_message_is_not_sent_mid_turn_and_reaches_the_agent_when_the_turn_ends() {
    // The fixture plays for about a second; the process then ends.
    let mut r = rig(150, 0);
    let job = r.running_and_reported();
    assert!(
        !r.store.status(job).unwrap().is_finished(),
        "still mid-turn"
    );

    let receipt = r
        .engine
        .intervene(job, "use the staging bucket", Mode::Queue, false);
    assert_eq!(receipt.state, State::Queued);
    // Mid-turn: the agent was told its first prompt and nothing else.
    for _ in 0..5 {
        r.frame();
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(!r.store.status(job).unwrap().is_finished());
    assert_eq!(r.told().len(), 1, "{:?}", r.told());
    assert!(!r.told()[0].contains("staging"));

    // The turn ends, and only then does the next start carry the message.
    let id = receipt.id;
    r.until("the message is delivered", |r| {
        matches!(
            r.engine.interventions.get(id).map(|x| x.state.clone()),
            Some(State::Delivered { .. })
        )
    });
    assert_eq!(
        r.store.status(job).unwrap(),
        JobStatus::Done,
        "the first turn finished on its own"
    );
    let told = r.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[1].contains("use the staging bucket"), "{}", told[1]);
    assert!(told[1].contains("while you were working"), "{}", told[1]);
    // The same Claude session, resumed.
    let starts = r.started_with();
    assert!(starts[0].contains("--session-id"), "{starts:?}");
    assert!(starts[1].contains("--resume"), "{starts:?}");
    let session = r.session.id.to_string();
    assert!(starts[0].contains(&session) && starts[1].contains(&session));

    // The ledger holds the message and its delivery, and still verifies.
    let entries = swarmux_application::ledger::Ledger::range(&*r.ledger, 1, 10_000).unwrap();
    let operator: Vec<_> = entries
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::Operator)
        .collect();
    assert_eq!(operator.len(), 2);
    assert!(
        operator[0]
            .draft
            .payload_json
            .contains(r#""outcome":"queued""#)
    );
    assert!(
        operator[0]
            .draft
            .payload_json
            .contains("use the staging bucket")
    );
    assert!(
        operator[1]
            .draft
            .payload_json
            .contains(r#""outcome":"delivered""#)
    );
    assert!(verify_all(&*r.ledger).unwrap().is_ok());
}

#[test]
fn a_steered_message_stops_the_turn_at_once_and_starts_the_next_with_it() {
    // The turn would play for about a second and then idle for ten more:
    // only a stop gets the message there before that.
    let mut r = rig(150, 10_000);
    let job = r.running_and_reported();
    let at = Instant::now();

    let receipt = r
        .engine
        .intervene(job, "wrong table: use orders_v2", Mode::Steer, true);
    assert!(
        matches!(receipt.state, State::Handed { .. }),
        "{:?}",
        receipt.state
    );
    assert_eq!(r.store.status(job).unwrap(), JobStatus::Cancelled);

    let id = receipt.id;
    r.until("the message is delivered", |r| {
        matches!(
            r.engine.interventions.get(id).map(|x| x.state.clone()),
            Some(State::Delivered { .. })
        )
    });
    assert!(
        at.elapsed() < Duration::from_secs(8),
        "the next start did not wait for the old turn to idle out"
    );
    let told = r.told();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(
        told[1].contains("wrong table: use orders_v2"),
        "{}",
        told[1]
    );
    assert!(told[1].contains("stopped your turn"), "{}", told[1]);
    assert!(r.started_with()[1].contains("--resume"));
    assert!(verify_all(&*r.ledger).unwrap().is_ok());
    // Stop the second process too, so the test does not wait for it.
    let _ = r
        .engine
        .jobs
        .cancel(match r.engine.interventions.get(id).unwrap().state {
            State::Delivered { job, .. } => job,
            _ => job,
        });
}
