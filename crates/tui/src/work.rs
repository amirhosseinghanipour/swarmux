//! The Work screen's state, and the choices it derives from the console
//! snapshot: which outcome is selected, which tab shows, and the rows of
//! actions the selected outcome offers.
//!
//! Everything here is a function of a [`ConsoleSnapshot`] and the user's
//! last choices, so it is tested without a terminal. The snapshot is the
//! console read model's: this module adds no data of its own.

use std::cell::Cell;

use swarmux_application::console::{
    ActionKind, ActionView, ConsoleSnapshot, DepState, EdgeKind, GroupKind, HoldState, HoldView,
    OutcomeView,
};
use swarmux_domain::Millis;
use swarmux_domain::keys::Action;

use crate::keys::{DECIDE_ALWAYS, DECIDE_NO, DECIDE_YES};
use crate::screens::work::{EVIDENCE, KILL, LAND, RESUME, RETRY};

/// The tabs of the right pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// Plan against drift, the claim against the checks, evidence, what
    /// landing does.
    #[default]
    Product,
    /// The job's action stream.
    Stream,
    /// What changed.
    Diff,
    /// Every check and every stage of the gate.
    Checks,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Product, Tab::Stream, Tab::Diff, Tab::Checks];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Product => "work product",
            Tab::Stream => "stream",
            Tab::Diff => "diff",
            Tab::Checks => "checks",
        }
    }
}

/// The working-tree diff of the selected outcome's session, read when the
/// diff tab opens (it needs git, so never while drawing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRead {
    /// The job it was read for.
    pub job: i64,
    pub lines: Vec<String>,
    /// Why there are no lines.
    pub note: Option<String>,
}

/// What the Work screen remembers between frames.
#[derive(Default)]
pub struct WorkScreen {
    /// The selected outcome, by id: the list re-ranks as work moves, so
    /// the selection follows the outcome and not the row.
    pub selected: Option<String>,
    /// Where the selection was in the flat list, for when the outcome
    /// leaves it.
    at: usize,
    /// The user picked an outcome. Until they do, the top of the queue is
    /// selected, whatever arrives: the most urgent thing is what Work opens on.
    chosen: bool,
    pub tab: Tab,
    /// Rows the right pane is scrolled down.
    pub scroll: usize,
    /// The furthest it can scroll, as drawn last.
    pub max_scroll: Cell<usize>,
    pub diff: Option<DiffRead>,
}

/// Every outcome of the queue in the order it is shown: DECIDE, READY TO
/// LAND, RUNNING, LANDED.
pub fn flat(snap: &ConsoleSnapshot) -> Vec<&OutcomeView> {
    snap.groups.iter().flat_map(|g| &g.outcomes).collect()
}

impl WorkScreen {
    /// The selected outcome; the first of the queue when nothing was
    /// chosen or the chosen one is gone.
    pub fn current<'a>(&self, snap: &'a ConsoleSnapshot) -> Option<&'a OutcomeView> {
        let all = flat(snap);
        if !self.chosen {
            return all.first().copied();
        }
        self.selected
            .as_deref()
            .and_then(|id| all.iter().find(|o| o.id == id).copied())
            .or_else(|| all.get(self.at.min(all.len().saturating_sub(1))).copied())
    }

    /// Keeps the selection pointing at an outcome that exists: the same
    /// one while it is on the queue, else the one now where it was.
    pub fn settle(&mut self, snap: &ConsoleSnapshot) {
        let all = flat(snap);
        let found = self
            .selected
            .as_deref()
            .and_then(|id| all.iter().position(|o| o.id == id))
            .filter(|_| self.chosen);
        let index = match found {
            Some(i) => i,
            None if !self.chosen => 0,
            None => self.at.min(all.len().saturating_sub(1)),
        };
        let id = all.get(index).map(|o| o.id.clone());
        if id != self.selected {
            self.changed();
        }
        self.selected = id;
        self.at = index;
    }

    /// Moves the selection by `delta` rows, stopping at the ends.
    pub fn step(&mut self, snap: &ConsoleSnapshot, delta: isize) {
        let all = flat(snap);
        if all.is_empty() {
            return;
        }
        self.settle(snap);
        self.chosen = true;
        let to = self
            .at
            .saturating_add_signed(delta)
            .min(all.len().saturating_sub(1));
        if to != self.at {
            self.selected = Some(all[to].id.clone());
            self.at = to;
            self.changed();
        }
    }

    /// Selects the outcome `id`, when it is on the queue.
    pub fn select(&mut self, snap: &ConsoleSnapshot, id: &str) -> bool {
        match flat(snap).iter().position(|o| o.id == id) {
            Some(i) => {
                self.chosen = true;
                self.selected = Some(id.to_string());
                self.at = i;
                self.changed();
                true
            }
            None => false,
        }
    }

    /// A different outcome is selected: what was read for the last one is
    /// not this one's.
    fn changed(&mut self) {
        self.scroll = 0;
        self.diff = None;
    }

    /// Shows `tab`, from the top.
    pub fn show(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0;
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll.get();
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }
}

