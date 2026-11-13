//! The Deck's state and what it derives from the console snapshot: the
//! graph laid out, the selected card, and the relations of a card in words.
//!
//! The data is the read model's `DeckGraph`; nothing is added to it. The
//! layout is pure (`layout`) and redone from the snapshot each time it is
//! needed (a few hundred cards at most), so it cannot go stale.

pub mod layout;

use std::cell::Cell;
use std::collections::HashMap;

use swarmux_application::console::{
    ConsoleSnapshot, DeckGraph, DeckNode, DepState, EdgeKind, NodeKind,
};
use swarmux_domain::console::Panel;

use layout::{InEdge, InNode, Input, Layered};

/// Where the keys go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Graph,
    Sessions,
}

/// Which side of a card `h` and `l` move to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toward {
    /// The cards it waits for.
    Producer,
    /// The cards that wait for it.
    Consumer,
}

/// What the Deck remembers between frames.
#[derive(Default)]
pub struct DeckScreen {
    /// The selected card, by id: the graph changes as work moves, so the
    /// selection follows the card.
    pub selected: Option<String>,
    pub focus: Focus,
    /// The selected row of the strip of sessions.
    pub session_at: usize,
    /// At a size that folds panels: the one shown in place of the graph.
    /// `None` is the graph.
    pub folded: Option<Panel>,
    /// Rows the graph is scrolled down, as drawn last.
    pub scroll: Cell<u16>,
    /// Rows of the list form scrolled, as drawn last.
    pub list_scroll: Cell<usize>,
}

/// The graph of a snapshot, laid out.
pub struct Plan {
    pub input: Input,
    pub layered: Layered,
    /// The order `j` and `k` walk.
    pub order: Vec<usize>,
}

impl Plan {
    pub fn of(snap: &ConsoleSnapshot) -> Plan {
        let input = input_of(snap);
        let layered = layout::layered(&input);
        let order = layout::reading_order(&layered);
        Plan {
            input,
            layered,
            order,
        }
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.input.nodes.iter().position(|n| n.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.input.nodes.is_empty()
    }
}

/// The layout's input for `snap`: cards grouped by where their outcome
/// sits in the queue (the one needing the user first).
pub fn input_of(snap: &ConsoleSnapshot) -> Input {
    let queue: HashMap<&str, usize> = snap
        .groups
        .iter()
        .flat_map(|g| &g.outcomes)
        .enumerate()
        .map(|(i, o)| (o.id.as_str(), i))
        .collect();
    let off_queue = queue.len();
    let graph = &snap.deck;
    let nodes = graph
        .nodes
        .iter()
        .map(|n| InNode {
            id: n.id.clone(),
            depth: n.depth,
            // Cards of no queue come after the queued ones, in card order.
            group: queue.get(n.outcome.as_str()).copied().unwrap_or(off_queue),
        })
        .collect();
    let index: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let edges = graph
        .edges
        .iter()
        .filter_map(|e| {
            Some(InEdge {
                from: *index.get(e.from.as_str())?,
                to: *index.get(e.to.as_str())?,
                kind: e.kind,
                state: e.state,
            })
        })
        .collect();
    Input { nodes, edges }
}

impl DeckScreen {
    /// The selected card; the first one when nothing was chosen or the
    /// chosen one is gone.
    pub fn current(&self, plan: &Plan) -> Option<usize> {
        self.selected
            .as_deref()
            .and_then(|id| plan.index_of(id))
            .or_else(|| plan.order.first().copied())
    }

    /// Moves by `delta` cards in reading order, stopping at the ends.
    pub fn step(&mut self, plan: &Plan, delta: isize) {
        let Some(now) = self.current(plan) else {
            return;
        };
        let at = plan.order.iter().position(|n| *n == now).unwrap_or(0);
        let to = at
            .saturating_add_signed(delta)
            .min(plan.order.len().saturating_sub(1));
        self.select(plan, plan.order[to]);
    }

    /// Goes to a card joined to the selected one: a producer or a consumer.
    /// The first, in column order; false when there is none.
    pub fn toward(&mut self, plan: &Plan, side: Toward) -> bool {
        let Some(now) = self.current(plan) else {
            return false;
        };
        let (before, after) = layout::neighbours(&plan.layered, now);
        let found = match side {
            Toward::Producer => before,
            Toward::Consumer => after,
        };
        match found.first() {
            Some(n) => {
                self.select(plan, *n);
                true
            }
            None => false,
        }
    }

