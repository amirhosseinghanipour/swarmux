use std::rc::Rc;

use swarmux_domain::ledger::{LedgerDraft, LedgerKind};

use super::testing::{HashingSigner, MemoryAnchors};
use super::*;
use crate::testing::{FixedClock, InMemoryLedger};

struct Rig {
    ledger: Rc<InMemoryLedger>,
    anchors: Rc<MemoryAnchors>,
    clock: Rc<FixedClock>,
    service: AnchorService,
}

fn draft(n: u32, text: &str) -> LedgerDraft {
    LedgerDraft {
        ts: i64::from(n),
        kind: LedgerKind::JobQueued,
        actor: "swarmux".into(),
        job: None,
        session: None,
        payload_json: format!("{{\"n\":{n},\"text\":\"{text}\"}}"),
    }
}

fn fill(ledger: &InMemoryLedger, n: u32, text: &str) {
    for i in 1..=n {
        ledger.append(draft(i, text)).unwrap();
    }
}

fn rig(entries: u32) -> Rig {
    let ledger = Rc::new(InMemoryLedger::default());
    fill(&ledger, entries, "original");
    let anchors = Rc::new(MemoryAnchors::default());
    let clock = Rc::new(FixedClock::new(1_000));
    let service = AnchorService::new(
        ledger.clone(),
        anchors.clone(),
        Rc::new(HashingSigner("key-a".into())),
        clock.clone(),
    );
    Rig {
        ledger,
        anchors,
        clock,
        service,
    }
}

#[test]
fn a_checkpoint_signs_the_head_and_an_untouched_ledger_verifies_against_it() {
    let r = rig(9);
    let head = r.ledger.head().unwrap().unwrap();
    let cp = r.service.checkpoint().unwrap();
    assert_eq!((cp.seq, cp.head, cp.at), (head.seq, head.hash, 1_000));
    assert_eq!(r.anchors.list().unwrap(), vec![cp.clone()]);

    let report = r.service.audit(&[]).unwrap();
    assert!(report.is_ok(), "{:?}", report.problems());
    assert_eq!(report.checkpoints.len(), 1);
    assert_eq!(report.unanchored, 0);

    // Work continues: the new entries are not anchored yet, nothing is wrong.
    r.ledger.append(draft(50, "later")).unwrap();
    r.ledger.append(draft(51, "later")).unwrap();
    let report = r.service.audit(&[]).unwrap();
    assert!(report.is_ok());
    assert_eq!(report.unanchored, 2);
    assert_eq!(r.service.covering(5).unwrap(), Some(cp));
    assert_eq!(r.service.covering(500).unwrap(), None);
}

#[test]
fn an_empty_ledger_has_nothing_to_anchor() {
    let r = rig(0);
    assert!(r.service.checkpoint().is_err());
    assert!(r.service.audit(&[]).unwrap().is_ok());
}

#[test]
fn a_cut_off_tail_is_reported_even_though_the_chain_still_verifies() {
    let r = rig(9);
    let cp = r.service.checkpoint().unwrap();
    // An attacker removes the newest entries: what is left is a valid chain.
    for seq in (cp.seq - 2)..=cp.seq {
        r.ledger.remove(seq);
    }
    let report = r.service.audit(&[]).unwrap();
    assert!(report.chain.is_none(), "the shorter chain is whole");
    assert!(!report.is_ok());
    let problems = report.problems().join("\n");
    assert!(problems.contains("cut short"), "{problems}");
    assert!(
        problems.contains(&format!("checkpoint #{}", cp.seq)),
        "{problems}"
    );
}

#[test]
fn a_rewritten_and_rechained_history_is_reported() {
    let r = rig(9);
    let cp = r.service.checkpoint().unwrap();
    // The same number of entries, edited from entry 6 on and rechained from
    // there, so the chain verifies and has a different head.
    let forged = Rc::new(InMemoryLedger::default());
    fill(&forged, 4, "original");
    for i in 5..=9 {
        forged.append(draft(i, "forged")).unwrap();
    }
    assert!(verify_all(&*forged).unwrap().is_ok());
    assert_ne!(forged.head().unwrap().unwrap().hash, cp.head);

    let audit = AnchorService::new(
        forged,
        r.anchors.clone(),
        Rc::new(HashingSigner("key-a".into())),
        r.clock.clone(),
    );
    let report = audit.audit(&[]).unwrap();
    assert!(report.chain.is_none());
    assert!(!report.is_ok());
    let problems = report.problems().join("\n");
    assert!(problems.contains("rewritten"), "{problems}");
}

#[test]
fn a_rewrite_before_the_checkpoint_taken_later_is_not_noticed_by_that_checkpoint() {
    // The honest limit: entries after the newest checkpoint are not anchored.
    let r = rig(5);
    r.service.checkpoint().unwrap();
    r.ledger.append(draft(60, "after")).unwrap();
    r.ledger.tamper(6, "{\"n\":1}");
    let report = r.service.audit(&[]).unwrap();
    assert!(
        report.chain.is_some(),
        "the chain itself still notices an edit"
    );
    assert_eq!(report.unanchored, 1);
}

#[test]
fn a_forged_checkpoint_and_one_from_another_key_are_told_apart() {
    let r = rig(5);
    let mut cp = r.service.checkpoint().unwrap();
    // The checkpoint store is edited to agree with a rewritten ledger.
    cp.head = [9; 32];
    r.anchors.replace(vec![cp]);
    let report = r.service.audit(&[]).unwrap();
    assert!(!report.checkpoints[0].signed);
    assert!(report.problems().join("\n").contains("signature"));

    // A checkpoint made with another key is judged by its own key.
    let other = AnchorService::new(
        r.ledger.clone(),
        Rc::new(MemoryAnchors::default()),
        Rc::new(HashingSigner("key-b".into())),
        r.clock.clone(),
    );
    let foreign = other.checkpoint().unwrap();
    r.anchors.replace(Vec::new());
    let report = r.service.audit(&[foreign]).unwrap();
    assert!(report.checkpoints[0].signed);
    assert!(!report.checkpoints[0].own_key);
    assert!(report.is_ok());
}

#[test]
fn exported_checkpoints_kept_elsewhere_catch_a_store_that_was_wiped() {
    let r = rig(7);
    let cp = r.service.checkpoint().unwrap();
    let line = cp.to_json();
    // The attacker also empties the store and cuts the ledger.
    r.anchors.replace(Vec::new());
    r.ledger.remove(cp.seq);
    r.ledger.remove(cp.seq - 1);
    assert!(r.service.audit(&[]).unwrap().is_ok(), "nothing remembered");
    let kept = Checkpoint::from_json(&line).unwrap();
    let report = r.service.audit(&[kept]).unwrap();
    assert!(!report.is_ok());
    assert!(report.problems().join("\n").contains("cut short"));
}
