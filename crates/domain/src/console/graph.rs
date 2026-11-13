//! The Deck's graph: the typed edges between the cards, and each card's
//! depth from producer to consumer. No layout: positions are a client's
//! business.
//!
//! Three kinds of edge exist, each derived from data swarmux keeps:
//!
//! - **Task dependency**: a plan's task depends on another (`depends_on`).
//!   Met once the producer merged, blocked when it needs the user (or
//!   waits for something that does), pending otherwise.
//! - **Contract link**: a repository provides a symbol (a table, an
//!   endpoint, a function, …) that another repository uses, and the
//!   cards work in those two repositories. Met once the provider's work
//!   is done, blocked when it failed, pending otherwise.
//! - **File claim**: two cards claim overlapping files (a plan's task
//!   owns its `files`; an agent's live claim on the board). The edge is
//!   symmetric and says who holds the files first. Overlap is judged
//!   from the claims (areas, globs) only: what the agents really changed
//!   is not known until they are done, so it is not guessed.
//!
//! Task and contract edges point from producer to consumer and give the
//! cards a depth. File claims do not: two cards that contend for files are
//! not ordered by them.

use std::collections::{BTreeMap, BTreeSet};

use crate::plan::Plan;
use crate::taskgraph::{areas_overlap, claims_overlap};

/// Most symbols or areas kept on one edge.
const DETAIL_MAX: usize = 5;

/// How far a card's work has got, as far as its neighbours care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Has not started: it waits for something.
    Waiting,
    /// A job is out for it, or it is being verified or landed.
    Working,
    /// Merged or delivered: what it produced is there to use.
    Done,
    /// It needs the user.
    Failed,
}

/// What kind of relation an edge is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    TaskDependency,
    ContractLink,
    FileClaim,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::TaskDependency => "task_dependency",
            EdgeKind::ContractLink => "contract_link",
            EdgeKind::FileClaim => "file_claim",
        }
    }
}

/// Where an edge stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepState {
    /// What the consumer needs is there.
    Met,
    /// It is not there yet, and nothing stops it coming.
    Pending,
    /// It will not come without the user.
    Blocked,
}

impl DepState {
    pub fn as_str(self) -> &'static str {
        match self {
            DepState::Met => "met",
            DepState::Pending => "pending",
            DepState::Blocked => "blocked",
        }
    }
}

/// A relation between two cards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// The producer (or, for a file claim, the card holding the files).
    pub from: String,
    /// The consumer (or, for a file claim, the card kept waiting).
    pub to: String,
    pub kind: EdgeKind,
    pub state: DepState,
    /// What it is about: the contract's symbols, the claimed areas. Empty
    /// for a task dependency.
    pub detail: Vec<String>,
}

/// A card, as far as edges are concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    /// The repositories it works in; empty when unknown.
    pub repos: Vec<String>,
    /// The files it claims; empty when it claims none.
    pub claims: Vec<String>,
    pub progress: Progress,
}

/// A repository that provides symbols another uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractFlow {
    pub provider: String,
    pub consumer: String,
    /// The symbols, as the contracts name them (`table orders`).
    pub symbols: Vec<String>,
}

fn dep_state(p: Progress) -> DepState {
    match p {
        Progress::Done => DepState::Met,
        Progress::Failed => DepState::Blocked,
        Progress::Waiting | Progress::Working => DepState::Pending,
    }
}

/// The dependency edges of a plan's tasks. `node_of` names the card of a
/// task (by its id in the plan) and `progress_of` says how far it is.
///
/// An edge is blocked when its producer needs the user, and also when the
/// producer is still waiting on something that is blocked: the whole
/// chain stands still.
pub fn task_edges(
    plan: &Plan,
    node_of: &dyn Fn(&str) -> String,
    progress_of: &dyn Fn(&str) -> Progress,
) -> Vec<Edge> {
    // Tasks that cannot go on: failed, or waiting on one that cannot.
    let mut stuck: BTreeSet<&str> = plan
        .tasks
        .iter()
        .filter(|t| progress_of(&t.id) == Progress::Failed)
        .map(|t| t.id.as_str())
        .collect();
    loop {
        let before = stuck.len();
        for t in &plan.tasks {
            if progress_of(&t.id) == Progress::Waiting
                && t.depends_on.iter().any(|d| stuck.contains(d.as_str()))
            {
                stuck.insert(t.id.as_str());
            }
        }
        if stuck.len() == before {
            break;
        }
    }
    let mut out = Vec::new();
    for t in &plan.tasks {
        for d in &t.depends_on {
            let state = if progress_of(d) == Progress::Done {
                DepState::Met
            } else if stuck.contains(d.as_str()) {
                DepState::Blocked
            } else {
                DepState::Pending
            };
            out.push(Edge {
                from: node_of(d),
                to: node_of(&t.id),
                kind: EdgeKind::TaskDependency,
                state,
                detail: Vec::new(),
            });
        }
    }
    out.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    out
}

