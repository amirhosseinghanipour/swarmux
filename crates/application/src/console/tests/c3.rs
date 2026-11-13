//! Per-file counts, due, metrics and the failing line, read through the
//! console snapshot.

use super::*;

#[test]
fn a_sealed_bundles_files_reach_the_outcome_beside_its_totals() {
    let snap = evening().snapshot();
    let o = outcome(&snap, "mission:fix-invoice");
    let d = o.diff.as_ref().unwrap();
    assert_eq!((d.files, d.insertions, d.deletions), (3, 20, 4));
    let counts: Vec<(&str, u32, u32)> = o
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.insertions, f.deletions))
        .collect();
    assert_eq!(
        counts,
        [
            ("src/etl.rs", 12, 2),
            ("src/etl/counts.rs", 5, 1),
            ("docs/etl.md", 3, 1)
        ]
    );
    assert_eq!(
        o.files[0].change,
        Some(crate::console::ChangeKind::Modified)
    );
    // The list adds up to the totals.
    let (ins, del) = o
        .files
        .iter()
        .fold((0, 0), |(i, d), f| (i + f.insertions, d + f.deletions));
    assert_eq!((ins, del), (d.insertions, d.deletions));
}

#[test]
fn a_bundle_sealed_before_per_file_counts_shows_totals_only() {
    let mut r = evening();
    let mut old = stored_bundle();
    old.bundle.diff.changes.clear();
    r.stored.insert("fix-invoice".into(), old);
    let snap = r.snapshot();
    let o = outcome(&snap, "mission:fix-invoice");
    // The totals are there; no count is invented for the files.
    assert_eq!(o.diff.as_ref().unwrap().files, 3);
    assert!(o.files.is_empty());
}

#[test]
fn counts_taken_when_the_build_was_checked_show_per_task_and_outside_the_plan() {
    use swarmux_domain::plan::check_drift_counted;
    let mut r = Rig::new();
    let job = r.job(1, "api-gateway", JobStatus::Done);
    r.fact_mut(job).finished_at = Some(2_000_000);
    let mut m = mission("wide", MissionState::Verifying, 3_000_000);
    m.job = Some(job);
    r.missions.push(m);
    let plan = Plan {
        summary: "api, then docs".into(),
        tasks: vec![
            task("t1", &["src/api/**"], &[]),
            task("t2", &["docs/**"], &["t1"]),
        ],
    };
    let drift = check_drift_counted(
        &plan,
        &[
            DiffFile::text("src/api/routes.rs", 30, 4),
            DiffFile::text("src/api/mod.rs", 2, 0),
            DiffFile::text("scripts/x.sh", 7, 1),
            DiffFile::text("Cargo.lock", 90, 90),
        ],
        0.3,
    );
    r.plans.insert(
        "wide".into(),
        PlanRecord {
            status: PlanStatus::Approved,
            plan: Some(plan),
            rounds: 1,
            drift: Some(drift),
            ..PlanRecord::default()
        },
    );
    let snap = r.snapshot();
    let o = outcome(&snap, "mission:wide");
    // Nothing is sealed yet: the files are the ones counted for the check.
    assert!(o.diff.is_none());
    let paths: Vec<&str> = o.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["scripts/x.sh", "src/api/mod.rs", "src/api/routes.rs"]
    );
    let plan = o.plan.as_ref().unwrap();
    let t1 = &plan.tasks[0];
    let counts: Vec<(&str, u32, u32)> = t1
        .changed
        .iter()
        .map(|f| (f.path.as_str(), f.insertions, f.deletions))
        .collect();
    assert_eq!(
        counts,
        [("src/api/mod.rs", 2, 0), ("src/api/routes.rs", 30, 4)]
    );
    assert!(plan.tasks[1].changed.is_empty(), "docs were never touched");
    let drift = plan.drift.as_ref().unwrap();
    assert_eq!(drift.untouched, ["t2"]);
    assert_eq!(drift.outside_files.len(), 1);
    assert_eq!(drift.outside_files[0].path, "scripts/x.sh");
    assert_eq!(drift.outside_files[0].insertions, 7);
}

#[test]
fn a_mission_due_time_is_shown_only_when_the_contract_has_one() {
    let mut r = Rig::new();
    let mut with = mission("with-due", MissionState::Escalated, 9_000);
    with.contract.due = Some(NOW + 3_600_000);
    r.missions.push(with);
    r.missions
        .push(mission("without-due", MissionState::Escalated, 8_000));
    let snap = r.snapshot();
    let due_of = |id: &str| outcome(&snap, id).contract.as_ref().unwrap().due;
    assert_eq!(due_of("mission:with-due"), Some(NOW + 3_600_000));
    assert_eq!(due_of("mission:without-due"), None);
}

