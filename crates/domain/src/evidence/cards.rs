//! What the Ledger screen shows: run cards, the labels outcomes earn
//! later, the numbers over a set of runs and the digest of what happened
//! while the user was away.

use crate::{JobId, Millis};

/// A merged change counts as reverted when a revert merged within this
/// long after it: seven days.
pub const SETTLE_MS: Millis = 7 * 24 * 3_600_000;
/// Passing runs in a row the headline reliability figure stands for.
const PASS_K: i32 = 3;

/// How a pull request stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged { at: Millis },
    Closed,
}

/// What GitHub says about the pull request of a mission's branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrOutcome {
    pub number: u64,
    pub state: PrState,
    pub url: String,
    /// When a pull request that reverts it merged, if one did.
    pub reverted_at: Option<Millis>,
}

/// What became of the delivered work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelKind {
    PrOpen,
    Merged,
    /// Merged, then reverted within [`SETTLE_MS`].
    Reverted,
    /// Closed without merging.
    Closed,
}

impl LabelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LabelKind::PrOpen => "pr_open",
            LabelKind::Merged => "merged",
            LabelKind::Reverted => "reverted",
            LabelKind::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<LabelKind> {
        [
            LabelKind::PrOpen,
            LabelKind::Merged,
            LabelKind::Reverted,
            LabelKind::Closed,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// An outcome label on a mission, attached after delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label {
    pub kind: LabelKind,
    pub pr: u64,
    /// When the labelled event happened (the merge, say).
    pub at: Millis,
}

/// The label a pull request's state earns.
pub fn label_of(pr: &PrOutcome, now: Millis) -> Label {
    let (kind, at) = match pr.state {
        PrState::Open => (LabelKind::PrOpen, now),
        PrState::Closed => (LabelKind::Closed, now),
        PrState::Merged { at } => match pr.reverted_at {
            Some(r) if r >= at && r - at <= SETTLE_MS => (LabelKind::Reverted, r),
            _ => (LabelKind::Merged, at),
        },
    };
    Label {
        kind,
        pr: pr.number,
        at,
    }
}

/// How a run ended, as a card shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// A mission whose gate passed, with its bundle.
    Delivered,
    /// The user decides: the gate gave up, the budget ran out or it failed.
    Escalated,
    Refused,
    Failed,
    Cancelled,
    Queued,
    Running,
    /// A plain job that finished; nothing judged it.
    Done,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RunOutcome::Delivered => "delivered",
            RunOutcome::Escalated => "escalated",
            RunOutcome::Refused => "refused",
            RunOutcome::Failed => "failed",
            RunOutcome::Cancelled => "cancelled",
            RunOutcome::Queued => "queued",
            RunOutcome::Running => "running",
            RunOutcome::Done => "done",
        }
    }

    /// The run is over for good, one way or the other.
    pub fn is_final(self) -> bool {
        !matches!(self, RunOutcome::Queued | RunOutcome::Running)
    }

    /// Someone has to look at it.
    pub fn needs_you(self) -> bool {
        matches!(
            self,
            RunOutcome::Escalated | RunOutcome::Refused | RunOutcome::Failed
        )
    }
}

/// The bundle of a card, in a few numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEvidence {
    /// The first characters of the bundle's digest.
    pub digest: String,
    pub checks_passed: usize,
    pub checks_total: usize,
    pub blocks: u32,
}

/// One run: a mission or a plain job.
#[derive(Clone, Debug, PartialEq)]
pub struct RunCard {
    pub job: JobId,
    /// The mission's slug, when the run is a mission's.
    pub mission: Option<String>,
    pub title: String,
    pub outcome: RunOutcome,
    pub evidence: Option<RunEvidence>,
    pub cost_usd: f64,
    /// Dollars per model, largest first.
    pub models: Vec<(String, f64)>,
    pub labels: Vec<Label>,
    pub at: Millis,
    /// Share of the run's input served from the prompt cache; `None` when
    /// unknown.
    pub cache: Option<f64>,
}

impl RunCard {
    /// True when the title, mission, outcome or a label contains `query`,
    /// ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.title.to_lowercase().contains(&q)
            || self.outcome.as_str().contains(&q)
            || self
                .mission
                .as_deref()
                .is_some_and(|m| m.to_lowercase().contains(&q))
            || self.labels.iter().any(|l| l.kind.as_str().contains(&q))
    }

    pub fn is_mission(&self) -> bool {
        self.mission.is_some()
    }
}

/// The numbers over a set of runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedgerStats {
    /// Missions that reached an end.
    pub missions: usize,
    pub delivered: usize,
    pub spent_usd: f64,
    /// Delivered over ended; `None` before any mission ended.
    pub pass_rate: Option<f64>,
    /// The chance that three missions in a row are delivered.
    pub pass_k: Option<f64>,
    /// What each verified outcome cost, failures included.
    pub cost_per_verified: Option<f64>,
    /// Gate blocks and hand-offs to the user, per ended mission.
    pub interventions_per_task: Option<f64>,
    pub merged: usize,
    pub reverted: usize,
}

