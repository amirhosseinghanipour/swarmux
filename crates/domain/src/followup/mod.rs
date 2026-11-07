//! Follow-ups: what agents notice, ask or suggest when they finish, kept
//! apart from the agents that said it so the owner can decide in batches.
//!
//! Everything here is deterministic. A model may help *find* follow-ups in
//! an agent's last message, but whether one is approved, declined or parked
//! is decided by the owner or by a standing order the owner wrote
//! ([`order`]), never by a model.

mod brief;
mod order;
mod stamp;

pub use brief::{BriefInput, BriefSlots, Card, OwnerBrief, compose, due_slot, pick, reason_to_decline};
pub use order::{Matched, OrderAction, PROTECTED_GLOBS, StandingOrder, decide, touches_protected};
pub use stamp::{
    RUBBER_STAMP_MIN, RUBBER_STAMP_RATE, RUBBER_STAMP_WINDOW, Stamped, rubber_stamped,
};

use crate::board::clean;
use crate::{JobId, Millis};

/// Longest follow-up text kept, in characters.
pub const TEXT_MAX: usize = 400;
/// Longest rationale or counter-argument kept.
pub const NOTE_MAX: usize = 400;

/// What a follow-up is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FollowKind {
    /// Something only the owner can answer.
    Question,
    /// A choice the agent could not make alone ("A or B?").
    Decision,
    /// Extra work worth doing ("should I also do Y?").
    SuggestedTask,
    /// Something that may go wrong.
    Risk,
    /// Something in the way ("needs a migration first").
    Blocker,
}

impl FollowKind {
    pub const ALL: [FollowKind; 5] = [
        FollowKind::Question,
        FollowKind::Decision,
        FollowKind::SuggestedTask,
        FollowKind::Risk,
        FollowKind::Blocker,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FollowKind::Question => "question",
            FollowKind::Decision => "decision",
            FollowKind::SuggestedTask => "task",
            FollowKind::Risk => "risk",
            FollowKind::Blocker => "blocker",
        }
    }

    pub fn parse(s: &str) -> Option<FollowKind> {
        let s = s.trim().to_ascii_lowercase();
        FollowKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .or(match s.as_str() {
                "suggested_task" | "suggested-task" | "suggestedtask" | "suggestion" => {
                    Some(FollowKind::SuggestedTask)
                }
                _ => None,
            })
    }

    /// Kinds a standing order may only approve when it says so itself.
    pub fn needs_owner(self) -> bool {
        matches!(
            self,
            FollowKind::Decision | FollowKind::Risk | FollowKind::Blocker
        )
    }

    /// Ordering weight in a brief: smaller comes first.
    pub(crate) fn weight(self) -> u8 {
        match self {
            FollowKind::Risk => 0,
            FollowKind::Blocker => 1,
            FollowKind::Decision => 2,
            FollowKind::Question => 3,
            FollowKind::SuggestedTask => 4,
        }
    }
}

/// How soon the owner should hear of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    /// The inbox's P1: look now.
    High,
}

impl Urgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::High => "p1",
        }
    }

    pub fn parse(s: &str) -> Option<Urgency> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" | "p3" => Some(Urgency::Low),
            "normal" | "medium" | "p2" => Some(Urgency::Normal),
            "high" | "urgent" | "p1" => Some(Urgency::High),
            _ => None,
        }
    }
}

/// Where a follow-up stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FollowStatus {
    Open,
    /// A standing order approved it.
    AutoApproved,
    Approved,
    Declined,
    /// Held back: until `due_at`, or parked for good when that is unset.
    Deferred,
    /// Handled: a question answered, an intent turned into a mission.
    Done,
}

impl FollowStatus {
    pub const ALL: [FollowStatus; 6] = [
        FollowStatus::Open,
        FollowStatus::AutoApproved,
        FollowStatus::Approved,
        FollowStatus::Declined,
        FollowStatus::Deferred,
        FollowStatus::Done,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FollowStatus::Open => "open",
            FollowStatus::AutoApproved => "auto-approved",
            FollowStatus::Approved => "approved",
            FollowStatus::Declined => "declined",
            FollowStatus::Deferred => "deferred",
            FollowStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<FollowStatus> {
        FollowStatus::ALL.into_iter().find(|x| x.as_str() == s)
    }

    /// Whether it counts as approved work waiting to become a mission.
    pub fn is_approved(self) -> bool {
        matches!(self, FollowStatus::Approved | FollowStatus::AutoApproved)
    }
}

/// Who decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecidedBy {
    Owner,
    /// A standing order, by its id.
    Order(String),
}

impl DecidedBy {
    /// `owner` or `order:<id>`, as stored.
    pub fn to_text(&self) -> String {
        match self {
            DecidedBy::Owner => "owner".into(),
            DecidedBy::Order(id) => format!("order:{id}"),
        }
    }

    pub fn parse(s: &str) -> Option<DecidedBy> {
        match s {
            "owner" => Some(DecidedBy::Owner),
            _ => s
                .strip_prefix("order:")
                .map(|id| DecidedBy::Order(id.into())),
        }
    }
}

/// Where a follow-up came from. A merged follow-up has several.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    pub job: Option<JobId>,
    pub mission: Option<String>,
    /// The Board entry it was made from.
    pub board_entry: Option<i64>,
}