/// The held calls and denials of `outcome`, the ones waiting first, then
/// the newest refusal.
pub fn holds_of<'a>(snap: &'a ConsoleSnapshot, outcome: &OutcomeView) -> Vec<&'a HoldView> {
    let mut mine: Vec<&HoldView> = snap
        .holds
        .iter()
        .filter(|h| h.outcome.as_deref() == Some(outcome.id.as_str()))
        .collect();
    mine.sort_by_key(|h| (h.state != HoldState::Held, std::cmp::Reverse(h.at)));
    mine
}

/// The call the decision keys answer for `outcome`: the first held one.
pub fn held_request(outcome: &OutcomeView) -> Option<u64> {
    outcome
        .actions
        .iter()
        .find(|a| a.kind == ActionKind::AnswerHeldCall && a.enabled)
        .and_then(|a| a.request)
}

/// The mission whose proposed plan the decision keys answer for `outcome`.
pub fn plan_slug(outcome: &OutcomeView) -> Option<&str> {
    outcome
        .actions
        .iter()
        .find(|a| a.kind == ActionKind::ApprovePlan && a.enabled)
        .and_then(|a| a.mission.as_deref())
}

/// What an action row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    AllowOnce,
    AlwaysAllow,
    Deny,
    ApprovePlan,
    ChangePlan,
    Land,
    Kill,
    Retry,
    Resume,
    Evidence,
}

/// One line of the ACTIONS panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionRow {
    pub what: Do,
    /// The keymap action whose key the row shows.
    pub action: Action,
    pub label: &'static str,
    /// What it does, or why it cannot be done now.
    pub detail: String,
    pub enabled: bool,
}

fn find(o: &OutcomeView, kind: ActionKind) -> Option<&ActionView> {
    o.actions.iter().find(|a| a.kind == kind)
}

/// The actions the selected outcome offers, from the read model's allowed
/// actions: the ones that apply, enabled; and the ones the user would look
/// for (land, evidence) shown disabled with the reason. Nothing else is
/// offered: there is no send back, pause or merge to main.
pub fn action_rows(o: &OutcomeView, hold: Option<&HoldView>) -> Vec<ActionRow> {
    let mut rows = Vec::new();
    let row = |what, action, label, detail: String, enabled| ActionRow {
        what,
        action,
        label,
        detail,
        enabled,
    };

    if let Some(a) = find(o, ActionKind::AnswerHeldCall).filter(|a| a.enabled) {
        let held = hold.filter(|h| h.request == a.request);
        let destructive = held.is_some_and(|h| h.destructive);
        rows.push(row(
            Do::AllowOnce,
            DECIDE_YES,
            "allow once",
            if destructive {
                "irreversible: you name the target to confirm".into()
            } else {
                "let this one call through".into()
            },
            true,
        ));
        if destructive {
            rows.push(row(
                Do::AlwaysAllow,
                DECIDE_ALWAYS,
                "always allow",
                "irreversible calls are never always-allowed".into(),
                false,
            ));
        } else {
            rows.push(row(
                Do::AlwaysAllow,
                DECIDE_ALWAYS,
                "always allow",
                "add an allow rule for calls like this one".into(),
                true,
            ));
        }
        rows.push(row(
            Do::Deny,
            DECIDE_NO,
            "deny",
            "refuse the call; the agent is told it was denied".into(),
            true,
        ));
    }
    if find(o, ActionKind::ApprovePlan).is_some_and(|a| a.enabled) {
        rows.push(row(
            Do::ApprovePlan,
            DECIDE_YES,
            "approve plan",
            "start the build with the plan in its prompt".into(),
            true,
        ));
        rows.push(row(
            Do::ChangePlan,
            DECIDE_NO,
            "change plan",
            "send it back with a note for a new plan".into(),
            true,
        ));
    }
    if let Some(a) = find(o, ActionKind::LandToIntegration)
        && o.kind == swarmux_application::console::OutcomeKind::Mission
    {
        let detail = match (&a.reason, &o.landing) {
            (Some(why), _) => why.clone(),
            (None, Some(l)) => format!(
                "to {} · producers first, all or none, undo if one fails",
                l.into
            ),
            (None, None) => "land the work in the integration branch".into(),
        };
        rows.push(row(Do::Land, LAND, "land", detail, a.enabled));
    }
    if let Some(a) = find(o, ActionKind::Resume).filter(|a| a.enabled) {
        rows.push(row(
            Do::Resume,
            RESUME,
            "resume",
            format!(
                "continue job #{} where it stopped",
                a.job.unwrap_or_default()
            ),
            true,
        ));
    }
    if let Some(a) = find(o, ActionKind::Retry).filter(|a| a.enabled) {
        rows.push(row(
            Do::Retry,
            RETRY,
            "retry",
            format!(
                "queue job #{}'s prompt again as a new job",
                a.job.unwrap_or_default()
            ),
            true,
        ));
    }
    if let Some(a) = find(o, ActionKind::Kill).filter(|a| a.enabled) {
        rows.push(row(
            Do::Kill,
            KILL,
            "kill",
            format!("stop job #{}", a.job.unwrap_or_default()),
            true,
        ));
    }
    if let Some(a) = find(o, ActionKind::OpenEvidence)
        && (a.enabled || o.kind == swarmux_application::console::OutcomeKind::Mission)
    {
        let detail = match (&a.reason, &o.evidence) {
            (Some(why), _) => why.clone(),
            (None, Some(e)) => format!(
                "bundle {} in the Ledger, on this job",
                short(&e.bundle_hash)
            ),
            (None, None) => "open the evidence bundle in the Ledger".into(),
        };
        rows.push(row(Do::Evidence, EVIDENCE, "evidence", detail, a.enabled));
    }
    rows
}

