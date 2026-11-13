//! Outcomes: one [`OutcomeView`] per mission, and one per plain job that
//! belongs to no mission, assembled from what swarmux already keeps.

use std::collections::{HashMap, HashSet};

use swarmux_domain::console as rules;
use swarmux_domain::evidence::Bundle;
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::mission::{Landing, Mission, MissionState};
use swarmux_domain::plan::{DriftReport, Plan, PlanRecord, PlanStatus as DomainPlanStatus};
use swarmux_domain::taskgraph::{GraphRun, TaskState as DomainTaskState, integration_branch};
use swarmux_domain::{AttentionItem, JobId, JobStatus, ReviewState, ReviewVerdict};

use super::hold::cut;
use super::service::{Caches, Sources};
use super::view::{
    ActionKind, ActionView, AskView, BudgetView, CheckProgress, CheckRunView, CheckStatus,
    ChecksSource, ChecksView, ClaimView, ContractView, DeckNode, DiffView, DriftView, EvidenceView,
    GateVerdict, GateView, GroupKind, HoldState, HoldView, JobState, JobView, LandState,
    LandingMode, LandingRepoView, LandingView, Lifecycle, NodeKind, NodeState, OutcomeKind,
    OutcomeView, PlanStatus, PlanTaskView, PlanView, SignatureState, SpendView, StageKey,
    StageState, StageView,
};
use crate::gate::{GateDetail, GateRun, GateState};
use crate::jobs::JobFact;
use crate::missions::LandingPolicy;

/// Longest quote of what the agent claims, in characters.
const CLAIM_MAX: usize = 1_500;

/// A built outcome, with what the ranking and the Deck need of it.
pub(super) struct Built {
    pub view: OutcomeView,
    pub standing: rules::Standing,
    pub cards: Vec<Card>,
    pub tasks: Option<TaskGraphFacts>,
}

/// One card of the Deck, and what edges are derived from.
pub(super) struct Card {
    pub node: DeckNode,
    pub facts: rules::Node,
}

/// A plan run as a task graph, for the dependency edges.
pub(super) struct TaskGraphFacts {
    pub plan: Plan,
    pub node_of: HashMap<String, String>,
    pub progress: HashMap<String, rules::Progress>,
}

/// What a delivered mission's sealed bundle says, kept after the first
/// read: a sealed bundle does not change.
#[derive(Clone)]
pub(super) struct Sealed {
    pub evidence: EvidenceView,
    pub diff: DiffView,
    pub runs: Vec<CheckRunView>,
    pub gate: Option<GateView>,
    pub hidden: Option<(u32, u32)>,
    pub review: Option<(String, u32)>,
}

impl Sealed {
    pub(super) fn of(stored: &crate::Stored) -> Sealed {
        let b: &Bundle = &stored.bundle;
        let signature = match b.signature_ok() {
            Ok(true) => SignatureState::Valid,
            Ok(false) => SignatureState::Invalid,
            Err(_) if b.signature.is_none() => SignatureState::Unsigned,
            Err(_) => SignatureState::Unchecked,
        };
        Sealed {
            evidence: EvidenceView {
                bundle_hash: stored.digest.clone(),
                sealed_at: b.sealed_at,
                seq_from: b.ledger.from_seq,
                seq_to: b.ledger.to_seq,
                head_hash: b.ledger.head_hash.clone(),
                seal_seq: stored.seal_seq,
                signature,
                anchored: b.anchor.is_some(),
            },
            diff: DiffView {
                files: b.diff.files,
                insertions: b.diff.insertions,
                deletions: b.diff.deletions,
                hash: b.diff.hash.clone(),
            },
            runs: b
                .checks
                .iter()
                .map(|c| CheckRunView {
                    name: c.name.clone(),
                    status: match (c.passed, c.flaky) {
                        (true, true) => CheckStatus::Flaky,
                        (true, false) => CheckStatus::Passed,
                        (false, _) => CheckStatus::Failed,
                    },
                    duration_ms: c.duration_ms,
                    first_failure: Some(c.first_failure.clone())
                        .filter(|l| !c.passed && !l.is_empty())
                        .map(|l| first_line(&l)),
                })
                .collect(),
            gate: b.gates.last().and_then(|g| {
                let verdict = match g.outcome.as_str() {
                    "pass" => GateVerdict::Pass,
                    "block" => GateVerdict::Block,
                    "escalate" => GateVerdict::Escalate,
                    _ => return None,
                };
                Some(GateView {
                    verdict,
                    blocks: b.blocks(),
                    max_blocks: None,
                    at: b.sealed_at,
                })
            }),
            hidden: b.hidden.map(|h| (h.ran, h.passed)),
            review: b.review.as_ref().map(|r| (r.verdict.clone(), r.concerns)),
        }
    }
}

