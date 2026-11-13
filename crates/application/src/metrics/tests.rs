use std::rc::Rc;

use swarmux_domain::JobId;
use swarmux_domain::ledger::{LedgerDraft, LedgerKind, Payload};

use super::*;
use crate::testing::{FixedClock, InMemoryLedger};

const HOUR: Millis = 3_600_000;

fn draft(ts: Millis, kind: LedgerKind, job: Option<i64>, payload: Payload) -> LedgerDraft {
    LedgerDraft {
        ts,
        kind,
        actor: "gate".into(),
        job: job.map(JobId),
        session: None,
        payload_json: payload.build(),
    }
}

fn pass(ts: Millis, job: i64) -> LedgerDraft {
    draft(
        ts,
        LedgerKind::GateVerdict,
        Some(job),
        Payload::new()
            .str("event", "Stop")
            .str("outcome", "pass")
            .int("blocks", 0)
            .objects("checks", vec![Payload::new(), Payload::new()]),
    )
}

fn noise(ts: Millis) -> LedgerDraft {
    draft(
        ts,
        LedgerKind::AgentEvent,
        None,
        Payload::new().str("event", "x"),
    )
}

fn ledger_of(drafts: Vec<LedgerDraft>) -> Rc<InMemoryLedger> {
    let ledger = Rc::new(InMemoryLedger::default());
    for d in drafts {
        ledger.append(d).unwrap();
    }
    ledger
}

#[test]
fn the_window_is_read_back_to_its_start_and_no_further() {
    let now = 100 * HOUR;
    let mut drafts: Vec<LedgerDraft> = Vec::new();
    // A long past, then the window's entries.
    drafts.extend((1..=1200).map(noise));
    drafts.push(pass(now - 2 * HOUR, 1));
    drafts.push(pass(now - HOUR, 2));
    let ledger = ledger_of(drafts);
    let (entries, reached) = read_window(&*ledger, now - 24 * HOUR, MAX_ENTRIES).unwrap();
    assert!(reached);
    // Pages are whole, so a few older entries come with the last page, but
    // nowhere near all 1200: the read stopped when it saw one before the window.
    assert!(entries.len() < 1000, "{}", entries.len());
    assert!(
        entries.windows(2).all(|w| w[0].seq + 1 == w[1].seq),
        "in order, no gaps"
    );
    assert_eq!(entries.last().unwrap().seq, 1203);
    let m = read_metrics(&*ledger, now, 24 * HOUR).unwrap();
    assert_eq!(m.verified, 2);
    assert_eq!(m.checks_run, 4);
    assert!(m.complete);
}

#[test]
fn a_read_cut_short_says_so() {
    let now = 100 * HOUR;
    // Every entry is inside the window; more than the cap allows.
    let drafts: Vec<LedgerDraft> = (0..1500).map(|i| noise(now - HOUR + i)).collect();
    let ledger = ledger_of(drafts);
    let (entries, reached) = read_window(&*ledger, now - 24 * HOUR, 600).unwrap();
    assert!(!reached);
    assert!(
        entries.len() >= 600 && entries.len() < 1501,
        "{}",
        entries.len()
    );
    // The newest entries are the ones kept.
    assert_eq!(
        entries.last().unwrap().seq,
        1501,
        "the genesis entry makes 1501"
    );
    let (all, reached) = read_window(&*ledger, now - 24 * HOUR, 100_000).unwrap();
    assert!(reached);
    assert_eq!(all.len(), 1501);
}

#[test]
fn an_empty_ledger_gives_empty_figures() {
    let ledger = ledger_of(Vec::new());
    let m = read_metrics(&*ledger, 5 * HOUR, 24 * HOUR).unwrap();
    assert_eq!((m.verified, m.checks_run, m.bundles_sealed), (0, 0, 0));
    assert_eq!(m.first_run, None);
    assert_eq!(m.claim_to_verified, None);
    assert!(m.complete);
}

#[test]
fn the_figures_are_kept_for_a_while_then_read_again() {
    let clock = Rc::new(FixedClock::new(50 * HOUR));
    let ledger = ledger_of(vec![pass(50 * HOUR - 1000, 1)]);
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let mut metrics = MetricsService::new(service, 24);
    assert_eq!(metrics.window_ms(), 24 * HOUR);
    let now = 50 * HOUR;
    assert_eq!(metrics.current(now).unwrap().verified, 1);
    // Another job is verified: the kept figures do not know yet.
    ledger.append(pass(now + 1000, 2)).unwrap();
    assert_eq!(metrics.current(now + 5_000).unwrap().verified, 1);
    assert_eq!(metrics.current(now + REFRESH_MS - 1).unwrap().verified, 1);
    // Past the refresh, the ledger is read again.
    assert_eq!(metrics.current(now + REFRESH_MS).unwrap().verified, 2);
}

#[test]
fn the_window_is_kept_within_what_the_setting_allows() {
    let clock = Rc::new(FixedClock::new(0));
    let ledger = ledger_of(Vec::new());
    let service = Rc::new(LedgerService::new(ledger, clock));
    assert_eq!(MetricsService::new(service.clone(), 0).window_ms(), HOUR);
    assert_eq!(
        MetricsService::new(service, 100_000).window_ms(),
        Millis::from(MAX_HOURS) * HOUR
    );
}