#[test]
fn among_decisions_of_one_priority_the_earlier_due_time_comes_first_and_none_last() {
    let mut r = Rig::new();
    // All escalated (P2); the one waiting longest has no due time.
    let none = mission("a-none", MissionState::Escalated, 1_000);
    let mut late = mission("b-late", MissionState::Escalated, 2_000);
    late.contract.due = Some(NOW + 9 * 3_600_000);
    let mut soon = mission("c-soon", MissionState::Escalated, 3_000);
    soon.contract.due = Some(NOW + 3_600_000);
    r.missions.extend([none, late, soon]);
    let snap = r.snapshot();
    assert_eq!(
        ids(group(&snap, GroupKind::Decide)),
        ["mission:c-soon", "mission:b-late", "mission:a-none"]
    );
}

#[test]
fn the_snapshot_carries_the_metrics_it_is_given_and_none_otherwise() {
    use swarmux_domain::console::{FirstRun, Metrics, Spread};
    let mut r = Rig::new();
    assert_eq!(r.snapshot().metrics, None, "no ledger read, no figures");
    let mut m = Metrics::none(NOW, 24 * 3_600_000);
    m.first_run = Some(FirstRun {
        passed: 3,
        judged: 4,
    });
    m.claim_to_verified = Some(Spread {
        median_ms: 130_000,
        p90_ms: 560_000,
        samples: 4,
    });
    m.verified = 4;
    m.verified_per_hour = Some(0.5);
    m.checks_run = 17;
    m.bundles_sealed = 2;
    r.metrics = Some(m);
    let view = r.snapshot().metrics.unwrap();
    assert_eq!(view.window_ms, 24 * 3_600_000);
    let f = view.first_run.unwrap();
    assert_eq!((f.passed, f.judged), (3, 4));
    assert!((f.rate - 0.75).abs() < 1e-9);
    assert_eq!(view.claim_to_verified.unwrap().p90_ms, 560_000);
    assert_eq!(
        (view.verified, view.checks_run, view.bundles_sealed),
        (4, 17, 2)
    );
    assert_eq!(view.verified_per_hour, Some(0.5));
    // Without data a figure is null, not zero.
    r.metrics = Some(Metrics::none(NOW, 3_600_000));
    let view = r.snapshot().metrics.unwrap();
    assert_eq!(
        (
            view.first_run,
            view.claim_to_verified,
            view.verified_per_hour
        ),
        (None, None, None)
    );
}

#[test]
fn new_figures_make_a_new_snapshot() {
    use swarmux_domain::console::Metrics;
    let mut r = Rig::new();
    let mut service = ConsoleService::new();
    r.metrics = Some(Metrics::none(NOW, 3_600_000));
    let first = service.snapshot(&r.sources()).revision;
    assert_eq!(
        service.snapshot(&r.sources()).revision,
        first,
        "same inputs"
    );
    let mut more = Metrics::none(NOW, 3_600_000);
    more.verified = 1;
    r.metrics = Some(more);
    assert_eq!(service.snapshot(&r.sources()).revision, first + 1);
}

#[test]
fn a_check_that_could_not_start_says_why_as_its_failing_line() {
    let mut r = Rig::new();
    let job = r.job(1, "api-gateway", JobStatus::Done);
    let mut m = mission("no-cargo", MissionState::Verifying, 3_000_000);
    m.job = Some(job);
    r.missions.push(m);
    r.gates.insert(
        job,
        gate_state(GateOutcome::Escalate, &[("cargo test", false)], 3_100_000),
    );
    let mut broken = run_of("cargo test", false, 0, None);
    broken.exit = None;
    broken.spawn_error = Some("cannot start cargo: No such file or directory\nmore".into());
    let failed = run_of("lint", false, 900, Some("error: unused variable"));
    r.details.insert(
        job,
        GateDetail {
            runs: vec![broken, failed],
            ..GateDetail::default()
        },
    );
    let snap = r.snapshot();
    let runs = &outcome(&snap, "mission:no-cargo").checks.runs;
    assert_eq!(runs[0].status, CheckStatus::Error);
    assert_eq!(
        runs[0].first_failure.as_deref(),
        Some("cannot start cargo: No such file or directory")
    );
    // A check that ran and failed keeps the line it printed.
    assert_eq!(
        runs[1].first_failure.as_deref(),
        Some("error: unused variable")
    );
}
