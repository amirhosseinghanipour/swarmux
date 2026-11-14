//! The Return brief's read model: what happened while the user was away,
//! and what waits for them now. Plain data, no terminal types.
//!
//! It is built from two things swarmux already has: the console snapshot
//! (what stands now: the decisions, what is ready to land, what stopped,
//! what landed) and the ledger's digest of the window
//! ([`swarmux_domain::console::BriefDigest`]: what it cost, which calls
//! the Guard held, which fell back, which rules were added, which hidden
//! checks did not run, the verification figures, a timeline). Nothing is
//! invented: a figure with nothing behind it is `null` (or absent), never
//! zero, and no estimate of the owner's time is made, because swarmux does
//! not record how long a decision takes.
//!
//! # Wire shape
//!
//! The same rules as the console snapshot ([`super::view`]): `snake_case`
//! names, times in unix milliseconds, durations in milliseconds, money in
//! US dollars, a missing value is `null` and always present, lists are
//! present and may be empty, adding a field does not change
//! [`BRIEF_SCHEMA`], renaming or re-typing one does. The golden test in
//! `console/tests/c5.rs` fails until that is done on purpose.
//!
//! # Where the lines of later milestones go
//!
//! [`BriefView::back_pressure`] is where "dispatch held by WIP limit since
//! …" and "sampled items waiting" will be read from once review back
//! pressure exists (milestone X5). It is `null` until then, and nothing
//! fills it in.

use serde::{Deserialize, Serialize};
use swarmux_domain::Millis;
use swarmux_domain::console::{BriefDigest, BriefTone, CallState};

use super::view::{
    ActionKind, AskKind, ConsoleSnapshot, FirstRunView, GroupKind, LandState, Lifecycle,
    OutcomeView, PriorityKind, SpreadView,
};
use crate::AppResult;
use crate::ledger::Ledger;
use crate::metrics::{MAX_ENTRIES, read_window};

/// The shape of everything in this module. Bump it on a breaking change.
pub const BRIEF_SCHEMA: u32 = 1;

/// Decisions, stopped outcomes and the like the view lists in full; more
/// are counted in `total`.
pub const LIST_CAP: usize = 50;

/// What the brief reads besides the snapshot.
#[derive(Clone, Copy)]
pub struct BriefInput<'a> {
    pub now: Millis,
    /// When the user was last here and when they came back; `None` when
    /// there is no history (the first run).
    pub window: Option<(Millis, Millis)>,
    /// The ledger's account of the window; `None` where no ledger is read.
    pub digest: Option<&'a BriefDigest>,
}

/// Reads the ledger for the window `from..=to` and digests it. A ledger
/// that cannot be read is an error for the caller to show.
pub fn read_digest(ledger: &dyn Ledger, from: Millis, to: Millis) -> AppResult<BriefDigest> {
    let (entries, reached) = read_window(ledger, from, MAX_ENTRIES)?;
    Ok(swarmux_domain::console::compute_brief_digest(
        &entries, from, to, reached,
    ))
}

/// What kind of brief it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BriefState {
    /// No history before this run: there is no "while you were away".
    FirstRun,
    /// A window, and nothing notable happened in it and nothing waits.
    Nothing,
    /// Something happened or something waits.
    Happened,
}

/// The whole brief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BriefView {
    pub schema: u32,
    pub state: BriefState,
    /// When it was built.
    pub at: Millis,
    /// `null` on the first run.
    pub window: Option<WindowView>,
    /// Outcomes that landed in the window.
    pub landed: LandedView,
    /// Verified, waiting for the user to land them.
    pub ready: Vec<ReadyView>,
    pub ready_total: usize,
    /// Decisions waiting now, in the order Work ranks them.
    pub decisions: Vec<DecisionView>,
    pub decisions_total: usize,
    /// Outcomes that stopped or were escalated in the window.
    pub stopped: Vec<StoppedView>,
    pub stopped_total: usize,
    /// `null` where no ledger is read.
    pub spend: Option<SpendAwayView>,
    /// The ledger's verification figures over the window.
    pub verification: Option<VerificationView>,
    pub guard: Option<GuardView>,
    /// Hook calls decided without swarmux (an unreachable app).
    pub fallbacks: Vec<FallbackView>,
    /// Always-allow rules added in the window.
    pub rules_added: Vec<RuleAddedView>,
    /// Hidden checks that did not run, and checks run in a worktree.
    pub checks: Option<ChecksNoteView>,
    /// What landed, by repository, where landing data exists.
    pub lands_where: Vec<RepoLandsView>,
    /// The window, oldest first.
    pub timeline: Vec<MomentView>,
    /// Review back pressure (milestone X5). `null` until it exists.
    pub back_pressure: Option<BackPressureView>,
    /// How much of the ledger was read; `null` where none was.
    pub read: Option<ReadView>,
}

