use crossterm::event::KeyCode;
use swarmux_application::PlanReply;
use swarmux_application::testing::asks;
use swarmux_domain::mission::{Acceptance, Constraints, Contract};
use swarmux_domain::plan::{Plan, PlanStatus, PlanTask};

use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app};

fn plan() -> Plan {
    let task = |id: &str, files: &[&str], deps: &[&str]| PlanTask {
        id: id.into(),
        title: format!("task {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: "cargo test".into(),
    };
    Plan {
        summary: "index, then queries".into(),
        tasks: vec![
            task("t1", &["src/index.rs"], &[]),
            task("t2", &["src/query.rs"], &["t1"]),
        ],
    }
}

/// A plan-first mission whose plan has just come back.
fn proposed() -> (TestApp, String) {
    let mut t = test_app();
    t.with_session("search");
    let session = t.app.sessions[0].id;
    let contract = Contract {
        title: "Fast search".into(),
        goal: "make search answer in under 50 ms on the bench corpus".into(),
        acceptance: Acceptance {
            checks: vec!["cargo test".into()],
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec![],
            protected: vec!["tests/**".into()],
        },
        ..Contract::default()
    };
    let slug = t
        .app
        .mission_service()
        .create(contract, "/work", Some(session))
        .unwrap();
    t.linter.reply(0, asks(&[]));
    t.app.update();
    t.app.screen = Screen::Missions;
    t.keys([KeyCode::Char('p'), KeyCode::Char('r')]);
    assert_eq!(t.app.error, None);
    assert!(t.app.mission_planning(&slug));
    t.planner.reply(
        0,
        PlanReply {
            result: Ok(plan()),
            cost_usd: 0.05,
        },
    );
    // One update takes the reply, the next shows it in the inbox.
    t.app.update();
    t.app.update();
    assert_eq!(
        t.app.mission_plan(&slug).map(|r| r.status),
        Some(PlanStatus::Proposed)
    );
    (t, slug)
}

fn status(t: &TestApp, slug: &str) -> Option<PlanStatus> {
    t.app.mission_plan(slug).map(|r| r.status)
}

#[test]
fn the_plan_waits_in_the_inbox_and_y_approves_it_there() {
    let (mut t, slug) = proposed();
    assert_eq!(t.runner.started(), 0, "nothing is built yet");
    t.app.screen = Screen::Mission;
    let item = t.app.mission.selected_item().expect("the plan is an item");
    assert_eq!(item.id.as_str(), format!("plan:{slug}"));
    t.keys([KeyCode::Char('y')]);
    assert_eq!(t.app.error, None);
    t.app.update();
    let prompt = t.runner.request(0).prompt;
    assert!(prompt.contains("Approved plan:"), "{prompt}");
    assert_eq!(status(&t, &slug), Some(PlanStatus::Approved));
    assert!(
        t.app
            .mission
            .selected_item()
            .is_none_or(|i| i.id.as_str() != format!("plan:{slug}")),
        "the item is answered"
    );
}

#[test]
fn y_approves_from_the_missions_screen_too() {
    let (mut t, slug) = proposed();
    t.app.screen = Screen::Missions;
    t.keys([KeyCode::Char('y')]);
    assert_eq!(t.app.error, None);
    let job = t
        .app
        .missions()
        .iter()
        .find(|m| m.slug == slug)
        .unwrap()
        .job;
    assert!(job.is_some());
}

#[test]
fn n_asks_for_a_note_and_sends_the_plan_back() {
    let (mut t, slug) = proposed();
    t.keys([KeyCode::Char('n')]);
    t.keys("split t1 in two".chars().map(KeyCode::Char));
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.error, None);
    assert!(t.app.mission_planning(&slug));
    assert!(t.planner.requests()[1].prompt.contains("split t1 in two"));
    // While the new plan is out nothing is waiting.
    t.app.update();
    t.app.screen = Screen::Mission;
    assert!(
        t.app
            .mission
            .selected_item()
            .is_none_or(|i| i.id.as_str() != format!("plan:{slug}"))
    );
}

#[test]
fn c_cancels_the_plan_and_esc_leaves_it_waiting() {
    let (mut t, slug) = proposed();
    t.keys([KeyCode::Char('n'), KeyCode::Esc]);
    assert_eq!(status(&t, &slug), Some(PlanStatus::Proposed));
    t.keys([KeyCode::Char('c')]);
    assert_eq!(status(&t, &slug), Some(PlanStatus::Idle));
    assert_eq!(t.runner.started(), 0);
}

#[test]
fn enter_on_the_plan_item_opens_the_mission() {
    let (mut t, slug) = proposed();
    t.app.screen = Screen::Mission;
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Missions);
    assert_eq!(t.app.missions()[t.app.missions_screen.selected].slug, slug);
}
