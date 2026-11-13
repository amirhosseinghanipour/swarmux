//! The Work screen's choices over a console snapshot: selection that
//! follows the outcome, the actions it offers, what it holds up.
//!
//! The snapshot is the read model's own golden (the evening scenario:
//! a held push, a denied edit, a failed job, a plan, a task graph, a
//! verified job and a delivered mission), so the screen is tested over
//! data the read model really produces.

use swarmux_application::console::{
    ActionKind, ConsoleSnapshot, DeckNode, DepState, EdgeKind, GroupKind, HoldState, NodeKind,
    NodeState, OutcomeView,
};

use super::*;

/// The read model's golden snapshot: what the application tests pin.
pub(crate) fn evening() -> ConsoleSnapshot {
    let json = include_str!("../../../application/src/console/golden/snapshot.json");
    serde_json::from_str(json).expect("the golden is a snapshot")
}

pub(crate) fn outcome<'a>(snap: &'a ConsoleSnapshot, id: &str) -> &'a OutcomeView {
    flat(snap)
        .into_iter()
        .find(|o| o.id == id)
        .unwrap_or_else(|| panic!("no outcome {id}"))
}

fn ids(snap: &ConsoleSnapshot) -> Vec<&str> {
    flat(snap).iter().map(|o| o.id.as_str()).collect()
}

fn labels(rows: &[ActionRow]) -> Vec<(&'static str, bool)> {
    rows.iter().map(|r| (r.label, r.enabled)).collect()
}

#[test]
fn the_queue_is_every_outcome_in_the_order_it_is_shown() {
    let snap = evening();
    let all = flat(&snap);
    let groups: Vec<GroupKind> = all.iter().map(|o| o.group).collect();
    let mut sorted = groups.clone();
    sorted.sort_by_key(|g| match g {
        GroupKind::Decide => 0,
        GroupKind::ReadyToLand => 1,
        GroupKind::Running => 2,
        GroupKind::Landed => 3,
    });
    assert_eq!(groups, sorted, "DECIDE, READY TO LAND, RUNNING, LANDED");
    assert_eq!(all.first().map(|o| o.group), Some(GroupKind::Decide));
    assert_eq!(
        all.len(),
        snap.groups.iter().map(|g| g.outcomes.len()).sum::<usize>()
    );
}

#[test]
fn the_first_outcome_is_selected_until_the_user_chooses() {
    let snap = evening();
    let mut w = WorkScreen::default();
    assert_eq!(
        w.current(&snap).map(|o| o.id.as_str()),
        flat(&snap).first().map(|o| o.id.as_str())
    );
    w.settle(&snap);
    assert_eq!(w.selected.as_deref(), Some(flat(&snap)[0].id.as_str()));
}

#[test]
fn stepping_moves_through_the_queue_and_stops_at_its_ends() {
    let snap = evening();
    let order = ids(&snap);
    let mut w = WorkScreen::default();
    w.step(&snap, -1);
    assert_eq!(w.selected.as_deref(), Some(order[0]), "stops at the top");
    w.step(&snap, 1);
    w.step(&snap, 1);
    assert_eq!(w.selected.as_deref(), Some(order[2]));
    for _ in 0..order.len() + 3 {
        w.step(&snap, 1);
    }
    assert_eq!(
        w.selected.as_deref(),
        order.last().copied(),
        "stops at the bottom"
    );
}

#[test]
fn the_selection_follows_the_outcome_when_the_queue_re_ranks() {
    let mut snap = evening();
    let mut w = WorkScreen::default();
    let chosen = ids(&snap)[1].to_string();
    assert!(w.select(&snap, &chosen));
    // The decide group is re-ordered: the chosen outcome is not where it was.
    snap.groups[0].outcomes.reverse();
    w.settle(&snap);
    assert_eq!(w.selected.as_deref(), Some(chosen.as_str()));
    assert_eq!(
        w.current(&snap).map(|o| o.id.as_str()),
        Some(chosen.as_str())
    );
}

#[test]
fn a_selected_outcome_that_leaves_the_queue_gives_way_to_its_neighbour() {
    let mut snap = evening();
    let mut w = WorkScreen::default();
    let order: Vec<String> = ids(&snap).iter().map(|s| s.to_string()).collect();
    assert!(w.select(&snap, &order[2]));
    snap.groups[0].outcomes.retain(|o| o.id != order[2]);
    w.settle(&snap);
    assert_eq!(
        w.selected.as_deref(),
        Some(order[3].as_str()),
        "the one now where it was"
    );
    // And an empty queue selects nothing, without a panic.
    for g in &mut snap.groups {
        g.outcomes.clear();
    }
    w.settle(&snap);
    assert_eq!(w.selected, None);
    assert!(w.current(&snap).is_none());
    w.step(&snap, 1);
}

