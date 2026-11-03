use std::time::{Duration, Instant};

use swarmux_domain::evidence::{LabelKind, PrState, RunOutcome};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::mission::MissionState;

use super::testing::{EvidenceRig, pr};
use super::*;
use crate::HistoryService;

fn delivered(rig: &EvidenceRig, slug: &str) -> Mission {
    rig.mission(slug, MissionState::Delivered, true)
}

#[test]
fn sealing_records_what_the_ledger_and_the_job_hold() {
    let rig = EvidenceRig::new();
    let m = delivered(&rig, "parser-fix");
    let b = rig.service.seal(&m).unwrap();
    assert_eq!(b.contract.slug, "parser-fix");
    assert_eq!(b.job, m.job.unwrap().0);
    assert_eq!(b.head_sha, "cafe0000cafe0000");
    assert_eq!(b.base_sha.as_deref(), Some("base0000"));
    assert_eq!(b.diff.files, 2);
    assert_eq!(b.repo, "/repo");
    // The check the passing verdict ran, with the command to run it again.
    assert_eq!(b.checks.len(), 1);
    let c = &b.checks[0];
    assert_eq!((c.program.as_str(), c.args.len()), ("cargo", 3));
    assert!(c.passed && c.timeout_secs > 0);
    // A blocked stop and the pass, each with a digest of its entry.
    let outcomes: Vec<&str> = b.gates.iter().map(|g| g.outcome.as_str()).collect();
    assert_eq!(outcomes, ["block", "pass"]);
    assert!(b.gates.iter().all(|g| g.digest.len() == 64));
    assert_eq!(b.blocks(), 1);
    // Cost by model: the job's, then the lint's.
    assert!((b.cost.implement - 1.5).abs() < 1e-9);
    assert_eq!(b.cost.by_model[0].0, "sonnet");
    assert_eq!(b.cost.by_model[1].0, "haiku");
    assert_eq!(b.cost.basis, "reported");
    assert_eq!(b.wall_ms, 90_000);
    assert_eq!(b.signature_ok(), Ok(true));

    // The range runs from the job's first entry to its last, whose hash it names.
    let entries = rig.ledger.entries();
    let first = entries.iter().find(|e| e.draft.job == m.job).unwrap();
    let last = entries
        .iter()
        .rev()
        .find(|e| e.draft.job == m.job && e.draft.kind != LedgerKind::Evidence)
        .unwrap();
    assert_eq!(b.ledger.from_seq, first.seq);
    assert_eq!(b.ledger.to_seq, last.seq);
    assert_eq!(b.ledger.head_hash, swarmux_domain::ledger::hex(&last.hash));

    // Kept, and sealed into the ledger itself.
    let stored = rig.service.get("parser-fix").unwrap().unwrap();
    assert_eq!(stored.digest, b.digest_hex());
    let seal = entries
        .iter()
        .find(|e| e.draft.kind == LedgerKind::Evidence)
        .unwrap();
    assert_eq!(stored.seal_seq, Some(seal.seq));
    assert!(seal.draft.payload_json.contains(&b.digest_hex()));
}

#[test]
fn only_a_delivered_mission_with_a_passing_verdict_is_sealed() {
    let rig = EvidenceRig::new();
    let escalated = rig.mission("e", MissionState::Escalated, false);
    assert!(
        rig.service
            .seal(&escalated)
            .unwrap_err()
            .to_string()
            .contains("only a delivered")
    );
    // Delivered on paper, but the ledger never saw the gate pass.
    let mut forged = rig.mission("f", MissionState::Delivered, false);
    let other = rig.mission("g", MissionState::Delivered, false);
    forged.job = Some(swarmux_domain::JobId(other.job.unwrap().0 + 50));
    let why = rig.service.seal(&forged).unwrap_err().to_string();
    assert!(why.contains("no passing gate verdict"), "{why}");
    // Outside a git repository there is no commit to name.
    *rig.vcs.facts.borrow_mut() = None;
    let why = rig.service.seal(&other).unwrap_err().to_string();
    assert!(why.contains("not in a git repository"), "{why}");
    assert!(rig.service.list().unwrap().is_empty());
}

#[test]
fn verify_reruns_the_checks_in_a_clean_checkout_and_matches() {
    let rig = EvidenceRig::new();
    let b = rig.service.seal(&delivered(&rig, "a")).unwrap();
    let report = rig.service.verify_now(&b).unwrap();
    assert_eq!(report.verdict(), "match", "{:?}", report.items);
    let asked = rig.checkouts.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "cafe0000cafe0000");
    assert_eq!(asked[0].0, PathBuf::from("/repo"));
    // The checks ran in the clean copy, not in the repository.
    let roots = rig.checker.roots.lock().unwrap().clone();
    assert_eq!(roots, [PathBuf::from("/clean")]);
    let subjects: Vec<&str> = report.items.iter().map(|i| i.subject.as_str()).collect();
    assert!(subjects.contains(&"signature"), "{subjects:?}");
    assert!(subjects.contains(&"ledger chain"), "{subjects:?}");
    assert!(subjects.iter().any(|s| s.starts_with("ledger seal")));
    assert!(subjects.contains(&"clean checkout"));
}

