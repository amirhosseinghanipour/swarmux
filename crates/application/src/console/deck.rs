//! The Deck's graph: a card per job (or per task of a task graph), and
//! the typed edges between them, derived by the domain's rules from the
//! plans' dependencies, the repositories' contracts and the files cards
//! claim.

use std::collections::HashSet;

use swarmux_domain::console as rules;

use super::outcome::Built;
use super::service::{Caches, Sources};
use super::view::{DeckEdge, DeckGraph, GroupKind};

/// The graph of every outcome of the moment, placed or not.
pub(super) fn build(
    src: &Sources<'_>,
    caches: &mut Caches,
    built: &[Built],
    groups: &[Option<GroupKind>],
) -> DeckGraph {
    let mut nodes = Vec::new();
    let mut facts: Vec<rules::Node> = Vec::new();
    for (b, group) in built.iter().zip(groups) {
        for card in &b.cards {
            let mut node = card.node.clone();
            node.group = *group;
            nodes.push(node);
            facts.push(card.facts.clone());
        }
    }

    let mut edges: Vec<rules::Edge> = Vec::new();
    for b in built {
        if let Some(t) = &b.tasks {
            edges.extend(rules::task_edges(
                &t.plan,
                &|task| t.node_of.get(task).cloned().unwrap_or_default(),
                &|task| {
                    t.progress
                        .get(task)
                        .copied()
                        .unwrap_or(rules::Progress::Waiting)
                },
            ));
        }
    }
    edges.extend(rules::contract_edges(&facts, &flows(src, caches)));
    edges.extend(rules::claim_edges(&facts));
    edges.sort_by(|a, b| (a.kind, &a.from, &a.to).cmp(&(b.kind, &b.from, &b.to)));

    let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
    let depth = rules::depths(&ids, &edges);
    for n in &mut nodes {
        n.depth = depth.get(&n.id).copied().unwrap_or(0);
    }
    DeckGraph {
        max_depth: nodes.iter().map(|n| n.depth).max().unwrap_or(0),
        nodes,
        edges: edges
            .into_iter()
            .map(|e| DeckEdge {
                from: e.from,
                to: e.to,
                kind: e.kind.into(),
                state: e.state.into(),
                detail: e.detail,
            })
            .collect(),
    }
}

/// What the repositories of every workspace a mission works in link.
fn flows(src: &Sources<'_>, caches: &mut Caches) -> Vec<rules::ContractFlow> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for ws in src.missions.iter().filter_map(|m| m.workspace) {
        if seen.insert(ws) {
            out.extend(caches.flows(ws, src));
        }
    }
    out
}