/// Dollars to the micro-dollar: sums of cents carry float noise
/// (`1.9400000000000002`) that does not belong on the wire.
pub(super) fn usd(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().trim().to_string()
}

/// The jobs of a mission, the one its verdict is read from first.
fn job_ids(m: &Mission, graph: Option<&GraphRun>) -> (Vec<JobId>, Option<JobId>) {
    let lead = graph.and_then(|g| g.final_job).or(m.job);
    let mut ids: Vec<JobId> = Vec::new();
    let mut add = |j: Option<JobId>| {
        if let Some(j) = j
            && !ids.contains(&j)
        {
            ids.push(j);
        }
    };
    add(lead);
    add(m.job);
    if let Some(g) = graph {
        for t in &g.tasks {
            add(t.job);
        }
    }
    (ids, lead)
}

/// A mission is an outcome once a job is out for it, or while its plan is
/// being made or waits for the user. A draft, a mission only linted and
/// one refused are the Missions screen's business.
fn started(src: &Sources<'_>, m: &Mission) -> bool {
    !matches!(
        m.state,
        MissionState::Draft | MissionState::Linted | MissionState::Refused
    ) || matches!(
        (src.plans)(&m.slug).map(|r| r.status),
        Some(DomainPlanStatus::Planning | DomainPlanStatus::Proposed)
    ) && m.state != MissionState::Refused
}

/// The id of the outcome `job` belongs to.
pub(super) fn outcome_of(src: &Sources<'_>, job: JobId) -> String {
    for m in src.missions.iter().filter(|m| started(src, m)) {
        let graph = (src.plans)(&m.slug).and_then(|r| r.graph.as_ref());
        if job_ids(m, graph).0.contains(&job) {
            return format!("mission:{}", m.slug);
        }
    }
    format!("job:{}", job.0)
}

/// The mission an inbox item is about, when its id says so
/// (`plan:<slug>`, `drift:<slug>`, `merge:<slug>:<task>`).
fn mission_of_item(id: &str) -> Option<&str> {
    let rest = ["plan:", "drift:", "merge:"]
        .iter()
        .find_map(|p| id.strip_prefix(p))?;
    rest.split(':').next()
}

/// Every outcome of the moment: the missions that started, then the plain
/// jobs that belong to none.
pub(super) fn build(
    src: &Sources<'_>,
    caches: &mut Caches,
    holds: &[HoldView],
    landing: LandingPolicy,
) -> Vec<Built> {
    let facts: HashMap<JobId, &JobFact> = src.facts.iter().map(|f| (f.job_id, f)).collect();
    let mut owned: HashSet<JobId> = HashSet::new();
    let mut out = Vec::new();
    for m in src.missions {
        let graph = (src.plans)(&m.slug).and_then(|r| r.graph.as_ref());
        owned.extend(job_ids(m, graph).0);
        if started(src, m) {
            out.push(mission(src, caches, holds, &facts, m, landing));
        }
    }
    for f in src
        .facts
        .iter()
        .filter(|f| f.review_of.is_none() && !owned.contains(&f.job_id))
    {
        out.push(plain(src, caches, holds, f));
    }
    out
}