/// The rows for `o`: its actions, with the held call they answer.
pub fn rows_for(snap: &ConsoleSnapshot, o: &OutcomeView) -> Vec<ActionRow> {
    let holds = holds_of(snap, o);
    let held = holds
        .first()
        .copied()
        .filter(|h| h.state == HoldState::Held);
    action_rows(o, held)
}

/// The first eight characters of a digest.
pub fn short(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

/// `9:30`: minutes and seconds left until `deadline`, `0:00` once past.
pub fn countdown(deadline: Millis, now: Millis) -> String {
    let s = ((deadline - now).max(0) + 999) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The group's heading word.
pub fn group_title(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::Decide => "DECIDE",
        GroupKind::ReadyToLand => "READY TO LAND",
        GroupKind::Running => "RUNNING",
        GroupKind::Landed => "LANDED",
    }
}

/// What the heading says beside the count.
pub fn group_note(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::Decide => "only you can",
        GroupKind::ReadyToLand => "verified · not landed",
        GroupKind::Running => "and queued · no action needed",
        GroupKind::Landed => "delivered, with proof",
    }
}

/// What an outcome holds up, and what holds it up, through the Deck's
/// edges: task dependencies, repositories' contracts and file claims. Only
/// edges not yet met count: a producer that landed blocks no one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relations {
    /// Cards of other outcomes that wait for this one.
    pub blocks: Vec<Related>,
    /// Cards of other outcomes this one waits for.
    pub waits_on: Vec<Related>,
}

/// The card on the other end of an edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Related {
    pub title: String,
    pub kind: EdgeKind,
    pub state: DepState,
}

/// The words for an edge kind.
pub fn edge_label(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::TaskDependency => "task dependency",
        EdgeKind::ContractLink => "contract",
        EdgeKind::FileClaim => "file claim",
    }
}

/// The unmet edges between `outcome` and the rest of the Deck's graph.
pub fn relations(snap: &ConsoleSnapshot, outcome: &OutcomeView) -> Relations {
    let graph = &snap.deck;
    let mine = |id: &str| {
        graph
            .nodes
            .iter()
            .any(|n| n.id == id && n.outcome == outcome.id)
    };
    let title_of = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.title.clone())
    };
    let other = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .is_some_and(|n| n.outcome != outcome.id)
    };
    let mut out = Relations::default();
    for e in graph.edges.iter().filter(|e| e.state != DepState::Met) {
        let related = |id: &str| {
            title_of(id).map(|title| Related {
                title,
                kind: e.kind,
                state: e.state,
            })
        };
        if mine(&e.from) && other(&e.to) {
            out.blocks.extend(related(&e.to));
        } else if mine(&e.to) && other(&e.from) {
            out.waits_on.extend(related(&e.from));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
