//! The Deck's choices over a console snapshot: the plan, the selection that
//! follows the card, moving along edges, and the relations in words.

use swarmux_application::console::{ConsoleSnapshot, DepState, EdgeKind};

use super::*;
use crate::work::tests::evening;

fn plan(snap: &ConsoleSnapshot) -> Plan {
    Plan::of(snap)
}

fn id_of(snap: &ConsoleSnapshot, plan: &Plan, node: usize) -> String {
    snap.deck.nodes[node].id.clone()
        + &plan.layered.column[node].map_or(String::new(), |c| format!("@{c}"))
}

#[test]
fn the_plan_has_a_card_for_every_node_and_a_column_for_the_linked_ones() {
    let snap = evening();
    let p = plan(&snap);
    assert_eq!(p.input.nodes.len(), snap.deck.nodes.len());
    let t1 = p.index_of("task:split-app:t1").unwrap();
    let t2 = p.index_of("task:split-app:t2").unwrap();
    let t3 = p.index_of("task:split-app:t3").unwrap();
    // The producers left, the consumer right.
    assert_eq!(p.layered.column[t1], Some(0));
    assert_eq!(p.layered.column[t2], Some(0));
    assert_eq!(p.layered.column[t3], Some(1));
    // The file claim puts the holder (job 2) left of the waiter (job 4).
    let j2 = p.index_of("job:2").unwrap();
    let j4 = p.index_of("job:4").unwrap();
    assert!(p.layered.column[j2] < p.layered.column[j4]);
    // Cards with no edge are on the shelf.
    let quiet = p.index_of("job:5").unwrap();
    assert!(p.layered.shelf.contains(&quiet));
    // Reading order visits every card once.
    let mut order = p.order.clone();
    order.sort_unstable();
    assert_eq!(order, (0..snap.deck.nodes.len()).collect::<Vec<_>>());
}

#[test]
fn cards_of_one_outcome_stay_together_and_the_queue_order_decides_between_outcomes() {
    let snap = evening();
    let p = plan(&snap);
    // The first outcome of the queue is the held push (job 2): its card is
    // the first the shelf or the columns list in its column.
    let first_outcome = &snap.groups[0].outcomes[0].id;
    let group_of = |id: &str| p.input.nodes[p.index_of(id).unwrap()].group;
    assert_eq!(group_of(first_outcome), 0);
    // A card on no queue comes after every queued one.
    let off = group_of("job:5");
    assert!(
        snap.deck
            .nodes
            .iter()
            .filter(|n| n.group.is_some())
            .all(|n| group_of(&n.id) < off)
    );
}

#[test]
fn nothing_is_chosen_until_the_user_moves_and_then_the_card_is_followed_by_id() {
    let snap = evening();
    let p = plan(&snap);
    let mut d = DeckScreen::default();
    let first = *p.order.first().unwrap();
    assert_eq!(d.current(&p), Some(first));
    d.step(&p, 1);
    let second = p.order[1];
    assert_eq!(d.current(&p), Some(second));
    assert_eq!(
        d.selected.as_deref(),
        Some(snap.deck.nodes[second].id.as_str())
    );
    // A card that is gone: the first one is selected again.
    d.selected = Some("job:404".into());
    assert_eq!(d.current(&p), Some(first));
}

#[test]
fn stepping_stops_at_both_ends() {
    let snap = evening();
    let p = plan(&snap);
    let mut d = DeckScreen::default();
    d.step(&p, -1);
    assert_eq!(d.current(&p), p.order.first().copied());
    for _ in 0..p.order.len() + 3 {
        d.step(&p, 1);
    }
    assert_eq!(d.current(&p), p.order.last().copied());
    // Nothing to select: nothing happens.
    let empty = Plan::of(&ConsoleSnapshot {
        deck: swarmux_application::console::DeckGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            max_depth: 0,
        },
        ..snap
    });
    let mut d = DeckScreen::default();
    d.step(&empty, 1);
    assert_eq!(d.current(&empty), None);
    assert!(!d.toward(&empty, Toward::Producer));
}

#[test]
fn the_edge_keys_go_to_a_producer_or_a_consumer_and_not_where_there_is_none() {
    let snap = evening();
    let p = plan(&snap);
    let mut d = DeckScreen::default();
    let t3 = p.index_of("task:split-app:t3").unwrap();
    d.select(&p, t3);
    // t3 waits for t1 and t2: the first producer in column order.
    assert!(d.toward(&p, Toward::Producer));
    let went = d.selected.clone().unwrap();
    assert!(
        went == "task:split-app:t1" || went == "task:split-app:t2",
        "{went}"
    );
    // Nothing waits for t3.
    d.select(&p, t3);
    assert!(!d.toward(&p, Toward::Consumer));
    assert_eq!(d.selected.as_deref(), Some("task:split-app:t3"), "stayed");
    // t1 feeds t3.
    let t1 = p.index_of("task:split-app:t1").unwrap();
    d.select(&p, t1);
    assert!(d.toward(&p, Toward::Consumer));
    assert_eq!(d.selected.as_deref(), Some("task:split-app:t3"));
    // A card with no edge has no neighbour either way.
    let quiet = p.index_of("job:5").unwrap();
    d.select(&p, quiet);
    assert!(!d.toward(&p, Toward::Producer) && !d.toward(&p, Toward::Consumer));
}

