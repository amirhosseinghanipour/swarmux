use crossterm::event::KeyCode;
use swarmux_application::PlanReply;
use swarmux_application::testing::asks;
use swarmux_domain::mission::{Acceptance, Constraints, Contract};
use swarmux_domain::plan::{Plan, PlanStatus, PlanTask};

use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app, test_app_with_graph};

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
    proposed_in(test_app(), plan())
}

fn proposed_in(mut t: TestApp, plan: Plan) -> (TestApp, String) {
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
            result: Ok(plan),
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
fn an_approved_fan_out_plan_shows_its_task_graph_and_merge_queue() {
    let task = |id: &str, files: &[&str], deps: &[&str]| PlanTask {
        id: id.into(),
        title: format!("part {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: "cargo test".into(),
    };
    let fan = Plan {
        summary: "two modules, then the wiring".into(),
        tasks: vec![
            task("t1", &["src/api/**"], &[]),
            task("t2", &["src/db/**"], &[]),
            task("t3", &["src/main.rs"], &["t1", "t2"]),
        ],
    };
    let (mut t, slug) = proposed_in(test_app_with_graph(), fan);
    t.app.screen = Screen::Missions;
    t.keys([KeyCode::Char('y')]);
    assert_eq!(t.app.error, None);
    t.app.update();
    assert_eq!(t.runner.started(), 2, "t1 and t2 run at once");
    let screen = t.app.render_text(160, 48);
    for want in [
        "task graph ×3",
        "t1 part t1  ▸ running",
        "t3 part t3  · waiting",
        "topology    fanout ×2",
        "merge queue empty",
        "claims      t1 src/api/** · t2 src/db/**",
    ] {
        assert!(screen.contains(want), "{want:?} missing:\n{screen}");
    }
    // t1 ends: it merges, and the card says so.
    t.runner.emit(
        0,
        swarmux_application::ports::RunnerMsg::Event(swarmux_domain::AgentEvent::Result {
            is_error: false,
            subtype: "success".into(),
            cost_usd: 0.1,
            duration_ms: 1,
            num_turns: 1,
            tokens_in: 1,
            tokens_out: 1,
            cache_read: 0,
            cache_creation: None,
            context_window: None,
            subagents_spawned: 0,
            text: "done".into(),
        }),
    );
    t.runner.emit(
        0,
        swarmux_application::ports::RunnerMsg::Exited {
            code: Some(0),
            stderr_tail: String::new(),
        },
    );
    t.app.update();
    t.app.update();
    let run = t.app.mission_plan(&slug).unwrap().graph.clone().unwrap();
    assert_eq!(run.merged, vec!["t1"], "{run:?}");
    let screen = t.app.render_text(160, 48);
    assert!(screen.contains("t1 part t1  ✓ merged"), "{screen}");
    assert_eq!(t.merge.as_ref().unwrap().merged.borrow().len(), 1);
}

#[test]
fn enter_on_the_plan_item_opens_the_mission() {
    let (mut t, slug) = proposed();
    t.app.screen = Screen::Mission;
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Missions);
    assert_eq!(t.app.missions()[t.app.missions_screen.selected].slug, slug);
}
