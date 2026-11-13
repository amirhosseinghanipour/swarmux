//! Plan-first missions: plan, approve, build with the plan, compare.

use super::*;
use crate::testing::{FakeChanges, FakePlanner, InMemoryPlans};
use swarmux_domain::AttentionKind;
use swarmux_domain::plan::{Plan, PlanStatus, PlanTask};

struct P {
    f: Fixture,
    planner: Rc<FakePlanner>,
    plans: Rc<InMemoryPlans>,
    changes: Rc<FakeChanges>,
}

fn task(id: &str, files: &[&str], deps: &[&str]) -> PlanTask {
    PlanTask {
        id: id.into(),
        title: format!("task {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: "cargo test".into(),
    }
}

fn a_plan() -> Plan {
    Plan {
        summary: "index first, then queries".into(),
        tasks: vec![
            task("t1", &["src/search/index.rs"], &[]),
            task("t2", &["src/search/query.rs", "tests/search_*.rs"], &["t1"]),
        ],
    }
}

fn proposed(plan: Plan) -> PlanReply {
    PlanReply {
        result: Ok(plan),
        cost_usd: 0.05,
    }
}

fn setup(plan_first_default: bool) -> P {
    let mut f = fixture();
    let planner = Rc::new(FakePlanner::default());
    let plans = Rc::new(InMemoryPlans::default());
    let changes = Rc::new(FakeChanges::default());
    let svc = std::mem::replace(
        &mut f.svc,
        MissionService::new(f.store.clone(), f.clock.clone()).unwrap(),
    );
    f.svc = svc
        .with_planner(planner.clone(), plans.clone())
        .unwrap()
        .with_plan_settings(PlanSettings {
            plan_first: plan_first_default,
            ..PlanSettings::default()
        })
        .with_changes(
            changes.clone(),
            Rc::new(|job, root| root.join(format!("wt-{job}"))),
        );
    P {
        f,
        planner,
        plans,
        changes,
    }
}

impl P {
    fn mission(&mut self) -> String {
        self.f.create(contract(&["cargo test"], &["tests/**"]))
    }

    fn status(&self, slug: &str) -> Option<PlanStatus> {
        self.f.svc.plan(slug).map(|r| r.status)
    }
}

fn plan_events(f: &Fixture) -> Vec<String> {
    f.mission_entries()
        .into_iter()
        .filter(|e| e.contains(r#""plan":"#))
        .collect()
}

#[test]
fn a_plan_first_mission_plans_before_it_builds() {
    let mut p = setup(true);
    let slug = p.mission();
    assert!(p.f.svc.plan_first(&slug));
    let direct = p.f.svc.launch(&slug, &mut p.f.jobs).unwrap_err();
    assert!(direct.to_string().contains("plans first"), "{direct}");
    let begun = p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    assert_eq!(begun, Begun::Planning);
    assert!(p.f.svc.planning(&slug));
    assert!(p.f.jobs.facts().is_empty(), "no build yet");
    let asked = &p.planner.requests()[0];
    assert!(
        asked.prompt.contains("make search answer"),
        "{}",
        asked.prompt
    );
    assert!(
        p.f.svc.begin(&slug, &mut p.f.jobs).is_err(),
        "already planning"
    );

    p.planner.reply(0, proposed(a_plan()));
    assert!(p.f.pump().changed);
    assert_eq!(p.status(&slug), Some(PlanStatus::Proposed));
    assert_eq!(p.f.svc.get(&slug).unwrap().spend.plan, 0.05);
    let e = p.f.svc.begin(&slug, &mut p.f.jobs).unwrap_err();
    assert!(e.to_string().contains("approve, change or cancel"), "{e}");
    let items = p.f.svc.plan_items(p.f.clock.now());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, AttentionKind::PlanReady);

    let job = p.f.svc.approve_plan(&slug, &mut p.f.jobs).unwrap();
    assert_eq!(p.status(&slug), Some(PlanStatus::Approved));
    assert_eq!(p.f.svc.get(&slug).unwrap().job, Some(job));
    let prompt = p.f.runner.request(0).prompt;
    assert!(prompt.contains("Approved plan:"), "{prompt}");
    assert!(prompt.contains("- t2 task t2 (after t1)"), "{prompt}");
    assert!(
        prompt.contains("`cargo test`"),
        "the contract is still there"
    );
    assert!(p.f.svc.plan_items(p.f.clock.now()).is_empty());
    // Stored and ledgered.
    let stored = crate::missions::PlanStore::list(&*p.plans).unwrap();
    assert_eq!(stored[0].1.status, PlanStatus::Approved);
    let events = plan_events(&p.f);
    assert!(events.iter().any(|e| e.contains(r#""plan":"proposed""#)));
    let approved = events
        .iter()
        .find(|e| e.contains(r#""plan":"approved""#))
        .unwrap();
    assert!(approved.contains(&a_plan().digest()), "{approved}");
}

#[test]
fn a_mission_that_does_not_plan_first_launches_as_before() {
    let mut p = setup(false);
    let slug = p.mission();
    assert!(!p.f.svc.plan_first(&slug));
    let begun = p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    assert!(matches!(begun, Begun::Job(_)));
    assert!(p.planner.requests().is_empty());
    assert!(!p.f.runner.request(0).prompt.contains("Approved plan:"));
}

#[test]
fn the_setting_is_per_mission_and_beats_the_default() {
    let mut p = setup(false);
    let slug = p.mission();
    p.f.svc.set_plan_first(&slug, true).unwrap();
    assert!(p.f.svc.plan_first(&slug));
    assert_eq!(
        p.f.svc.begin(&slug, &mut p.f.jobs).unwrap(),
        Begun::Planning
    );
    let mut q = setup(true);
    let other = q.mission();
    q.f.svc.set_plan_first(&other, false).unwrap();
    assert!(matches!(
        q.f.svc.begin(&other, &mut q.f.jobs).unwrap(),
        Begun::Job(_)
    ));
    assert!(
        q.f.svc.set_plan_first(&other, true).is_err(),
        "not while out"
    );
}

#[test]
fn a_rejected_plan_is_revised_with_the_note() {
    let mut p = setup(true);
    let slug = p.mission();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.planner.reply(0, proposed(a_plan()));
    p.f.pump();
    p.f.svc.reject_plan(&slug, "split the index work").unwrap();
    assert_eq!(p.status(&slug), Some(PlanStatus::Planning));
    let again = &p.planner.requests()[1];
    assert!(
        again.prompt.contains("split the index work"),
        "{}",
        again.prompt
    );
    assert!(again.prompt.contains("rejected"), "{}", again.prompt);
    assert_eq!(p.f.svc.plan(&slug).unwrap().rounds, 2);
    let mut better = a_plan();
    better.tasks.push(task("t3", &["docs/search.md"], &["t2"]));
    p.planner.reply(1, proposed(better));
    p.f.pump();
    let r = p.f.svc.plan(&slug).unwrap();
    assert_eq!(r.status, PlanStatus::Proposed);
    assert_eq!(r.plan.as_ref().unwrap().tasks.len(), 3);
    assert!(
        plan_events(&p.f)
            .iter()
            .any(|e| e.contains(r#""plan":"rejected""#) && e.contains("split the index work"))
    );
    // A third round: nothing can be approved while its plan is out.
    p.f.svc.reject_plan(&slug, "").unwrap();
    assert!(p.f.svc.approve_plan(&slug, &mut p.f.jobs).is_err());
}

#[test]
fn a_failed_plan_says_why_and_can_be_asked_again() {
    let mut p = setup(true);
    let slug = p.mission();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.planner.reply(
        0,
        PlanReply {
            result: Err("tasks wait on each other: t1, t2".into()),
            cost_usd: 0.02,
        },
    );
    p.f.pump();
    let r = p.f.svc.plan(&slug).unwrap();
    assert_eq!(r.status, PlanStatus::Failed);
    assert!(r.note.contains("wait on each other"));
    assert!(p.f.svc.plan_items(0).is_empty());
    assert_eq!(
        p.f.svc.begin(&slug, &mut p.f.jobs).unwrap(),
        Begun::Planning
    );
    assert!(p.f.svc.approve_plan(&slug, &mut p.f.jobs).is_err());
}

#[test]
fn cancelling_drops_the_plan_and_the_run() {
    let mut p = setup(true);
    let slug = p.mission();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.f.svc.cancel_plan(&slug).unwrap();
    assert!(!p.f.svc.planning(&slug));
    p.planner.reply(0, proposed(a_plan()));
    assert!(!p.f.pump().changed, "a cancelled run's reply is dropped");
    assert_eq!(p.status(&slug), Some(PlanStatus::Idle));
    assert!(
        plan_events(&p.f)
            .iter()
            .any(|e| e.contains(r#""plan":"cancelled""#))
    );
    assert_eq!(p.f.state(&slug), MissionState::Linted);
}

#[test]
fn editing_the_contract_voids_the_plan_but_keeps_the_setting() {
    let mut p = setup(false);
    let slug = p.mission();
    p.f.svc.set_plan_first(&slug, true).unwrap();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.planner.reply(0, proposed(a_plan()));
    p.f.pump();
    p.f.svc
        .edit(
            &slug,
            contract(&["cargo test", "cargo clippy"], &["tests/**"]),
        )
        .unwrap();
    let r = p.f.svc.plan(&slug).unwrap();
    assert_eq!((r.status, r.plan_first), (PlanStatus::Idle, Some(true)));
    assert!(r.plan.is_none());
}

#[test]
fn plans_come_back_with_the_store() {
    let mut p = setup(true);
    let slug = p.mission();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.planner.reply(0, proposed(a_plan()));
    p.f.pump();
    let again = MissionService::new(p.f.store.clone(), p.f.clock.clone())
        .unwrap()
        .with_planner(p.planner.clone(), p.plans.clone())
        .unwrap();
    assert_eq!(again.plan(&slug).unwrap().status, PlanStatus::Proposed);
}

/// Plans, approves, builds and finishes a mission whose job changed `files`.
fn built(p: &mut P, files: &[&str]) -> String {
    let slug = p.mission();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.planner.reply(0, proposed(a_plan()));
    p.f.pump();
    let job = p.f.svc.approve_plan(&slug, &mut p.f.jobs).unwrap();
    *p.changes.files.borrow_mut() = files.iter().map(|s| s.to_string()).collect();
    p.f.pump();
    p.f.finish(0, 0.4, 0);
    p.f.pump_with(job, GateOutcome::Pass, p.f.clock.now());
    slug
}

#[test]
fn a_build_that_strays_from_the_plan_raises_a_drift_item() {
    let mut p = setup(true);
    let slug = built(
        &mut p,
        &[
            "src/search/index.rs",
            "src/db/schema.rs",
            "src/db/migrate.rs",
            "Cargo.lock",
        ],
    );
    let r = p.f.svc.plan(&slug).unwrap();
    let d = r.drift.as_ref().expect("compared");
    assert!(d.drifted(), "{d:?}");
    assert_eq!(d.outside, ["src/db/migrate.rs", "src/db/schema.rs"]);
    assert_eq!(d.untouched, ["t2"]);
    let asked = p.changes.asked.borrow();
    assert_eq!(asked.len(), 1, "compared once");
    assert!(asked[0].ends_with("wt-1"), "{:?}", asked[0]);
    let items = p.f.svc.plan_items(p.f.clock.now());
    assert!(
        items
            .iter()
            .any(|i| i.kind == AttentionKind::Drift && i.detail.contains("outside the plan")),
        "{items:?}"
    );
    assert!(
        plan_events(&p.f)
            .iter()
            .any(|e| e.contains(r#""plan":"drift""#))
    );
}

#[test]
fn a_build_that_follows_the_plan_raises_nothing() {
    let mut p = setup(true);
    let slug = built(
        &mut p,
        &[
            "src/search/index.rs",
            "src/search/query.rs",
            "tests/search_basic.rs",
            "Cargo.lock",
        ],
    );
    let d = p.f.svc.plan(&slug).unwrap().drift.clone().unwrap();
    assert!(!d.drifted(), "{d:?}");
    assert!(p.f.svc.plan_items(p.f.clock.now()).is_empty());
    assert!(
        plan_events(&p.f)
            .iter()
            .any(|e| e.contains(r#""plan":"on_plan""#))
    );
}

#[test]
fn a_build_that_cannot_be_compared_is_reported_not_blocked() {
    let mut p = setup(true);
    *p.changes.broken.borrow_mut() = Some("no such folder".into());
    let slug = p.mission();
    p.f.svc.begin(&slug, &mut p.f.jobs).unwrap();
    p.planner.reply(0, proposed(a_plan()));
    p.f.pump();
    let job = p.f.svc.approve_plan(&slug, &mut p.f.jobs).unwrap();
    p.f.pump();
    p.f.finish(0, 0.1, 0);
    let report = p.f.pump_with(job, GateOutcome::Pass, p.f.clock.now());
    let said: Vec<String> = report.errors.iter().map(|e| e.to_string()).collect();
    assert!(
        said.iter().any(|e| e.contains("could not compare")),
        "{said:?}"
    );
    assert!(p.f.svc.plan(&slug).unwrap().drift.is_none());
    assert_eq!(p.f.state(&slug), MissionState::Delivered);
}

#[test]
fn a_counted_build_keeps_the_lines_of_each_file_in_the_drift_report() {
    use swarmux_domain::evidence::DiffFile;
    let mut p = setup(true);
    *p.changes.counted.borrow_mut() = vec![
        DiffFile::text("src/search/index.rs", 30, 4),
        DiffFile::text("src/db/schema.rs", 9, 0),
        DiffFile::text("Cargo.lock", 80, 80),
    ];
    let slug = built(&mut p, &[]);
    let d = p.f.svc.plan(&slug).unwrap().drift.clone().unwrap();
    assert_eq!(d.outside, ["src/db/schema.rs"]);
    assert_eq!(d.changed, 2, "the lockfile is not scope");
    let counts: Vec<(&str, u32, u32)> = d
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.insertions, f.deletions))
        .collect();
    assert_eq!(
        counts,
        [("src/db/schema.rs", 9, 0), ("src/search/index.rs", 30, 4)]
    );
    // The names alone were not asked for: the counted list was enough.
    assert!(p.changes.asked.borrow().is_empty());
}

#[test]
fn a_build_compared_by_name_keeps_no_counts() {
    let mut p = setup(true);
    let slug = built(&mut p, &["src/search/index.rs"]);
    let d = p.f.svc.plan(&slug).unwrap().drift.clone().unwrap();
    assert!(d.files.is_empty(), "no count is worked out after the fact");
}