/// The window the brief is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowView {
    pub from_at: Millis,
    pub to_at: Millis,
    pub duration_ms: u64,
}

/// The ledger entries the figures were worked out from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadView {
    pub entries: u32,
    /// The window was read whole; false when the read was cut short.
    pub complete: bool,
}

/// Outcomes that landed in the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandedView {
    /// Landed with a sealed evidence bundle.
    pub with_proof: u32,
    /// Landed with no bundle (a plain job nothing sealed).
    pub without_proof: u32,
    /// The queue lists only its newest landed outcomes: more may have
    /// landed in the window than were counted.
    pub truncated: bool,
    /// Newest first.
    pub items: Vec<LandedItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandedItem {
    pub id: String,
    pub title: String,
    pub at: Millis,
    /// The sealed bundle's hash; `null` when there is none.
    pub bundle: Option<String>,
    pub files: Option<u32>,
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadyView {
    pub id: String,
    pub title: String,
    /// Since when it has been ready.
    pub since_at: Millis,
    /// The user can land it from Work now.
    pub can_land: bool,
    pub repos: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionView {
    pub id: String,
    pub title: String,
    pub priority: Option<PriorityKind>,
    pub kind: Option<AskKind>,
    /// What is asked.
    pub ask: String,
    /// The first line of what the ask says besides its title, when it
    /// says something else.
    pub detail: Option<String>,
    pub since_at: Millis,
    /// When it resolves without the user (a held call is denied).
    pub deadline_at: Option<Millis>,
    /// What waits on this one: cards whose dependency on it is not met.
    pub blocks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoppedView {
    pub id: String,
    pub title: String,
    pub lifecycle: Lifecycle,
    pub at: Millis,
    /// Why, in the words swarmux kept; `null` when none were.
    pub reason: Option<String>,
    /// It is also among the decisions.
    pub decision_waiting: bool,
}

/// What the jobs that ended while away cost.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendAwayView {
    pub usd: f64,
    pub jobs: u32,
    /// Across every Claude Code session on the machine today, when read.
    pub today_usd: Option<f64>,
    /// swarmux has no day budget setting: always `null` for now.
    pub day_budget_usd: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationView {
    pub first_run: Option<FirstRunView>,
    pub claim_to_verified: Option<SpreadView>,
    pub verified: u32,
    pub checks_run: u32,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardView {
    /// Allowed without asking: counted, not listed.
    pub allowed: u32,
    /// Held, refused and stopped calls, oldest first.
    pub calls: Vec<GuardCallView>,
    pub calls_total: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardCallState {
    /// Held; waiting for the user now.
    Waiting,
    /// Held; no entry in the window says how it ended.
    Held,
    HeldAllowed,
    HeldDenied,
    Denied,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardCallView {
    pub at: Millis,
    pub job: Option<i64>,
    pub tool: String,
    pub target: String,
    /// The rule that decided it, or why none did.
    pub reason: String,
    pub state: GuardCallState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FallbackView {
    pub at: Millis,
    pub job: Option<i64>,
    pub tool: String,
    pub reason: String,
    /// What the hook did without swarmux: `allow` or `deny`.
    pub mode: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleAddedView {
    pub at: Millis,
    pub rule: String,
    pub scope: String,
    pub root: Option<String>,
    pub source: String,
    pub expires_at: Option<Millis>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChecksNoteView {
    pub hidden_not_run: Vec<HiddenNotRunView>,
    pub worktree_runs: Vec<WorktreeRunView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiddenNotRunView {
    pub job: Option<i64>,
    /// The outcome the job belongs to, when it is on the queue.
    pub title: Option<String>,
    pub planned: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorktreeRunView {
    pub job: Option<i64>,
    pub title: Option<String>,
    pub root: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoLandsView {
    pub repo: String,
    pub items: Vec<RepoLandItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoLandItem {
    pub outcome: String,
    pub title: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MomentTone {
    Good,
    Warn,
    Bad,
    Plain,
}

impl From<BriefTone> for MomentTone {
    fn from(t: BriefTone) -> MomentTone {
        match t {
            BriefTone::Good => MomentTone::Good,
            BriefTone::Warn => MomentTone::Warn,
            BriefTone::Bad => MomentTone::Bad,
            BriefTone::Plain => MomentTone::Plain,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MomentView {
    pub at: Millis,
    pub job: Option<i64>,
    pub tone: MomentTone,
    pub text: String,
}

/// Review back pressure, from milestone X5. Nothing builds it yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackPressureView {
    /// "dispatch held by WIP limit since …".
    pub held_since: Option<Millis>,
    /// "sampled items waiting".
    pub sampled_waiting: Option<u32>,
}

/// Builds the brief from the snapshot and the ledger's digest.
pub fn build_brief(snap: &ConsoleSnapshot, input: &BriefInput<'_>) -> BriefView {
    let all: Vec<&OutcomeView> = snap.groups.iter().flat_map(|g| &g.outcomes).collect();
    let in_group = |kind: GroupKind| -> Vec<&OutcomeView> {
        snap.groups
            .iter()
            .filter(|g| g.kind == kind)
            .flat_map(|g| g.outcomes.iter())
            .collect()
    };
    let window = input.window;
    let within = |at: Millis| window.is_some_and(|(from, to)| (from..=to).contains(&at));

    // Landed in the window.
    let landed_group = snap.groups.iter().find(|g| g.kind == GroupKind::Landed);
    let landed: Vec<&OutcomeView> = in_group(GroupKind::Landed)
        .into_iter()
        .filter(|o| within(o.since_at))
        .collect();
    let truncated = landed_group.is_some_and(|g| {
        g.total > g.outcomes.len()
            && window.is_some_and(|(from, _)| g.outcomes.last().is_some_and(|o| o.since_at >= from))
    });
    let items: Vec<LandedItem> = landed
        .iter()
        .map(|o| LandedItem {
            id: o.id.clone(),
            title: o.title.clone(),
            at: o.since_at,
            bundle: o.evidence.as_ref().map(|e| e.bundle_hash.clone()),
            files: o.diff.as_ref().map(|d| d.files),
            insertions: o.diff.as_ref().map(|d| d.insertions),
            deletions: o.diff.as_ref().map(|d| d.deletions),
        })
        .collect();
    let with_proof = items.iter().filter(|i| i.bundle.is_some()).count();
    let landed_view = LandedView {
        with_proof: u32::try_from(with_proof).unwrap_or(u32::MAX),
        without_proof: u32::try_from(items.len() - with_proof).unwrap_or(u32::MAX),
        truncated,
        items,
    };

    // Ready to land.
    let ready_all = in_group(GroupKind::ReadyToLand);
    let ready: Vec<ReadyView> = ready_all
        .iter()
        .take(LIST_CAP)
        .map(|o| ReadyView {
            id: o.id.clone(),
            title: o.title.clone(),
            since_at: o.since_at,
            can_land: o
                .actions
                .iter()
                .any(|a| a.kind == ActionKind::LandToIntegration && a.enabled),
            repos: o
                .landing
                .as_ref()
                .map(|l| l.repos.iter().map(|r| r.repo.clone()).collect())
                .or_else(|| o.contract.as_ref().map(|c| c.repos.clone()))
                .unwrap_or_default(),
        })
        .collect();

    // Decisions waiting, in Work's order.
    let decide = in_group(GroupKind::Decide);
    let decisions: Vec<DecisionView> = decide
        .iter()
        .take(LIST_CAP)
        .map(|o| decision(snap, o, input.now))
        .collect();

    // Stopped or escalated in the window.
    let stopped_all: Vec<StoppedView> = all
        .iter()
        .filter(|o| {
            matches!(
                o.lifecycle,
                Lifecycle::Escalated
                    | Lifecycle::Refused
                    | Lifecycle::Failed
                    | Lifecycle::Interrupted
            ) && within(o.updated_at)
        })
        .map(|o| StoppedView {
            id: o.id.clone(),
            title: o.title.clone(),
            lifecycle: o.lifecycle,
            at: o.updated_at,
            reason: stop_reason(o),
            decision_waiting: o.group == GroupKind::Decide,
        })
        .collect();
    let stopped_total = stopped_all.len();
    let mut stopped = stopped_all;
    stopped.sort_by_key(|s| std::cmp::Reverse(s.at));
    stopped.truncate(LIST_CAP);

    let digest = input.digest;
    let title_of_job = |job: Option<i64>| -> Option<String> {
        let job = job?;
        all.iter()
            .find(|o| o.jobs.iter().any(|j| j.id == job))
            .map(|o| o.title.clone())
    };

    let spend = digest.and_then(|d| d.spend).map(|s| SpendAwayView {
        usd: s.usd,
        jobs: s.jobs,
        today_usd: snap.header.spend.today_usd,
        day_budget_usd: None,
    });
    let verification = digest.and_then(|d| {
        let m = &d.metrics;
        (m.first_run.is_some() || m.claim_to_verified.is_some() || m.verified > 0).then(|| {
            VerificationView {
                first_run: m.first_run.map(|f| FirstRunView {
                    passed: f.passed,
                    judged: f.judged,
                    rate: f.rate(),
                }),
                claim_to_verified: m.claim_to_verified.map(|s| SpreadView {
                    median_ms: s.median_ms,
                    p90_ms: s.p90_ms,
                    samples: s.samples,
                }),
                verified: m.verified,
                checks_run: m.checks_run,
                complete: m.complete,
            }
        })
    });
    let guard = digest.map(|d| {
        let calls: Vec<GuardCallView> = d
            .guard
            .calls
            .iter()
            .map(|c| {
                let live = snap.holds.iter().find(|h| {
                    h.state == super::view::HoldState::Held
                        && h.job == c.job
                        && h.tool == c.tool
                        && h.target == c.subject
                });
                let reason = match (&c.rule, live) {
                    (Some(r), _) => r.clone(),
                    (None, Some(h)) if h.rule.is_some() => h.rule.clone().unwrap_or_default(),
                    (None, Some(h)) if h.destructive => "destructive risk".to_string(),
                    (None, _) if c.state == CallState::Stopped => "loop breaker".to_string(),
                    (None, _) => "no rule matched".to_string(),
                };
                GuardCallView {
                    at: c.at,
                    job: c.job,
                    tool: c.tool.clone(),
                    target: c.subject.clone(),
                    reason,
                    state: match (c.state, live.is_some()) {
                        (CallState::Held, true) => GuardCallState::Waiting,
                        (CallState::Held, false) => GuardCallState::Held,
                        (CallState::HeldAllowed, _) => GuardCallState::HeldAllowed,
                        (CallState::HeldDenied, _) => GuardCallState::HeldDenied,
                        (CallState::Denied, _) => GuardCallState::Denied,
                        (CallState::Stopped, _) => GuardCallState::Stopped,
                    },
                }
            })
            .collect();
        GuardView {
            allowed: d.guard.allowed,
            calls_total: calls.len(),
            calls,
        }
    });
    let fallbacks: Vec<FallbackView> = digest
        .map(|d| {
            d.fallbacks
                .iter()
                .map(|f| FallbackView {
                    at: f.at,
                    job: f.job,
                    tool: f.tool.clone(),
                    reason: f.reason.clone(),
                    mode: f.mode.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let rules_added: Vec<RuleAddedView> = digest
        .map(|d| {
            d.rules_added
                .iter()
                .map(|r| RuleAddedView {
                    at: r.at,
                    rule: r.rule.clone(),
                    scope: r.scope.clone(),
                    root: r.root.clone(),
                    source: r.source.clone(),
                    expires_at: r.expires_at,
                })
                .collect()
        })
        .unwrap_or_default();
    let checks = digest.map(|d| ChecksNoteView {
        hidden_not_run: d
            .hidden_not_run
            .iter()
            .map(|h| HiddenNotRunView {
                job: h.job,
                title: title_of_job(h.job),
                planned: h.planned,
                reason: h.reason.clone(),
            })
            .collect(),
        worktree_runs: d
            .worktree_runs
            .iter()
            .map(|w| WorktreeRunView {
                job: w.job,
                title: title_of_job(w.job),
                root: w.root.clone(),
            })
            .collect(),
    });
    let timeline: Vec<MomentView> = digest
        .map(|d| {
            d.timeline
                .iter()
                .map(|m| MomentView {
                    at: m.at,
                    job: m.job,
                    tone: m.tone.into(),
                    text: m.text.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let lands_where = lands_where(&landed);

    let notable = landed_view.with_proof + landed_view.without_proof > 0
        || stopped_total > 0
        || spend.is_some()
        || guard.as_ref().is_some_and(|g| !g.calls.is_empty())
        || !fallbacks.is_empty()
        || !rules_added.is_empty()
        || checks
            .as_ref()
            .is_some_and(|c| !c.hidden_not_run.is_empty() || !c.worktree_runs.is_empty())
        || !timeline.is_empty()
        || verification.is_some();
    let state = if window.is_none() {
        BriefState::FirstRun
    } else if notable || !decide.is_empty() || !ready_all.is_empty() {
        BriefState::Happened
    } else {
        BriefState::Nothing
    };

    BriefView {
        schema: BRIEF_SCHEMA,
        state,
        at: input.now,
        window: window.map(|(from, to)| WindowView {
            from_at: from,
            to_at: to,
            duration_ms: u64::try_from((to - from).max(0)).unwrap_or(0),
        }),
        landed: landed_view,
        ready_total: ready_all.len(),
        ready,
        decisions_total: decide.len(),
        decisions,
        stopped_total,
        stopped,
        spend,
        verification,
        guard,
        fallbacks,
        rules_added,
        checks,
        lands_where,
        timeline,
        back_pressure: None,
        read: digest.map(|d| ReadView {
            entries: d.entries,
            complete: d.complete,
        }),
    }
}

/// One decision: what is asked, when it resolves by itself, what waits on it.
fn decision(snap: &ConsoleSnapshot, o: &OutcomeView, now: Millis) -> DecisionView {
    let deciding = o.asks.iter().filter(|a| a.decides);
    let first = deciding.clone().next().or_else(|| o.asks.first());
    // The soonest moment something resolves without the user, if it has not
    // already passed.
    let deadline_at = deciding
        .filter_map(|a| a.deadline_at)
        .filter(|d| *d >= now - 1_000)
        .min();
    DecisionView {
        id: o.id.clone(),
        title: o.title.clone(),
        priority: o.priority.or(first.map(|a| a.priority)),
        kind: first.map(|a| a.kind),
        ask: first.map_or_else(|| "needs a decision".to_string(), |a| a.title.clone()),
        detail: first
            .and_then(|a| a.detail.lines().map(str::trim).find(|l| !l.is_empty()))
            .filter(|l| Some(*l) != first.map(|a| a.title.as_str()))
            .map(str::to_string),
        since_at: o.since_at,
        deadline_at,
        blocks: blocked_by(snap, o),
    }
}

/// The titles of cards that wait on a card of `o`: an edge that is a real
/// dependency (a task's, or a contract's) and not met, to a card that is
/// not done.
fn blocked_by(snap: &ConsoleSnapshot, o: &OutcomeView) -> Vec<String> {
    use super::view::{DepState, EdgeKind, NodeState};
    let mine: Vec<&str> = snap
        .deck
        .nodes
        .iter()
        .filter(|n| n.outcome == o.id)
        .map(|n| n.id.as_str())
        .collect();
    let mut titles: Vec<String> = Vec::new();
    for e in &snap.deck.edges {
        if e.kind == EdgeKind::FileClaim
            || e.state == DepState::Met
            || !mine.contains(&e.from.as_str())
        {
            continue;
        }
        let Some(card) = snap.deck.nodes.iter().find(|n| n.id == e.to) else {
            continue;
        };
        if card.outcome == o.id || matches!(card.state, NodeState::Done | NodeState::Stopped) {
            continue;
        }
        if !titles.contains(&card.title) {
            titles.push(card.title.clone());
        }
    }
    titles
}

/// Why an outcome stopped, in the words swarmux kept.
fn stop_reason(o: &OutcomeView) -> Option<String> {
    o.contract
        .as_ref()
        .and_then(|c| c.note.clone())
        .or_else(|| o.jobs.iter().rev().find_map(|j| j.error.clone()))
        .or_else(|| o.asks.first().map(|a| a.title.clone()))
        .filter(|s| !s.trim().is_empty())
}

/// What landed, by repository: only where the outcome has landing data.
fn lands_where(landed: &[&OutcomeView]) -> Vec<RepoLandsView> {
    let mut by_repo: Vec<RepoLandsView> = Vec::new();
    for o in landed {
        let Some(l) = &o.landing else { continue };
        for r in l.repos.iter().filter(|r| r.state == LandState::Landed) {
            let item = RepoLandItem {
                outcome: o.id.clone(),
                title: o.title.clone(),
                branch: r.branch.clone(),
                commit: r.commit.clone(),
            };
            match by_repo.iter_mut().find(|x| x.repo == r.repo) {
                Some(x) => x.items.push(item),
                None => by_repo.push(RepoLandsView {
                    repo: r.repo.clone(),
                    items: vec![item],
                }),
            }
        }
    }
    by_repo.sort_by(|a, b| a.repo.cmp(&b.repo));
    by_repo
}
