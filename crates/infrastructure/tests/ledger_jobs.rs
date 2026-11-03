//! A job run through the fake `claude` lands in the SQLite ledger, every
//! stored event included. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use swarmux_application::JobService;
use swarmux_application::ledger::{Ledger, LedgerService, LedgeredJobs, verify_all};
use swarmux_application::ports::{JobRepository, SessionRepository};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{JobStatus, Session, SessionId};
use swarmux_infrastructure::{ClaudeCli, SqliteLedger, SqliteStore, SystemClock};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

#[test]
fn job_events_are_mirrored_into_the_ledger() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swarmux.db");
    let store = Rc::new(SqliteStore::open(&db).unwrap());
    let ledger = Rc::new(SqliteLedger::open(&db).unwrap());
    let clock = Rc::new(SystemClock);
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let jobs = Rc::new(LedgeredJobs::new(store.clone(), service));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream_basic.jsonl");
    let cli = ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(vec![(
        "FAKE_CLAUDE_FIXTURE".into(),
        fixture.to_string_lossy().into_owned(),
    )]);
    let session = Session::new(SessionId::random(), "s", dir.path().into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*store, &session).unwrap();
    let mut svc = JobService::new(store.clone(), jobs, Rc::new(cli), clock);

    let job = svc.dispatch(session.id, "hello").unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    while !store.status(job).unwrap().is_finished() && Instant::now() < end {
        let report = svc.pump();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(store.status(job).unwrap(), JobStatus::Done);

    let entries = ledger.range(1, 10_000).unwrap();
    let kinds: Vec<LedgerKind> = entries.iter().map(|e| e.draft.kind).collect();
    assert_eq!(kinds.first(), Some(&LedgerKind::Genesis));
    assert_eq!(kinds[1..3], [LedgerKind::JobQueued, LedgerKind::JobStarted]);
    assert_eq!(kinds.last(), Some(&LedgerKind::JobFinished));
    let events = store.events(job).unwrap();
    assert!(!events.is_empty());
    let mirrored: Vec<_> = entries
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::AgentEvent)
        .collect();
    assert_eq!(
        mirrored.len(),
        events.len(),
        "every stored event is in the ledger"
    );
    for (entry, event) in mirrored.iter().zip(&events) {
        assert_eq!(
            (entry.draft.ts, &entry.draft.actor),
            (event.ts, &event.actor)
        );
        assert_eq!(entry.draft.job, Some(job));
        assert!(
            entry
                .draft
                .payload_json
                .starts_with(&format!(r#"{{"event":"{}""#, event.kind))
        );
    }
    let report = verify_all(&*ledger).unwrap();
    assert!(report.is_ok());
    assert_eq!(report.entries, entries.len() as u64);
}