impl Source {
    /// `job 4 · mission m · board #7`, with `—` for nothing known.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(j) = self.job {
            parts.push(format!("job {j}"));
        }
        if let Some(m) = self.mission.as_deref().filter(|m| !m.is_empty()) {
            parts.push(format!("mission {m}"));
        }
        if let Some(b) = self.board_entry {
            parts.push(format!("board #{b}"));
        }
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// One follow-up.
#[derive(Clone, Debug, PartialEq)]
pub struct FollowUp {
    /// Assigned by the store; `0` before.
    pub id: i64,
    pub kind: FollowKind,
    pub text: String,
    /// Why it matters, as the agent saw it.
    pub rationale: String,
    /// A reason the owner might say no, as the finder saw it; the brief
    /// falls back to a fixed one for the kind when empty.
    pub against: String,
    /// The repository or workspace's name; empty when unknown.
    pub repo: String,
    pub paths: Vec<String>,
    pub urgency: Urgency,
    pub est_cost_usd: Option<f64>,
    /// Every place it was found; the first is the earliest.
    pub sources: Vec<Source>,
    pub created: Millis,
    pub status: FollowStatus,
    pub decided_by: Option<DecidedBy>,
    pub decided_at: Option<Millis>,
    /// A deferred follow-up opens again at this time.
    pub due_at: Option<Millis>,
}

impl FollowUp {
    pub fn new(kind: FollowKind, text: &str, source: Source, created: Millis) -> FollowUp {
        FollowUp {
            id: 0,
            kind,
            text: clean(text, TEXT_MAX),
            rationale: String::new(),
            against: String::new(),
            repo: String::new(),
            paths: Vec::new(),
            urgency: Urgency::Normal,
            est_cost_usd: None,
            sources: vec![source],
            created,
            status: FollowStatus::Open,
            decided_by: None,
            decided_at: None,
            due_at: None,
        }
    }

    /// Whether it is waiting for the owner (open, or deferred and due).
    pub fn waiting(&self, now: Millis) -> bool {
        match self.status {
            FollowStatus::Open => true,
            FollowStatus::Deferred => self.due_at.is_some_and(|d| d <= now),
            _ => false,
        }
    }

    /// Whether it interrupts the owner rather than waits for the brief.
    /// `running` says the job it blocks is still going.
    pub fn urgent(&self, running: bool) -> bool {
        match self.kind {
            FollowKind::Risk => self.urgency == Urgency::High,
            FollowKind::Blocker => running || self.urgency == Urgency::High,
            _ => false,
        }
    }

    /// The job that found it first.
    pub fn first_job(&self) -> Option<JobId> {
        self.sources.iter().find_map(|s| s.job)
    }

    /// Every job that found it.
    pub fn jobs(&self) -> Vec<JobId> {
        self.sources.iter().filter_map(|s| s.job).collect()
    }

    /// The same follow-up found again: sources are joined, the more urgent
    /// word and the fuller explanation are kept.
    pub fn merge(&mut self, other: &FollowUp) {
        for s in &other.sources {
            if !self.sources.contains(s) {
                self.sources.push(s.clone());
            }
        }
        self.urgency = self.urgency.max(other.urgency);
        if other.rationale.len() > self.rationale.len() {
            self.rationale.clone_from(&other.rationale);
        }
        if self.against.is_empty() {
            self.against.clone_from(&other.against);
        }
        if self.repo.is_empty() {
            self.repo.clone_from(&other.repo);
        }
        for p in &other.paths {
            if !self.paths.contains(p) {
                self.paths.push(p.clone());
            }
        }
        self.est_cost_usd = match (self.est_cost_usd, other.est_cost_usd) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.created = self.created.min(other.created);
    }

    /// Whether `other` is the same follow-up: the same kind, the same files
    /// and the same words (or nearly).
    pub fn same_as(&self, other: &FollowUp) -> bool {
        self.kind == other.kind
            && path_set(&self.paths) == path_set(&other.paths)
            && similar(&self.text, &other.text)
    }
}

fn path_set(paths: &[String]) -> Vec<String> {
    let mut v: Vec<String> = paths
        .iter()
        .map(|p| {
            p.replace('\\', "/")
                .trim_start_matches("./")
                .to_ascii_lowercase()
        })
        .filter(|p| !p.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Lower-case words only: punctuation and spacing do not matter.
pub fn normalize(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Texts that are equal once normalised, or share at least 85% of their
/// words.
fn similar(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize(a), normalize(b));
    if na == nb {
        return true;
    }
    let wa: std::collections::BTreeSet<&str> = na.split(' ').collect();
    let wb: std::collections::BTreeSet<&str> = nb.split(' ').collect();
    let both = wa.intersection(&wb).count();
    let all = wa.union(&wb).count();
    all > 0 && both * 100 >= all * 85
}

/// Where `fresh` belongs among `known`: the index of the follow-up it
/// duplicates, if any. Finished ones (`Done`) are not matched.
pub fn find_duplicate(known: &[FollowUp], fresh: &FollowUp) -> Option<usize> {
    known
        .iter()
        .position(|k| k.status != FollowStatus::Done && k.same_as(fresh))
}

#[cfg(test)]
mod tests;