fn title_of(f: &JobFact) -> String {
    f.prompt
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn job_state(s: JobStatus) -> JobState {
    s.into()
}

fn job_view(caches: &mut Caches, src: &Sources<'_>, f: &JobFact, task: Option<&str>) -> JobView {
    JobView {
        id: f.job_id.0,
        repo: caches
            .repo(f.session_id, src)
            .unwrap_or_else(|| f.session_name.clone()),
        session: f.session_name.clone(),
        model: f.model.clone(),
        state: job_state(f.status),
        task: task.map(str::to_string),
        spend_usd: usd(f.cost_usd),
        context_use: f.context,
        started_at: f.started_at,
        finished_at: f.finished_at,
        error: f.error.clone(),
    }
}

fn ask_view(i: &AttentionItem) -> AskView {
    AskView {
        id: i.id.to_string(),
        kind: i.kind.into(),
        priority: i.priority.into(),
        title: i.title.clone(),
        detail: i.detail.clone(),
        since_at: i.since,
        deadline_at: i.deadline,
        request: i.request,
        decides: rules::Ask::of(i).decides(),
    }
}

/// The folder-relative areas the live claims of `jobs` hold.
fn claimed(src: &Sources<'_>, jobs: &[JobId]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in src
        .claims
        .iter()
        .filter(|c| c.is_live_claim() && jobs.contains(&c.job))
    {
        for a in &c.areas {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out
}

fn drift_view(d: &DriftReport) -> DriftView {
    DriftView {
        changed: d.changed,
        outside: d.outside.clone(),
        outside_count: d.outside_count,
        tasks: d.tasks,
        untouched: d.untouched.clone(),
        score: d.score(),
        threshold: d.threshold,
        drifted: d.drifted(),
        summary: d.summary(),
    }
}

fn plan_view(record: &PlanRecord) -> Option<PlanView> {
    if record.status == DomainPlanStatus::Idle && record.plan.is_none() {
        return None;
    }
    let graph = record.graph.as_ref();
    let tasks = record
        .plan
        .as_ref()
        .map(|p| {
            p.tasks
                .iter()
                .map(|t| {
                    let run = graph.and_then(|g| g.task(&t.id));
                    PlanTaskView {
                        id: t.id.clone(),
                        title: t.title.clone(),
                        files: t.files.clone(),
                        depends_on: t.depends_on.clone(),
                        acceptance: t.acceptance.clone(),
                        state: run.map(|r| r.state.into()),
                        job: run.and_then(|r| r.job).map(|j| j.0),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Some(PlanView {
        status: PlanStatus::from(record.status),
        summary: record
            .plan
            .as_ref()
            .map(|p| p.summary.clone())
            .unwrap_or_default(),
        tasks,
        drift: record.drift.as_ref().map(drift_view),
        topology: graph.map(|g| g.topology.as_str().to_string()),
        rounds: record.rounds,
        note: Some(record.note.clone()).filter(|n| !n.is_empty()),
    })
}

fn run_status(r: &GateRun) -> CheckStatus {
    if r.spawn_error.is_some() {
        CheckStatus::Error
    } else if r.timed_out {
        CheckStatus::TimedOut
    } else if r.passed {
        if r.flaky {
            CheckStatus::Flaky
        } else {
            CheckStatus::Passed
        }
    } else {
        CheckStatus::Failed
    }
}

fn stage(key: StageKey, state: StageState, summary: &str, items: Vec<String>) -> StageView {
    StageView {
        key,
        state,
        summary: summary.to_string(),
        items,
        ran: None,
        passed: None,
    }
}

/// What judged the work: the gate as it ran this session when it did,
/// else what the sealed bundle kept.
fn checks_view(
    configured: &[String],
    gate: Option<&GateState>,
    detail: Option<&GateDetail>,
    sealed: Option<&Sealed>,
    drift: Option<&DriftReport>,
    review: Option<&ReviewState>,
) -> ChecksView {
    let (source, runs): (ChecksSource, Vec<CheckRunView>) = match (detail, sealed) {
        (Some(d), _) => (
            ChecksSource::Gate,
            d.runs
                .iter()
                .map(|r| CheckRunView {
                    name: r.name.clone(),
                    status: run_status(r),
                    duration_ms: r.duration_ms,
                    first_failure: r.first_failure.clone(),
                })
                .collect(),
        ),
        (None, Some(s)) => (ChecksSource::Bundle, s.runs.clone()),
        (None, None) => (ChecksSource::None, Vec::new()),
    };
    let passed = runs
        .iter()
        .filter(|r| matches!(r.status, CheckStatus::Passed | CheckStatus::Flaky))
        .count();
    let total = configured.len().max(runs.len());
    let gate_view = match (gate, detail.is_some() || sealed.is_none()) {
        (Some(g), true) => Some(GateView {
            verdict: g.outcome.into(),
            blocks: g.blocks,
            max_blocks: Some(g.max_blocks),
            at: g.at,
        }),
        _ => sealed.and_then(|s| s.gate),
    };

    let live = detail.is_some();
    let judged = live || sealed.is_some();
    let never = "the gate has not judged yet";
    let pins = match (detail, judged) {
        (Some(d), _) if !d.pins.is_empty() => stage(
            StageKey::ProtectedFiles,
            StageState::Failed,
            &format!("{} protected file(s) changed", d.pins.len()),
            d.pins.clone(),
        ),
        (_, true) => stage(
            StageKey::ProtectedFiles,
            StageState::Passed,
            "no protected file changed",
            Vec::new(),
        ),
        _ => stage(
            StageKey::ProtectedFiles,
            StageState::NotRun,
            never,
            Vec::new(),
        ),
    };
    let weakening = match (detail, judged) {
        (Some(d), _) if !d.tampering.is_empty() => stage(
            StageKey::TestWeakening,
            StageState::Failed,
            &format!(
                "{} test change(s) make the checks easier to pass",
                d.tampering.len()
            ),
            d.tampering.clone(),
        ),
        (_, true) => stage(
            StageKey::TestWeakening,
            StageState::Passed,
            "no test was weakened",
            Vec::new(),
        ),
        _ => stage(
            StageKey::TestWeakening,
            StageState::NotRun,
            never,
            Vec::new(),
        ),
    };
    let base_suite = match detail.map(|d| d.base_suite.as_ref()) {
        Some(Some((true, text))) => stage(
            StageKey::BaseSuite,
            StageState::Passed,
            &first_line(text),
            Vec::new(),
        ),
        Some(Some((false, text))) => stage(
            StageKey::BaseSuite,
            StageState::Failed,
            &first_line(text),
            text.lines().skip(1).map(str::to_string).collect(),
        ),
        Some(None) => stage(
            StageKey::BaseSuite,
            StageState::NotRun,
            "the base suite did not run",
            Vec::new(),
        ),
        None if sealed.is_some() => stage(
            StageKey::BaseSuite,
            StageState::NotRun,
            "not recorded in the bundle",
            Vec::new(),
        ),
        None => stage(StageKey::BaseSuite, StageState::NotRun, never, Vec::new()),
    };
    let hidden_counts: Option<(u32, u32)> = match (gate.and_then(|g| g.hidden), sealed) {
        (Some(h), _) if live => Some((h.ran, h.passed)),
        (_, Some(s)) => s.hidden,
        (Some(h), None) => Some((h.ran, h.passed)),
        _ => None,
    };
    let hidden = match hidden_counts {
        Some((ran, passed)) => StageView {
            key: StageKey::HiddenChecks,
            state: if ran == passed {
                StageState::Passed
            } else {
                StageState::Failed
            },
            summary: format!("{passed} of {ran} hidden checks passed"),
            items: Vec::new(),
            ran: Some(ran),
            passed: Some(passed),
        },
        None => stage(
            StageKey::HiddenChecks,
            StageState::NotRun,
            "no hidden checks ran",
            Vec::new(),
        ),
    };
    let gaps = match (detail, judged) {
        (Some(d), _) if !d.gaps.is_empty() => stage(
            StageKey::EvidenceGaps,
            StageState::Failed,
            &format!("{} claim(s) nothing backs", d.gaps.len()),
            d.gaps.clone(),
        ),
        (_, true) => stage(
            StageKey::EvidenceGaps,
            StageState::Passed,
            "every claim is backed by a run",
            Vec::new(),
        ),
        _ => stage(
            StageKey::EvidenceGaps,
            StageState::NotRun,
            never,
            Vec::new(),
        ),
    };
    let drift_stage = match drift {
        Some(d) => {
            let mut items: Vec<String> = d.outside.clone();
            items.extend(d.untouched.iter().map(|t| format!("task {t} untouched")));
            stage(
                StageKey::Drift,
                if d.drifted() {
                    StageState::Flagged
                } else {
                    StageState::Passed
                },
                &d.summary(),
                items,
            )
        }
        None => stage(
            StageKey::Drift,
            StageState::NotRun,
            "no plan to compare with",
            Vec::new(),
        ),
    };
    let review_stage = match (review, sealed.and_then(|s| s.review.as_ref())) {
        (Some(ReviewState::Done { review, .. }), _) => match review.verdict {
            ReviewVerdict::Pass => stage(
                StageKey::Review,
                StageState::Passed,
                "the reviewer found no concerns",
                Vec::new(),
            ),
            ReviewVerdict::Concerns => stage(
                StageKey::Review,
                StageState::Flagged,
                &format!("the reviewer raised {} concern(s)", review.concerns.len()),
                review.concerns.clone(),
            ),
        },
        (Some(ReviewState::Requested), _) => stage(
            StageKey::Review,
            StageState::NotRun,
            "a review is asked for",
            Vec::new(),
        ),
        (Some(ReviewState::Running { .. }), _) => stage(
            StageKey::Review,
            StageState::NotRun,
            "the review is running",
            Vec::new(),
        ),
        (Some(ReviewState::Failed { error, .. }), _) => {
            stage(StageKey::Review, StageState::NotRun, error, Vec::new())
        }
        (Some(ReviewState::Skipped { why }), _) => {
            stage(StageKey::Review, StageState::NotRun, why, Vec::new())
        }
        (None, Some((verdict, concerns))) if verdict == "concerns" => stage(
            StageKey::Review,
            StageState::Flagged,
            &format!("the reviewer raised {concerns} concern(s)"),
            Vec::new(),
        ),
        (None, Some(_)) => stage(
            StageKey::Review,
            StageState::Passed,
            "the reviewer found no concerns",
            Vec::new(),
        ),
        (None, None) => stage(
            StageKey::Review,
            StageState::NotRun,
            "no review was asked for",
            Vec::new(),
        ),
    };

    ChecksView {
        source,
        total: u32::try_from(total).unwrap_or(u32::MAX),
        passed: u32::try_from(passed).unwrap_or(u32::MAX),
        failed: u32::try_from(runs.len() - passed).unwrap_or(u32::MAX),
        runs,
        gate: gate_view,
        stages: vec![
            pins,
            weakening,
            base_suite,
            hidden,
            gaps,
            drift_stage,
            review_stage,
        ],
    }
}

fn claim_view(lead: Option<&JobFact>) -> Option<ClaimView> {
    let f = lead?;
    let text = f.result.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }
    let (text, truncated) = cut(text, CLAIM_MAX);
    Some(ClaimView {
        job: f.job_id.0,
        text,
        truncated,
    })
}

fn landing_view(m: &Mission, record: Option<&PlanRecord>) -> Option<LandingView> {
    let graph = record
        .and_then(|r| r.graph.as_ref())
        .filter(|g| !g.tasks.is_empty());
    if m.workspace.is_none() && graph.is_none() {
        return None;
    }
    let into = m
        .landing
        .as_ref()
        .map(|l| l.into.clone())
        .or_else(|| {
            graph
                .map(|g| g.integration.clone())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| integration_branch(&m.slug));
    let repos: Vec<LandingRepoView> = match &m.landing {
        Some(l) => l
            .repos
            .iter()
            .map(|r| LandingRepoView {
                repo: r.repo.clone(),
                branch: Some(r.branch.clone()).filter(|b| !b.is_empty()),
                state: r.state.into(),
                commit: r.commit.clone(),
                note: Some(r.note.clone()).filter(|n| !n.is_empty()),
            })
            .collect(),
        None => m
            .contract
            .repos
            .iter()
            .map(|name| LandingRepoView {
                repo: name.clone(),
                branch: None,
                state: LandState::Waiting,
                commit: None,
                note: None,
            })
            .collect(),
    };
    let queue = match (graph, record.and_then(|r| r.plan.as_ref())) {
        (Some(g), Some(p)) => g.queue(p),
        _ => Vec::new(),
    };
    let summary = match (&m.landing, graph) {
        (Some(l), _) => l.summary(),
        (None, Some(g)) => format!(
            "{} of {} tasks merged into {into}",
            g.merged.len(),
            g.tasks.len()
        ),
        (None, None) => format!(
            "{} repositories land in {into}, producers first, all or none",
            repos.len()
        ),
    };
    Some(LandingView {
        mode: if m.landing.is_some() || m.state == MissionState::Delivered {
            LandingMode::Result
        } else {
            LandingMode::Preview
        },
        into,
        repos,
        queue,
        complete: m.landing.as_ref().is_some_and(Landing::complete)
            || (graph.is_some() && m.state == MissionState::Delivered),
        summary,
        at: m.landing.as_ref().map(|l| l.at),
    })
}

fn actions(
    facts: &rules::ActionFacts,
    job: Option<JobId>,
    mission: Option<&str>,
    request: Option<u64>,
) -> Vec<ActionView> {
    rules::allowed(facts)
        .into_iter()
        .map(|a| ActionView {
            kind: ActionKind::from(a.kind),
            enabled: a.enabled,
            reason: a.reason,
            job: job.map(|j| j.0),
            mission: mission.map(str::to_string),
            request,
        })
        .collect()
}

fn gate_progress(g: Option<&GateState>) -> CheckProgress {
    let (total, passed) = g.map_or((0, 0), |g| {
        (g.checks.len(), g.checks.iter().filter(|c| c.1).count())
    });
    CheckProgress {
        total: u32::try_from(total).unwrap_or(u32::MAX),
        passed: u32::try_from(passed).unwrap_or(u32::MAX),
        failed: u32::try_from(total - passed).unwrap_or(u32::MAX),
    }
}

fn view_progress(c: &ChecksView) -> CheckProgress {
    CheckProgress {
        total: c.total,
        passed: c.passed,
        failed: c.failed,
    }
}

/// How a mission's stage reads on its card, and how far its work is.
fn stage_node(stage: rules::Stage) -> (NodeState, rules::Progress) {
    match stage {
        rules::Stage::Idle | rules::Stage::Queued => (NodeState::Queued, rules::Progress::Waiting),
        rules::Stage::Running => (NodeState::Running, rules::Progress::Working),
        rules::Stage::Verifying => (NodeState::Verifying, rules::Progress::Working),
        rules::Stage::Verified => (NodeState::Ready, rules::Progress::Working),
        rules::Stage::Landed => (NodeState::Done, rules::Progress::Done),
        rules::Stage::Escalated => (NodeState::Failed, rules::Progress::Failed),
    }
}

fn mission(
    src: &Sources<'_>,
    caches: &mut Caches,
    holds: &[HoldView],
    facts: &HashMap<JobId, &JobFact>,
    m: &Mission,
    landing: LandingPolicy,
) -> Built {
    let record = (src.plans)(&m.slug);
    let graph = record
        .and_then(|r| r.graph.as_ref())
        .filter(|g| !g.tasks.is_empty());
    let (ids, lead_id) = job_ids(m, graph);
    let id = format!("mission:{}", m.slug);
    let jobs: Vec<&JobFact> = ids.iter().filter_map(|j| facts.get(j).copied()).collect();
    let lead = lead_id.and_then(|j| facts.get(&j).copied());
    let gate = lead_id.and_then(|j| src.gates.get(&j));
    let detail = lead_id.and_then(|j| (src.gate_detail)(j));
    let verified = gate.is_some_and(|g| g.outcome == GateOutcome::Pass);

    let planning = record.is_some_and(|r| r.status == DomainPlanStatus::Planning);
    let stage = match m.state {
        MissionState::Draft | MissionState::Linted | MissionState::Refused if planning => {
            rules::Stage::Running
        }
        MissionState::Draft | MissionState::Linted | MissionState::Refused => rules::Stage::Idle,
        MissionState::Queued => rules::Stage::Queued,
        MissionState::Running => rules::Stage::Running,
        MissionState::Verifying if verified => rules::Stage::Verified,
        MissionState::Verifying => rules::Stage::Verifying,
        MissionState::Delivered => rules::Stage::Landed,
        MissionState::Escalated => rules::Stage::Escalated,
    };
    let since = match stage {
        rules::Stage::Verified => gate.map_or(m.updated_at, |g| g.at),
        rules::Stage::Verifying => lead.and_then(|f| f.finished_at).unwrap_or(m.updated_at),
        rules::Stage::Running => lead.and_then(|f| f.started_at).unwrap_or(m.updated_at),
        _ => m.updated_at,
    };

    let asks: Vec<&AttentionItem> = src
        .attention
        .iter()
        .filter(|i| {
            i.job.is_some_and(|j| ids.contains(&j))
                || mission_of_item(i.id.as_str()) == Some(m.slug.as_str())
        })
        .collect();
    let my_holds: Vec<&HoldView> = holds
        .iter()
        .filter(|h| h.state == HoldState::Held && h.outcome.as_deref() == Some(id.as_str()))
        .collect();

    let sealed = if m.state == MissionState::Delivered {
        caches.sealed(&m.slug, m.updated_at, || {
            (src.evidence)(&m.slug).map(|s| Sealed::of(&s))
        })
    } else {
        None
    };
    let drift = record.and_then(|r| r.drift.as_ref());
    let review = lead.and_then(|f| f.review.as_ref());
    let checks = checks_view(
        &m.contract.acceptance.checks,
        gate,
        detail,
        sealed.as_ref(),
        drift,
        review,
    );

    let job_views: Vec<JobView> = jobs
        .iter()
        .map(|f| {
            let task = graph
                .and_then(|g| g.task_of(f.job_id))
                .map(|t| t.id.as_str());
            job_view(caches, src, f, task)
        })
        .collect();

    // Not `sum()`: an empty one is `-0.0`, which would reach the JSON.
    let spent = jobs.iter().fold(0.0, |total, f| total + f.cost_usd);
    let total = usd(m.spend.total().max(spent));
    let live = jobs
        .iter()
        .filter(|f| matches!(f.status, JobStatus::Queued | JobStatus::Running))
        .count();
    let target = jobs
        .iter()
        .find(|f| matches!(f.status, JobStatus::Queued | JobStatus::Running))
        .map(|f| f.job_id)
        .or(lead_id);
    let plan_proposed = record.is_some_and(|r| r.status == DomainPlanStatus::Proposed);
    let action_facts = rules::ActionFacts {
        mission: Some(m.state),
        job: lead.map(|f| f.status),
        live_jobs: live,
        held_calls: my_holds.len(),
        plan_proposed,
        evidence: sealed.is_some(),
        landing: rules::LandFacts {
            workspace: m.workspace.is_some(),
            verified: stage == rules::Stage::Verified,
            manual: landing == LandingPolicy::Manual,
        },
    };
    let first_request = my_holds.first().and_then(|h| h.request);

    let title = if m.contract.title.trim().is_empty() {
        m.slug.clone()
    } else {
        m.contract.title.trim().to_string()
    };
    let view = OutcomeView {
        id: id.clone(),
        kind: OutcomeKind::Mission,
        title: title.clone(),
        mission: Some(m.slug.clone()),
        lifecycle: lifecycle_of(m.state),
        group: GroupKind::Running,
        priority: None,
        since_at: since,
        created_at: m.created_at,
        updated_at: m.updated_at,
        asks: asks.iter().map(|i| ask_view(i)).collect(),
        holds: my_holds.iter().map(|h| h.id.clone()).collect(),
        contract: Some(ContractView {
            goal: m.contract.goal.clone(),
            repos: m.contract.repos.clone(),
            done_when: m.contract.acceptance.checks.clone(),
            human_signoff: m.contract.acceptance.human_signoff,
            protected: m.contract.constraints.protected.clone(),
            work_in: m.contract.constraints.paths_rw.clone(),
            budget: BudgetView {
                usd: m.contract.budget.usd,
                wall_minutes: m.contract.budget.wall_minutes,
                on_exhaust: m.contract.budget.on_exhaust.into(),
            },
            note: m.note.clone(),
        }),
        jobs: job_views.clone(),
        plan: record.and_then(plan_view),
        checks: checks.clone(),
        claim: claim_view(lead),
        evidence: sealed.as_ref().map(|s| s.evidence.clone()),
        diff: sealed.as_ref().map(|s| s.diff.clone()),
        landing: landing_view(m, record),
        spend: SpendView {
            total_usd: total,
            plan_usd: usd(m.spend.plan),
            implement_usd: usd(m.spend.implement),
            verify_usd: usd(m.spend.verify),
            review_usd: usd(m.spend.review),
        },
        actions: actions(&action_facts, target, Some(&m.slug), first_request),
    };

    // The Deck's cards: one per task of a graph, else one for the mission.
    let mut cards = Vec::new();
    let mut task_facts = None;
    let (outcome_state, outcome_progress) = stage_node(stage);
    let outcome_repos: Vec<String> = if m.contract.repos.is_empty() {
        job_views
            .first()
            .map(|j| vec![j.repo.clone()])
            .unwrap_or_default()
    } else {
        m.contract.repos.clone()
    };
    match (graph, record.and_then(|r| r.plan.as_ref())) {
        (Some(g), Some(plan)) => {
            let mut node_of = HashMap::new();
            let mut progress = HashMap::new();
            for t in &plan.tasks {
                let run = g.task(&t.id);
                let task_job = run.and_then(|r| r.job);
                let fact = task_job.and_then(|j| facts.get(&j).copied());
                let node_id = format!("task:{}:{}", m.slug, t.id);
                let task_state = run.map_or(DomainTaskState::Waiting, |r| r.state);
                let (state, prog) = match task_state {
                    DomainTaskState::Waiting => (NodeState::Waiting, rules::Progress::Waiting),
                    DomainTaskState::Running => (
                        if fact.is_some_and(|f| f.status == JobStatus::Queued) {
                            NodeState::Queued
                        } else {
                            NodeState::Running
                        },
                        rules::Progress::Working,
                    ),
                    DomainTaskState::Gate => (NodeState::Verifying, rules::Progress::Working),
                    DomainTaskState::Merging => (NodeState::Ready, rules::Progress::Working),
                    DomainTaskState::Merged => (NodeState::Done, rules::Progress::Done),
                    DomainTaskState::Conflict | DomainTaskState::Failed => {
                        (NodeState::Failed, rules::Progress::Failed)
                    }
                };
                let repo = fact.and_then(|f| caches.repo(f.session_id, src));
                let mut claims = t.files.clone();
                if let Some(j) = task_job {
                    for a in claimed(src, &[j]) {
                        if !claims.contains(&a) {
                            claims.push(a);
                        }
                    }
                }
                node_of.insert(t.id.clone(), node_id.clone());
                progress.insert(t.id.clone(), prog);
                cards.push(Card {
                    node: DeckNode {
                        id: node_id.clone(),
                        kind: NodeKind::Task,
                        outcome: id.clone(),
                        title: t.title.clone(),
                        repo: repo.clone(),
                        state,
                        group: None,
                        checks: gate_progress(task_job.and_then(|j| src.gates.get(&j))),
                        depth: 0,
                        spend_usd: usd(fact.map_or(0.0, |f| f.cost_usd)),
                        job: task_job.map(|j| j.0),
                    },
                    facts: rules::Node {
                        id: node_id,
                        repos: repo.into_iter().collect(),
                        claims,
                        progress: prog,
                    },
                });
            }
            task_facts = Some(TaskGraphFacts {
                plan: plan.clone(),
                node_of,
                progress,
            });
        }
        _ => {
            let mut claims = claimed(src, &ids);
            if let Some(plan) = record.and_then(|r| r.approved())
                && matches!(
                    m.state,
                    MissionState::Queued | MissionState::Running | MissionState::Verifying
                )
            {
                for a in plan.areas() {
                    if !claims.iter().any(|c| c == a) {
                        claims.push(a.to_string());
                    }
                }
            }
            cards.push(Card {
                node: DeckNode {
                    id: id.clone(),
                    kind: NodeKind::Job,
                    outcome: id.clone(),
                    title,
                    repo: job_views.first().map(|j| j.repo.clone()),
                    state: outcome_state,
                    group: None,
                    checks: view_progress(&checks),
                    depth: 0,
                    spend_usd: total,
                    job: lead_id.map(|j| j.0),
                },
                facts: rules::Node {
                    id: id.clone(),
                    repos: outcome_repos,
                    claims,
                    progress: outcome_progress,
                },
            });
        }
    }

    Built {
        standing: rules::Standing {
            id,
            stage,
            since,
            asks: asks.iter().map(|i| rules::Ask::of(i)).collect(),
        },
        view,
        cards,
        tasks: task_facts,
    }
}

fn lifecycle_of(s: MissionState) -> Lifecycle {
    match s {
        MissionState::Draft => Lifecycle::Draft,
        MissionState::Linted => Lifecycle::Linted,
        MissionState::Queued => Lifecycle::Queued,
        MissionState::Running => Lifecycle::Running,
        MissionState::Verifying => Lifecycle::Verifying,
        MissionState::Delivered => Lifecycle::Delivered,
        MissionState::Escalated => Lifecycle::Escalated,
        MissionState::Refused => Lifecycle::Refused,
    }
}

fn plain(src: &Sources<'_>, caches: &mut Caches, holds: &[HoldView], f: &JobFact) -> Built {
    let id = format!("job:{}", f.job_id.0);
    let gate = src.gates.get(&f.job_id);
    let detail = (src.gate_detail)(f.job_id);
    let verified =
        f.status == JobStatus::Done && gate.is_some_and(|g| g.outcome == GateOutcome::Pass);
    let stage = match f.status {
        JobStatus::Queued => rules::Stage::Queued,
        JobStatus::Running => rules::Stage::Running,
        JobStatus::Done if verified => rules::Stage::Verified,
        _ => rules::Stage::Idle,
    };
    let ended = f.finished_at.unwrap_or(f.created_at);
    let since = match stage {
        rules::Stage::Queued => f.created_at,
        rules::Stage::Running => f.started_at.unwrap_or(f.created_at),
        rules::Stage::Verified => gate.map_or(ended, |g| g.at),
        _ => ended,
    };
    let asks: Vec<&AttentionItem> = src
        .attention
        .iter()
        .filter(|i| i.job == Some(f.job_id))
        .collect();
    let my_holds: Vec<&HoldView> = holds
        .iter()
        .filter(|h| h.state == HoldState::Held && h.outcome.as_deref() == Some(id.as_str()))
        .collect();
    let review = f.review.as_ref();
    let checks = checks_view(&[], gate, detail, None, None, review);
    let job = job_view(caches, src, f, None);
    let live = usize::from(matches!(f.status, JobStatus::Queued | JobStatus::Running));
    let action_facts = rules::ActionFacts {
        mission: None,
        job: Some(f.status),
        live_jobs: live,
        held_calls: my_holds.len(),
        plan_proposed: false,
        evidence: false,
        landing: rules::LandFacts::default(),
    };
    let title = title_of(f);
    let view = OutcomeView {
        id: id.clone(),
        kind: OutcomeKind::Job,
        title: title.clone(),
        mission: None,
        lifecycle: match f.status {
            JobStatus::Queued => Lifecycle::Queued,
            JobStatus::Running => Lifecycle::Running,
            JobStatus::Done => Lifecycle::Done,
            JobStatus::Failed => Lifecycle::Failed,
            JobStatus::Interrupted => Lifecycle::Interrupted,
            JobStatus::Cancelled => Lifecycle::Cancelled,
        },
        group: GroupKind::Running,
        priority: None,
        since_at: since,
        created_at: f.created_at,
        updated_at: f.finished_at.or(f.last_event_at).unwrap_or(f.created_at),
        asks: asks.iter().map(|i| ask_view(i)).collect(),
        holds: my_holds.iter().map(|h| h.id.clone()).collect(),
        contract: None,
        jobs: vec![job.clone()],
        plan: None,
        checks: checks.clone(),
        claim: claim_view(Some(f)),
        evidence: None,
        diff: None,
        landing: None,
        spend: SpendView {
            total_usd: usd(f.cost_usd),
            plan_usd: 0.0,
            implement_usd: usd(f.cost_usd),
            verify_usd: 0.0,
            review_usd: 0.0,
        },
        actions: actions(
            &action_facts,
            Some(f.job_id),
            None,
            my_holds.first().and_then(|h| h.request),
        ),
    };
    let (state, progress) = match f.status {
        JobStatus::Queued => (NodeState::Queued, rules::Progress::Waiting),
        JobStatus::Running => (NodeState::Running, rules::Progress::Working),
        JobStatus::Done if verified => (NodeState::Ready, rules::Progress::Working),
        JobStatus::Done => (NodeState::Done, rules::Progress::Done),
        JobStatus::Failed | JobStatus::Interrupted => (NodeState::Failed, rules::Progress::Failed),
        JobStatus::Cancelled => (NodeState::Stopped, rules::Progress::Failed),
    };
    let claims = claimed(src, &[f.job_id]);
    let card = Card {
        node: DeckNode {
            id: id.clone(),
            kind: NodeKind::Job,
            outcome: id.clone(),
            title,
            repo: Some(job.repo.clone()),
            state,
            group: None,
            checks: view_progress(&checks),
            depth: 0,
            spend_usd: usd(f.cost_usd),
            job: Some(f.job_id.0),
        },
        facts: rules::Node {
            id: id.clone(),
            repos: vec![job.repo],
            claims,
            progress,
        },
    };
    Built {
        standing: rules::Standing {
            id,
            stage,
            since,
            asks: asks.iter().map(|i| rules::Ask::of(i)).collect(),
        },
        view,
        cards: vec![card],
        tasks: None,
    }
}