#[test]
fn choosing_another_outcome_forgets_what_was_read_for_the_last() {
    let snap = evening();
    let mut w = WorkScreen {
        scroll: 7,
        diff: Some(DiffRead {
            job: 2,
            lines: vec!["+x".into()],
            note: None,
        }),
        ..WorkScreen::default()
    };
    let other = ids(&snap)[1].to_string();
    assert!(w.select(&snap, &other));
    assert_eq!(w.scroll, 0);
    assert!(w.diff.is_none());
    assert!(!w.select(&snap, "job:999"), "an unknown outcome is refused");
    assert_eq!(w.selected.as_deref(), Some(other.as_str()));
}

#[test]
fn tabs_start_from_the_top_and_scrolling_stops_at_the_end() {
    let mut w = WorkScreen::default();
    w.max_scroll.set(12);
    w.scroll_by(10);
    w.scroll_by(10);
    assert_eq!(w.scroll, 12);
    w.scroll_by(-100);
    assert_eq!(w.scroll, 0);
    w.scroll_by(5);
    w.show(Tab::Checks);
    assert_eq!((w.tab, w.scroll), (Tab::Checks, 0));
    w.scroll_by(3);
    w.show(Tab::Checks);
    assert_eq!(w.scroll, 3, "the same tab keeps its place");
}

#[test]
fn a_held_call_offers_the_answers_and_names_what_an_irreversible_one_needs() {
    let snap = evening();
    let push = outcome(&snap, "job:2");
    let rows = rows_for(&snap, push);
    // The push is destructive: allow once needs the typed target, and an
    // irreversible call is never always-allowed.
    assert_eq!(
        labels(&rows),
        [
            ("allow once", true),
            ("always allow", false),
            ("deny", true),
            ("kill", true)
        ]
    );
    let allow = &rows[0];
    assert_eq!(allow.action, DECIDE_YES);
    assert!(
        allow.detail.contains("you name the target"),
        "{}",
        allow.detail
    );
    let always = &rows[1];
    assert!(always.detail.contains("never always-allowed"));
    assert_eq!(rows[2].action, DECIDE_NO);
    assert_eq!(held_request(push), Some(7));

    // The same call, not destructive: always allow is on.
    let mut snap = evening();
    snap.holds[0].destructive = false;
    let rows = rows_for(&snap, outcome(&snap, "job:2"));
    assert_eq!(rows[1].action, DECIDE_ALWAYS);
    assert!(rows[1].enabled);
}

#[test]
fn a_failed_job_offers_retry_and_resume_and_no_kill() {
    let snap = evening();
    let rows = rows_for(&snap, outcome(&snap, "job:3"));
    assert_eq!(labels(&rows), [("resume", true), ("retry", true)]);
    assert!(
        rows.iter()
            .all(|r| r.what != Do::Kill && r.what != Do::Land)
    );
}

#[test]
fn a_proposed_plan_is_approved_or_sent_back() {
    let snap = evening();
    let plan = outcome(&snap, "mission:plan-me");
    let rows = rows_for(&snap, plan);
    assert_eq!(rows[0].what, Do::ApprovePlan);
    assert_eq!(rows[1].what, Do::ChangePlan);
    assert_eq!(plan_slug(plan), Some("plan-me"));
    // The land row is there for a mission, disabled, with its reason.
    let land = rows.iter().find(|r| r.what == Do::Land).unwrap();
    assert!(!land.enabled);
    assert!(!land.detail.is_empty());
}

#[test]
fn a_disabled_action_says_why_and_never_runs() {
    let snap = evening();
    let done = outcome(&snap, "mission:fix-invoice");
    let rows = rows_for(&snap, done);
    let land = rows.iter().find(|r| r.what == Do::Land).unwrap();
    assert!(!land.enabled);
    assert_eq!(land.detail, "the work is delivered");
    let evidence = rows.iter().find(|r| r.what == Do::Evidence).unwrap();
    assert!(evidence.enabled);
    assert!(evidence.detail.contains("bundle"), "{}", evidence.detail);
    // Nothing the product cannot do is offered: no send back, pause or
    // merge to main.
    for r in &rows {
        assert!(
            !["send back", "pause", "merge"].contains(&r.label),
            "{}",
            r.label
        );
    }
}

#[test]
fn landing_is_offered_with_what_it_does_when_the_read_model_enables_it() {
    let mut snap = evening();
    let o = snap
        .groups
        .iter_mut()
        .flat_map(|g| &mut g.outcomes)
        .find(|o| o.id == "mission:fix-invoice")
        .unwrap();
    let land = o
        .actions
        .iter_mut()
        .find(|a| a.kind == ActionKind::LandToIntegration)
        .unwrap();
    land.enabled = true;
    land.reason = None;
    let rows = rows_for(&snap, outcome(&snap, "mission:fix-invoice"));
    let land = rows.iter().find(|r| r.what == Do::Land).unwrap();
    assert!(land.enabled);
    assert_eq!(land.action, LAND);
}