#[test]
fn verify_reports_a_check_that_no_longer_passes() {
    let rig = EvidenceRig::new();
    let b = rig.service.seal(&delivered(&rig, "a")).unwrap();
    rig.checker.exit("cargo test -p parser", 101);
    let report = rig.service.verify_now(&b).unwrap();
    assert_eq!(report.verdict(), "mismatch");
    let bad: Vec<_> = report.mismatches().collect();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].subject, "check cargo test -p parser");
    assert_eq!(bad[0].expected, "pass");
    assert_eq!(bad[0].found, "fail (exit 101)");
}

#[test]
fn verify_reports_a_commit_that_cannot_be_checked_out() {
    let rig = EvidenceRig::new();
    let b = rig.service.seal(&delivered(&rig, "a")).unwrap();
    *rig.checkouts.refuse.lock().unwrap() = Some("bad object".into());
    let report = rig.service.verify_now(&b).unwrap();
    assert_eq!(report.verdict(), "mismatch");
    assert!(report.mismatches().any(|i| i.subject == "clean checkout"));
    assert!(rig.checker.roots.lock().unwrap().is_empty());
}

#[test]
fn an_edited_bundle_or_ledger_does_not_verify() {
    let rig = EvidenceRig::new();
    let b = rig.service.seal(&delivered(&rig, "a")).unwrap();

    // The stored copy was edited: its digest, signature and seal disagree.
    rig.bundles.tamper("a", |b| b.diff.insertions = 1);
    let edited = rig.service.get("a").unwrap().unwrap().bundle;
    let items = rig
        .service
        .static_items(&edited, rig.service.get("a").unwrap().as_ref());
    let failed: Vec<&str> = items
        .iter()
        .filter(|i| !i.ok)
        .map(|i| i.subject.as_str())
        .collect();
    assert!(failed.contains(&"signature"), "{failed:?}");
    assert!(failed.contains(&"stored digest"), "{failed:?}");
    assert!(failed.iter().any(|s| s.starts_with("ledger seal")));

    // The ledger was edited behind the bundle's range.
    rig.bundles.tamper("a", |b2| *b2 = b.clone());
    let fresh = rig.service.get("a").unwrap().unwrap();
    assert!(
        rig.service
            .static_items(&b, Some(&fresh))
            .iter()
            .all(|i| i.ok)
    );
    rig.ledger.tamper(b.ledger.from_seq, "{\"x\":1}");
    let items = rig.service.static_items(&b, Some(&fresh));
    assert!(items.iter().any(|i| i.subject == "ledger chain" && !i.ok));
}

#[test]
fn work_that_was_not_committed_is_flagged() {
    let rig = EvidenceRig::new();
    rig.vcs.facts.borrow_mut().as_mut().unwrap().dirty = true;
    let b = rig.service.seal(&delivered(&rig, "a")).unwrap();
    assert!(b.dirty);
    let report = rig.service.verify_now(&b).unwrap();
    assert!(
        report
            .mismatches()
            .any(|i| i.subject == "work in the commit")
    );
}

#[test]
fn without_a_way_to_check_out_nothing_matches() {
    let rig = EvidenceRig::new();
    let m = delivered(&rig, "a");
    let service = EvidenceService::new(
        rig.bundles.clone(),
        rig.ledger_service.clone(),
        rig.store.clone(),
        rig.vcs.clone(),
        rig.clock.clone(),
    );
    let b = service.seal(&m).unwrap();
    assert_eq!(service.verify_now(&b).unwrap().verdict(), "mismatch");
}

#[test]
fn backfill_seals_delivered_missions_that_have_no_bundle() {
    let rig = EvidenceRig::new();
    let a = delivered(&rig, "a");
    let b = delivered(&rig, "b");
    let pending = rig.mission("c", MissionState::Escalated, false);
    rig.service.seal(&a).unwrap();
    let failed = rig.service.backfill(&[a, b, pending]);
    assert!(failed.is_empty(), "{failed:?}");
    let slugs: Vec<String> = rig
        .service
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.bundle.contract.slug)
        .collect();
    assert_eq!(slugs, ["a", "b"]);
}