impl LedgerStats {
    pub fn of(cards: &[RunCard]) -> LedgerStats {
        let ended: Vec<&RunCard> = cards
            .iter()
            .filter(|c| c.is_mission() && c.outcome.is_final())
            .collect();
        let delivered = ended
            .iter()
            .filter(|c| c.outcome == RunOutcome::Delivered)
            .count();
        let spent: f64 = ended.iter().map(|c| c.cost_usd).sum();
        let interventions: f64 = ended
            .iter()
            .map(|c| {
                let blocks = c.evidence.as_ref().map_or(0, |e| e.blocks);
                f64::from(blocks) + f64::from(u8::from(c.outcome.needs_you()))
            })
            .sum();
        let n = ended.len() as f64;
        let has = |kind: LabelKind| {
            cards
                .iter()
                .filter(|c| c.labels.last().is_some_and(|l| l.kind == kind))
                .count()
        };
        let pass_rate = (!ended.is_empty()).then(|| delivered as f64 / n);
        LedgerStats {
            missions: ended.len(),
            delivered,
            spent_usd: cards.iter().map(|c| c.cost_usd).sum(),
            pass_rate,
            pass_k: pass_rate.map(|r| r.powi(PASS_K)),
            cost_per_verified: (delivered > 0).then(|| spent / delivered as f64),
            interventions_per_task: (!ended.is_empty()).then(|| interventions / n),
            merged: has(LabelKind::Merged),
            reverted: has(LabelKind::Reverted),
        }
    }
}

/// What happened between two moments, for the morning brief and the
/// "while you were away" digest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brief {
    pub since: Millis,
    pub until: Millis,
    pub delivered: usize,
    pub escalated: usize,
    pub failed: usize,
    pub running: usize,
    pub spent_usd: f64,
    pub merged: usize,
    pub reverted: usize,
    /// Titles of the runs that need the user, newest first (at most five).
    pub needs_you: Vec<String>,
}

impl Brief {
    /// Nothing happened.
    pub fn is_empty(&self) -> bool {
        self.delivered + self.escalated + self.failed + self.running + self.merged + self.reverted
            == 0
    }

    /// The headline: counts, then what it cost.
    pub fn headline(&self) -> String {
        let mut parts = Vec::new();
        for (n, word) in [
            (self.delivered, "delivered"),
            (self.escalated, "escalated"),
            (self.failed, "failed"),
            (self.running, "running"),
            (self.merged, "merged"),
            (self.reverted, "reverted"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {word}"));
            }
        }
        if parts.is_empty() {
            return "nothing happened".to_string();
        }
        format!("{} · ${:.2}", parts.join(" · "), self.spent_usd)
    }
}

/// The runs that started or ended in `since..=until`, summed up.
pub fn brief(cards: &[RunCard], since: Millis, until: Millis) -> Brief {
    let mut b = Brief {
        since,
        until,
        ..Brief::default()
    };
    let mut mine: Vec<&RunCard> = cards
        .iter()
        .filter(|c| c.at >= since && c.at <= until)
        .collect();
    mine.sort_by_key(|c| std::cmp::Reverse(c.at));
    for c in mine {
        b.spent_usd += c.cost_usd;
        match c.outcome {
            RunOutcome::Delivered => b.delivered += 1,
            RunOutcome::Escalated | RunOutcome::Refused => b.escalated += 1,
            RunOutcome::Failed => b.failed += 1,
            RunOutcome::Running | RunOutcome::Queued => b.running += 1,
            RunOutcome::Cancelled | RunOutcome::Done => {}
        }
        if c.outcome.needs_you() && b.needs_you.len() < 5 {
            b.needs_you.push(c.title.clone());
        }
    }
    for c in cards {
        if let Some(l) = c.labels.last()
            && l.at >= since
            && l.at <= until
        {
            match l.kind {
                LabelKind::Merged => b.merged += 1,
                LabelKind::Reverted => b.reverted += 1,
                _ => {}
            }
        }
    }
    b
}

/// Runs per day over the last `days` days up to `now`, oldest day first.
pub fn heat_strip(cards: &[RunCard], now: Millis, days: usize) -> Vec<u32> {
    const DAY: Millis = 86_400_000;
    let today = now.div_euclid(DAY);
    let mut out = vec![0u32; days];
    for c in cards {
        let ago = today - c.at.div_euclid(DAY);
        if ago >= 0 && (ago as usize) < days {
            out[days - 1 - ago as usize] += 1;
        }
    }
    out
}