#[test]
fn what_an_outcome_holds_up_comes_from_the_unmet_edges_of_the_deck() {
    let mut snap = evening();
    let card = |id: &str, outcome: &str, title: &str| DeckNode {
        id: id.into(),
        kind: NodeKind::Job,
        outcome: outcome.into(),
        title: title.into(),
        repo: None,
        state: NodeState::Waiting,
        group: None,
        checks: swarmux_application::console::CheckProgress {
            total: 0,
            passed: 0,
            failed: 0,
        },
        depth: 0,
        spend_usd: 0.0,
        job: None,
    };
    snap.deck.nodes.push(card("job:90", "job:2", "the push"));
    snap.deck.nodes.push(card("job:91", "job:3", "the release"));
    snap.deck.nodes.push(card("job:92", "job:4", "the docs"));
    let edge = |from: &str, to: &str, state| swarmux_application::console::DeckEdge {
        from: from.into(),
        to: to.into(),
        kind: EdgeKind::TaskDependency,
        state,
        detail: Vec::new(),
    };
    snap.deck
        .edges
        .push(edge("job:90", "job:91", DepState::Pending));
    snap.deck
        .edges
        .push(edge("job:92", "job:90", DepState::Blocked));
    // A met edge holds nothing up.
    snap.deck
        .edges
        .push(edge("job:90", "job:92", DepState::Met));

    let rel = relations(&snap, outcome(&snap, "job:2"));
    let blocks: Vec<(&str, DepState)> = rel
        .blocks
        .iter()
        .map(|r| (r.title.as_str(), r.state))
        .collect();
    let waits: Vec<(&str, DepState)> = rel
        .waits_on
        .iter()
        .map(|r| (r.title.as_str(), r.state))
        .collect();
    assert!(
        blocks.contains(&("the release", DepState::Pending)),
        "{blocks:?}"
    );
    assert!(
        waits.contains(&("the docs", DepState::Blocked)),
        "{waits:?}"
    );
    // The met edge from the push to the docs holds nothing up.
    assert!(!blocks.iter().any(|b| b.0 == "the docs"), "{blocks:?}");
    assert_eq!(edge_label(EdgeKind::TaskDependency), "task dependency");
    // An outcome nothing relates to has no relations.
    assert_eq!(
        relations(&snap, outcome(&snap, "job:6")),
        Relations::default()
    );
}

#[test]
fn holds_are_the_waiting_ones_first_then_the_newest_refusal() {
    let snap = evening();
    let holds = holds_of(&snap, outcome(&snap, "job:2"));
    assert_eq!(holds.len(), 2);
    assert_eq!(holds[0].state, HoldState::Held);
    assert_eq!(holds[1].state, HoldState::Denied);
    assert!(holds_of(&snap, outcome(&snap, "job:6")).is_empty());
}

#[test]
fn a_countdown_reads_minutes_and_seconds_and_stops_at_zero() {
    assert_eq!(countdown(10_000 + 570_000, 10_000), "9:30");
    assert_eq!(countdown(10_000 + 9_001, 10_000), "0:10");
    assert_eq!(countdown(5_000, 10_000), "0:00");
}

#[test]
fn the_screen_is_built_from_semantic_roles_and_plain_glyphs() {
    // No literal colours and no private-use (Nerd Font) glyphs in the code
    // of the Work screen: colours come from the theme's roles, marks from
    // geometric shapes.
    let sources = [
        include_str!("../work.rs"),
        include_str!("../work_actions.rs"),
        include_str!("../screens/work.rs"),
        include_str!("../ui/work/mod.rs"),
        include_str!("../ui/work/queue.rs"),
        include_str!("../ui/work/right.rs"),
        include_str!("../ui/work/product.rs"),
        include_str!("../ui/work/util.rs"),
    ];
    for (i, src) in sources.iter().enumerate() {
        for banned in ["Color::Rgb", "Color::Indexed", "Color::Red", "Color::Green"] {
            assert!(!src.contains(banned), "file {i} names {banned}");
        }
        let private_use = src.chars().find(|c| ('\u{E000}'..='\u{F8FF}').contains(c));
        assert_eq!(private_use, None, "file {i} has a private-use glyph");
    }
}

#[test]
fn until_the_user_chooses_the_top_of_the_queue_is_what_work_opens_on() {
    let mut snap = evening();
    let mut w = WorkScreen::default();
    w.settle(&snap);
    let first = ids(&snap)[0].to_string();
    assert_eq!(w.selected.as_deref(), Some(first.as_str()));
    // A more urgent decision arrives (the queue is re-ranked): an
    // untouched Work follows the top, as it should.
    snap.groups[0].outcomes.reverse();
    w.settle(&snap);
    let new_top = ids(&snap)[0].to_string();
    assert_ne!(new_top, first);
    assert_eq!(w.selected.as_deref(), Some(new_top.as_str()));
    // Once the user moves, their choice is kept.
    w.step(&snap, 1);
    let chosen = w.selected.clone().unwrap();
    snap.groups[0].outcomes.reverse();
    w.settle(&snap);
    assert_eq!(w.selected, Some(chosen));
}