fn wait<T>(mut poll: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(v) = poll() {
            return v;
        }
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn start_verify_runs_in_the_background() {
    let rig = EvidenceRig::new();
    rig.service.seal(&delivered(&rig, "a")).unwrap();
    rig.service.start_verify("a").unwrap();
    assert!(rig.service.verifying("a") || rig.service.report("a").is_some());
    wait(|| {
        rig.service.pump_verify();
        rig.service.report("a")
    });
    let report = rig.service.report("a").unwrap();
    assert_eq!(report.verdict(), "match", "{:?}", report.items);
    assert!(!rig.service.verifying("a"));
    assert!(rig.service.start_verify("nope").is_err());
}

#[test]
fn labels_follow_the_pull_request_and_land_in_the_ledger() {
    let rig = EvidenceRig::new();
    rig.service.seal(&delivered(&rig, "a")).unwrap();
    assert_eq!(rig.service.request_labels().unwrap(), 1);
    assert_eq!(rig.service.request_labels().unwrap(), 0, "already asked");
    let req = rig.probe.requests();
    assert_eq!(req[0].branch, "swarmux/demo/1");
    rig.probe.reply(0, Ok(Some(pr(9, PrState::Open))));
    assert_eq!(rig.service.pump_labels(), ["a"]);
    assert!(!rig.service.probing_labels());

    // Merged, then asked again: a new label. Asked again: nothing new.
    assert_eq!(rig.service.request_labels().unwrap(), 1);
    rig.probe
        .reply(1, Ok(Some(pr(9, PrState::Merged { at: 1_000_000 }))));
    assert_eq!(rig.service.pump_labels(), ["a"]);
    assert_eq!(rig.service.request_labels().unwrap(), 1);
    rig.probe
        .reply(2, Ok(Some(pr(9, PrState::Merged { at: 1_000_000 }))));
    assert!(rig.service.pump_labels().is_empty());

    let kinds: Vec<LabelKind> = rig
        .service
        .get("a")
        .unwrap()
        .unwrap()
        .labels
        .iter()
        .map(|l| l.kind)
        .collect();
    assert_eq!(kinds, [LabelKind::PrOpen, LabelKind::Merged]);
    let ledgered = rig
        .ledger
        .entries()
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::Label)
        .count();
    assert_eq!(ledgered, 2);

    // A revert inside the window turns the label.
    let mut reverted = pr(9, PrState::Merged { at: 1_000_000 });
    reverted.reverted_at = Some(1_000_000 + 86_400_000);
    assert_eq!(rig.service.request_labels().unwrap(), 1);
    rig.probe.reply(3, Ok(Some(reverted)));
    assert_eq!(rig.service.pump_labels(), ["a"]);
    assert_eq!(
        rig.service
            .get("a")
            .unwrap()
            .unwrap()
            .labels
            .last()
            .unwrap()
            .kind,
        LabelKind::Reverted
    );
    assert_eq!(
        rig.service.request_labels().unwrap(),
        0,
        "a revert is final"
    );
}

#[test]
fn probe_trouble_is_kept_and_a_branch_without_a_pr_is_left_alone() {
    let rig = EvidenceRig::new();
    rig.service.seal(&delivered(&rig, "a")).unwrap();
    rig.service.request_labels().unwrap();
    rig.probe.reply(0, Ok(None));
    assert!(rig.service.pump_labels().is_empty());
    assert_eq!(rig.service.label_problem(), None);
    rig.service.request_labels().unwrap();
    rig.probe.reply(1, Err("gh is not installed".into()));
    rig.service.pump_labels();
    assert_eq!(
        rig.service.label_problem().as_deref(),
        Some("gh is not installed")
    );
}

#[test]
fn cards_join_jobs_missions_and_bundles() {
    let rig = EvidenceRig::new();
    let m = delivered(&rig, "parser-fix");
    rig.service.seal(&m).unwrap();
    let escalated = rig.mission("stuck", MissionState::Escalated, false);
    // A plain job with no mission.
    let plain = rig
        .mission("plain", MissionState::Draft, false)
        .job
        .unwrap();
    let rows = HistoryService::new(rig.store.clone()).list("").unwrap();
    let stored = rig.service.list().unwrap();
    let cards = build_cards(&rows, &[m.clone(), escalated.clone()], &stored);
    assert_eq!(cards.len(), 3);
    let by = |job| cards.iter().find(|c| c.job == job).unwrap();
    let done = by(m.job.unwrap());
    assert_eq!(done.outcome, RunOutcome::Delivered);
    assert_eq!(done.title, "parser-fix");
    let ev = done.evidence.as_ref().unwrap();
    assert_eq!((ev.checks_passed, ev.checks_total, ev.blocks), (1, 1, 1));
    assert_eq!(ev.digest.len(), 8);
    assert_eq!(done.models[0].0, "sonnet");
    assert_eq!(by(escalated.job.unwrap()).outcome, RunOutcome::Escalated);
    assert!(by(escalated.job.unwrap()).evidence.is_none());
    assert_eq!(by(plain).outcome, RunOutcome::Done);
    assert!(by(plain).mission.is_none());
}