/// The contract edges between cards: from every card working in a
/// providing repository to every other card working in a consuming one,
/// with the symbols that link them. Two cards in the same repository are
/// never linked by it.
pub fn contract_edges(nodes: &[Node], flows: &[ContractFlow]) -> Vec<Edge> {
    let mut links: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for flow in flows {
        if flow.provider == flow.consumer {
            continue;
        }
        for a in nodes.iter().filter(|n| n.repos.contains(&flow.provider)) {
            for b in nodes.iter().filter(|n| n.repos.contains(&flow.consumer)) {
                if a.id == b.id {
                    continue;
                }
                links
                    .entry((a.id.clone(), b.id.clone()))
                    .or_default()
                    .extend(flow.symbols.iter().cloned());
            }
        }
    }
    links
        .into_iter()
        .map(|((from, to), symbols)| {
            let progress = nodes
                .iter()
                .find(|n| n.id == from)
                .map_or(Progress::Waiting, |n| n.progress);
            Edge {
                from,
                to,
                kind: EdgeKind::ContractLink,
                state: dep_state(progress),
                detail: symbols.into_iter().take(DETAIL_MAX).collect(),
            }
        })
        .collect()
}

/// The claim edges: pairs of cards still to do their work whose claims
/// overlap. The card that already works holds the files and the other
/// waits (blocked); two cards in the same state are ordered by id and
/// pending. Cards in different repositories never contend, and a card
/// with no claim contends for nothing.
pub fn claim_edges(nodes: &[Node]) -> Vec<Edge> {
    let mut sorted: Vec<&Node> = nodes
        .iter()
        .filter(|n| matches!(n.progress, Progress::Waiting | Progress::Working))
        .filter(|n| !n.claims.is_empty())
        .collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut out = Vec::new();
    for (i, a) in sorted.iter().enumerate() {
        for b in &sorted[i + 1..] {
            let apart = !a.repos.is_empty()
                && !b.repos.is_empty()
                && !a.repos.iter().any(|r| b.repos.contains(r));
            if apart || !claims_overlap(&a.claims, &b.claims) {
                continue;
            }
            let (from, to, state) = match (a.progress, b.progress) {
                (Progress::Working, Progress::Waiting) => (a, b, DepState::Blocked),
                (Progress::Waiting, Progress::Working) => (b, a, DepState::Blocked),
                _ => (a, b, DepState::Pending),
            };
            let shared: Vec<String> = from
                .claims
                .iter()
                .filter(|x| to.claims.iter().any(|y| areas_overlap(x, y)))
                .take(DETAIL_MAX)
                .cloned()
                .collect();
            out.push(Edge {
                from: from.id.clone(),
                to: to.id.clone(),
                kind: EdgeKind::FileClaim,
                state,
                detail: shared,
            });
        }
    }
    out
}

/// Each card's depth: how many cards of producers sit between it and the
/// start of its chain (a card nothing precedes is 0). Only task and
/// contract edges count. Contracts can point both ways between two
/// repositories; edges are taken in order and one that would close a
/// cycle is left out, so the result is the same every time.
pub fn depths(ids: &[String], edges: &[Edge]) -> BTreeMap<String, u32> {
    let known: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let mut ordered: Vec<(&str, &str)> = edges
        .iter()
        .filter(|e| e.kind != EdgeKind::FileClaim)
        .filter(|e| e.from != e.to)
        .filter(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str()))
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    ordered.sort_unstable();
    ordered.dedup();
    let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in ordered {
        if !reaches(&next, to, from) {
            next.entry(from).or_default().push(to);
        }
    }
    // The graph is acyclic now: longest path from a source, by relaxing
    // in topological order.
    let mut indegree: BTreeMap<&str, usize> = known.iter().map(|id| (*id, 0)).collect();
    for to in next.values().flatten().copied() {
        *indegree.entry(to).or_default() += 1;
    }
    let mut depth: BTreeMap<&str, u32> = known.iter().map(|id| (*id, 0)).collect();
    let mut ready: Vec<&str> = indegree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| *id)
        .collect();
    while let Some(id) = ready.pop() {
        let here = depth[id];
        for to in next.get(id).into_iter().flatten().copied() {
            let d = depth.entry(to).or_default();
            *d = (*d).max(here + 1);
            let n = indegree.entry(to).or_default();
            *n -= 1;
            if *n == 0 {
                ready.push(to);
            }
        }
    }
    depth.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Whether `target` can be reached from `start` along `next`.