#[test]
fn relations_list_every_edge_of_a_card_with_its_role() {
    let snap = evening();
    let p = plan(&snap);
    let t3 = p.index_of("task:split-app:t3").unwrap();
    let rels = relations(&snap, &p, t3);
    assert_eq!(rels.len(), 2);
    assert!(
        rels.iter()
            .all(|r| r.role == Role::Needs && r.kind == EdgeKind::TaskDependency)
    );
    let states: Vec<DepState> = rels.iter().map(|r| r.state).collect();
    assert!(states.contains(&DepState::Met) && states.contains(&DepState::Blocked));
    assert!(rels.iter().all(|r| r.note.is_none()), "both are drawn");

    let phrases: Vec<String> = rels.iter().map(|r| phrase(&snap, r)).collect();
    assert!(
        phrases.contains(&"← t1 (dep, met)".to_string()),
        "{phrases:?}"
    );
    assert!(
        phrases.contains(&"← t2 (dep, blocked)".to_string()),
        "{phrases:?}"
    );

    // The claim: job 2 holds, job 4 waits.
    let j2 = relations(&snap, &p, p.index_of("job:2").unwrap());
    assert_eq!(phrase(&snap, &j2[0]), "holds src/checkout/** · #4 waits");
    let j4 = relations(&snap, &p, p.index_of("job:4").unwrap());
    assert_eq!(
        phrase(&snap, &j4[0]),
        "waits for #2 to release src/checkout/**"
    );
}

#[test]
fn an_edge_the_picture_leaves_out_says_why_in_words() {
    // Two cards of one column joined by a dependency the depths do not
    // order, and a cycle's closing edge.
    let mut snap = evening();
    let id = |s: &str| format!("task:split-app:{s}");
    // t1 and t2 are both depth 0: a dependency between them is lateral.
    snap.deck
        .edges
        .push(swarmux_application::console::DeckEdge {
            from: id("t1"),
            to: id("t2"),
            kind: EdgeKind::ContractLink,
            state: DepState::Pending,
            detail: vec!["table orders".into()],
        });
    // And one against the columns: t3 (depth 1) back to t1 (depth 0).
    snap.deck
        .edges
        .push(swarmux_application::console::DeckEdge {
            from: id("t3"),
            to: id("t1"),
            kind: EdgeKind::TaskDependency,
            state: DepState::Pending,
            detail: Vec::new(),
        });
    let p = plan(&snap);
    let t1 = relations(&snap, &p, p.index_of(&id("t1")).unwrap());
    let notes: Vec<_> = t1.iter().filter_map(|r| r.note).collect();
    assert!(notes.contains(&"same column, not drawn"), "{notes:?}");
    assert!(notes.contains(&"cycle, not drawn"), "{notes:?}");
    // Nothing panics and every card is placed.
    assert_eq!(p.order.len(), snap.deck.nodes.len());
}

#[test]
fn a_card_is_named_by_its_task_id_or_its_job_number() {
    let snap = evening();
    let name = |id: &str| label(snap.deck.nodes.iter().find(|n| n.id == id).unwrap());
    assert_eq!(name("task:split-app:t2"), "t2");
    assert_eq!(name("job:3"), "#3");
    assert_eq!(
        name("mission:plan-me"),
        "plan me",
        "a mission with no job: its title"
    );
}

#[test]
fn a_held_call_marks_its_card() {
    let snap = evening();
    let held = snap.deck.nodes.iter().find(|n| n.id == "job:2").unwrap();
    assert!(is_held(&snap, held));
    let other = snap.deck.nodes.iter().find(|n| n.id == "job:3").unwrap();
    assert!(!is_held(&snap, other));
}

#[test]
fn the_plan_is_laid_out_the_same_way_each_time() {
    let snap = evening();
    let (a, b) = (plan(&snap), plan(&snap));
    assert_eq!(a.layered, b.layered);
    assert_eq!(a.order, b.order);
    let named: Vec<String> = a.order.iter().map(|n| id_of(&snap, &a, *n)).collect();
    assert_eq!(named.len(), snap.deck.nodes.len());
}