    pub fn select(&mut self, plan: &Plan, node: usize) {
        self.selected = plan.input.nodes.get(node).map(|n| n.id.clone());
    }
}

/// How a card is named in a line of text: a task by its id in the plan,
/// a job by its number.
pub fn label(node: &DeckNode) -> String {
    match (node.kind, node.job) {
        (NodeKind::Task, _) => node.id.rsplit(':').next().unwrap_or(&node.id).to_string(),
        (NodeKind::Job, Some(job)) => format!("#{job}"),
        (NodeKind::Job, None) => node.title.chars().take(12).collect(),
    }
}

/// How a card relates to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    /// The other card's id.
    pub other: String,
    pub kind: EdgeKind,
    pub state: DepState,
    pub role: Role,
    /// The symbols or areas it is about.
    pub detail: Vec<String>,
    /// Why the line is not drawn between the columns, when it is not: it
    /// is left to words (a cycle that was broken, two cards of one column).
    pub note: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The card feeds the other.
    Feeds,
    /// The card waits for the other.
    Needs,
    /// The card holds files the other waits for.
    Holds,
    /// The card waits for the other to give up files.
    Waits,
}

/// Every edge of `node`, from `snap`'s graph.
pub fn relations(snap: &ConsoleSnapshot, plan: &Plan, node: usize) -> Vec<Relation> {
    let graph: &DeckGraph = &snap.deck;
    let Some(me) = graph.nodes.get(node) else {
        return Vec::new();
    };
    let note_of = |i: usize| {
        if plan.layered.back.contains(&i) {
            Some("cycle, not drawn")
        } else if plan.layered.lateral.contains(&i) {
            Some("same column, not drawn")
        } else {
            None
        }
    };
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from == me.id || e.to == me.id)
        .map(|(i, e)| {
            let outgoing = e.from == me.id;
            let other = if outgoing { &e.to } else { &e.from };
            let role = match (e.kind, outgoing) {
                (EdgeKind::FileClaim, true) => Role::Holds,
                (EdgeKind::FileClaim, false) => Role::Waits,
                (_, true) => Role::Feeds,
                (_, false) => Role::Needs,
            };
            Relation {
                other: other.clone(),
                kind: e.kind,
                state: e.state,
                role,
                detail: e.detail.clone(),
                note: note_of(i),
            }
        })
        .collect()
}

/// The words for an edge kind in a relation.
pub fn kind_words(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::TaskDependency => "dep",
        EdgeKind::ContractLink => "contract",
        EdgeKind::FileClaim => "claim",
    }
}

pub fn state_words(state: DepState) -> &'static str {
    match state {
        DepState::Met => "met",
        DepState::Pending => "pending",
        DepState::Blocked => "blocked",
    }
}

/// One relation as a phrase: `← t1 (dep, met)`, `→ #4 (contract orders,
/// pending)`, `holds src/** · #4 waits`.
pub fn phrase(snap: &ConsoleSnapshot, r: &Relation) -> String {
    let other = snap
        .deck
        .nodes
        .iter()
        .find(|n| n.id == r.other)
        .map_or_else(|| r.other.clone(), label);
    let about = if r.detail.is_empty() {
        String::new()
    } else {
        format!(" {}", r.detail.join(", "))
    };
    let kind = kind_words(r.kind);
    let state = state_words(r.state);
    match r.role {
        Role::Needs => format!("← {other} ({kind}{about}, {state})"),
        Role::Feeds => format!("→ {other} ({kind}{about}, {state})"),
        Role::Holds => format!("holds{about} · {other} waits"),
        Role::Waits => format!(
            "waits for {other}{}",
            if about.is_empty() {
                String::new()
            } else {
                format!(" to release{about}")
            }
        ),
    }
}

/// Whether `node`'s outcome has a call held for the user.
pub fn is_held(snap: &ConsoleSnapshot, node: &DeckNode) -> bool {
    snap.holds.iter().any(|h| {
        h.state == swarmux_application::console::HoldState::Held
            && h.outcome.as_deref() == Some(node.outcome.as_str())
            && (node.job.is_none() || h.job == node.job)
    })
}

#[cfg(test)]
mod tests;