fn reaches(next: &BTreeMap<&str, Vec<&str>>, start: &str, target: &str) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if id == target {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        stack.extend(next.get(id).into_iter().flatten().copied());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlanTask;

    fn task(id: &str, files: &[&str], after: &[&str]) -> PlanTask {
        PlanTask {
            id: id.into(),
            title: id.into(),
            files: files.iter().map(|s| s.to_string()).collect(),
            depends_on: after.iter().map(|s| s.to_string()).collect(),
            acceptance: String::new(),
        }
    }

    fn plan(tasks: Vec<PlanTask>) -> Plan {
        Plan {
            summary: String::new(),
            tasks,
        }
    }

    fn node(id: &str, repos: &[&str], claims: &[&str], progress: Progress) -> Node {
        Node {
            id: id.into(),
            repos: repos.iter().map(|s| s.to_string()).collect(),
            claims: claims.iter().map(|s| s.to_string()).collect(),
            progress,
        }
    }

    fn state_of(edges: &[Edge], from: &str, to: &str) -> DepState {
        edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("no edge {from} -> {to}"))
            .state
    }

    fn names(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn task_dependencies_are_met_pending_or_blocked() {
        let p = plan(vec![
            task("api", &["api/**"], &[]),
            task("db", &["db/**"], &[]),
            task("web", &["web/**"], &["api", "db"]),
        ]);
        let progress = |id: &str| match id {
            "api" => Progress::Done,
            "db" => Progress::Working,
            _ => Progress::Waiting,
        };
        let edges = task_edges(&p, &|id| format!("task:{id}"), &progress);
        assert_eq!(edges.len(), 2);
        assert_eq!(state_of(&edges, "task:api", "task:web"), DepState::Met);
        assert_eq!(state_of(&edges, "task:db", "task:web"), DepState::Pending);
        assert!(edges.iter().all(|e| e.kind == EdgeKind::TaskDependency));
    }

    #[test]
    fn a_failed_producer_blocks_the_whole_chain() {
        let p = plan(vec![
            task("a", &["a"], &[]),
            task("b", &["b"], &["a"]),
            task("c", &["c"], &["b"]),
            task("d", &["d"], &[]),
        ]);
        let progress = |id: &str| match id {
            "a" => Progress::Failed,
            "d" => Progress::Done,
            _ => Progress::Waiting,
        };
        let edges = task_edges(&p, &|id| id.to_string(), &progress);
        assert_eq!(state_of(&edges, "a", "b"), DepState::Blocked);
        // b has not started and cannot: its edge to c is blocked too.
        assert_eq!(state_of(&edges, "b", "c"), DepState::Blocked);
    }

    #[test]
    fn a_working_consumer_behind_a_met_edge_is_not_blocked() {
        let p = plan(vec![task("a", &["a"], &[]), task("b", &["b"], &["a"])]);
        let progress = |id: &str| {
            if id == "a" {
                Progress::Done
            } else {
                Progress::Working
            }
        };
        let edges = task_edges(&p, &|id| id.to_string(), &progress);
        assert_eq!(state_of(&edges, "a", "b"), DepState::Met);
    }

    #[test]
    fn contracts_link_cards_in_the_providing_and_consuming_repositories() {
        let nodes = [
            node("backend", &["api"], &[], Progress::Working),
            node("app", &["mobile"], &[], Progress::Waiting),
            node("docs", &["docs"], &[], Progress::Waiting),
        ];
        let flows = [ContractFlow {
            provider: "api".into(),
            consumer: "mobile".into(),
            symbols: names(&["table orders", "GET /api/orders"]),
        }];
        let edges = contract_edges(&nodes, &flows);
        assert_eq!(edges.len(), 1);
        let e = &edges[0];
        assert_eq!((e.from.as_str(), e.to.as_str()), ("backend", "app"));
        assert_eq!(e.kind, EdgeKind::ContractLink);
        assert_eq!(e.state, DepState::Pending);
        assert_eq!(e.detail, ["GET /api/orders", "table orders"]);
    }

    #[test]
    fn a_contract_edge_follows_its_providers_progress() {
        let flows = [ContractFlow {
            provider: "api".into(),
            consumer: "mobile".into(),
            symbols: names(&["table orders"]),
        }];
        let at = |p: Progress| {
            contract_edges(
                &[
                    node("backend", &["api"], &[], p),
                    node("app", &["mobile"], &[], Progress::Waiting),
                ],
                &flows,
            )[0]
            .state
        };
        assert_eq!(at(Progress::Done), DepState::Met);
        assert_eq!(at(Progress::Failed), DepState::Blocked);
        assert_eq!(at(Progress::Working), DepState::Pending);
        assert_eq!(at(Progress::Waiting), DepState::Pending);
    }

    #[test]
    fn one_card_in_both_repositories_links_to_nothing_but_others() {
        let nodes = [node("both", &["api", "mobile"], &[], Progress::Working)];
        let flows = [ContractFlow {
            provider: "api".into(),
            consumer: "mobile".into(),
            symbols: names(&["table orders"]),
        }];
        assert!(contract_edges(&nodes, &flows).is_empty());
        let same = [ContractFlow {
            provider: "api".into(),
            consumer: "api".into(),
            symbols: names(&["x"]),
        }];
        assert!(contract_edges(&[node("a", &["api"], &[], Progress::Done)], &same).is_empty());
    }

    #[test]
    fn overlapping_claims_make_a_claim_edge_held_first() {
        let nodes = [
            node("a", &["app"], &["src/**"], Progress::Waiting),
            node("b", &["app"], &["src/checkout/pay.rs"], Progress::Working),
            node("c", &["app"], &["docs/**"], Progress::Working),
        ];
        let edges = claim_edges(&nodes);
        assert_eq!(edges.len(), 1);
        let e = &edges[0];
        // b already works in src/, so it holds the files and a waits.
        assert_eq!((e.from.as_str(), e.to.as_str()), ("b", "a"));
        assert_eq!(e.kind, EdgeKind::FileClaim);
        assert_eq!(e.state, DepState::Blocked);
        assert_eq!(e.detail, ["src/checkout/pay.rs"]);
    }

    #[test]
    fn claims_in_other_repositories_or_of_finished_cards_do_not_contend() {
        let nodes = [
            node("a", &["app"], &["src/**"], Progress::Working),
            node("b", &["api"], &["src/**"], Progress::Working),
            node("c", &["app"], &["src/**"], Progress::Done),
            node("d", &["app"], &[], Progress::Working),
        ];
        assert!(claim_edges(&nodes).is_empty());
    }

    #[test]
    fn two_cards_in_the_same_state_are_ordered_by_id_and_pending() {
        let nodes = [
            node("z", &[], &["lib/"], Progress::Working),
            node("y", &[], &["lib/a.rs"], Progress::Working),
        ];
        let edges = claim_edges(&nodes);
        assert_eq!(edges.len(), 1);
        assert_eq!((edges[0].from.as_str(), edges[0].to.as_str()), ("y", "z"));
        assert_eq!(edges[0].state, DepState::Pending);
    }

    fn dep(from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            kind,
            state: DepState::Pending,
            detail: Vec::new(),
        }
    }

    #[test]
    fn depth_is_the_longest_chain_of_producers() {
        let ids = names(&["a", "b", "c", "d", "lone"]);
        let edges = [
            dep("a", "b", EdgeKind::TaskDependency),
            dep("b", "c", EdgeKind::TaskDependency),
            dep("a", "c", EdgeKind::ContractLink),
            dep("a", "d", EdgeKind::TaskDependency),
        ];
        let d = depths(&ids, &edges);
        assert_eq!(d["a"], 0);
        assert_eq!(d["b"], 1);
        assert_eq!(d["c"], 2);
        assert_eq!(d["d"], 1);
        assert_eq!(d["lone"], 0);
    }

    #[test]
    fn claim_edges_do_not_give_depth() {
        let ids = names(&["a", "b"]);
        let d = depths(&ids, &[dep("a", "b", EdgeKind::FileClaim)]);
        assert_eq!((d["a"], d["b"]), (0, 0));
    }

    #[test]
    fn a_cycle_of_contracts_is_broken_the_same_way_every_time() {
        let ids = names(&["a", "b", "c"]);
        let edges = [
            dep("b", "a", EdgeKind::ContractLink),
            dep("a", "b", EdgeKind::ContractLink),
            dep("b", "c", EdgeKind::ContractLink),
            dep("c", "a", EdgeKind::ContractLink),
        ];
        let first = depths(&ids, &edges);
        let mut shuffled = edges.to_vec();
        shuffled.reverse();
        assert_eq!(first, depths(&ids, &shuffled));
        // a -> b is taken first (sorted), b -> a would close a cycle.
        assert_eq!(first["a"], 0);
        assert_eq!(first["b"], 1);
        // b -> c fits; c -> a would close the cycle a -> b -> c -> a.
        assert_eq!(first["c"], 2);
    }

    #[test]
    fn unknown_cards_and_self_edges_are_ignored() {
        let ids = names(&["a"]);
        let d = depths(
            &ids,
            &[
                dep("a", "a", EdgeKind::TaskDependency),
                dep("a", "ghost", EdgeKind::TaskDependency),
            ],
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d["a"], 0);
    }
}
