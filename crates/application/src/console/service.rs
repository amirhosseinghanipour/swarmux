//! The console service: gathers the inputs, rebuilds the snapshot when
//! they change, and keeps what is expensive to derive.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use swarmux_domain::board::BoardEntry;
use swarmux_domain::console as rules;
use swarmux_domain::contracts::Index;
use swarmux_domain::mission::Mission;
use swarmux_domain::plan::PlanRecord;
use swarmux_domain::{AttentionItem, JobId, Millis, Session, SessionId, WorkspaceId};

use super::outcome::Sealed;
use super::view::{
    ConsoleSnapshot, Counts, GroupKind, GroupView, Header, LimitWindow, QueueFigures,
    SCHEMA_VERSION, SpendFigures,
};
use super::{deck, hold, outcome};
use crate::engine::Engine;
use crate::evidence::{EvidenceService, Stored};
use crate::gate::{GateDetail, GateState};
use crate::jobs::{JobFact, QueueStats};
use crate::ports::JobRepository;
use crate::{DecisionRecord, PendingApproval, TelemetrySnapshot};

/// Outcomes the landed group holds in full; the rest is counted.
pub const DEFAULT_LANDED: usize = 30;

/// Looks up a mission's plan record.
pub type PlanLookup<'a> = Box<dyn Fn(&str) -> Option<&'a PlanRecord> + 'a>;

/// Reads a delivered mission's sealed bundle.
pub type EvidenceLookup<'a> = Box<dyn Fn(&str) -> Option<Stored> + 'a>;

/// What the console reads. Plain borrowed data and lookups, so a snapshot
/// can be built from the live engine, from the demo's, or from a test's
/// literals alike.
pub struct Sources<'a> {
    pub now: Millis,
    /// The jobs of this run.
    pub facts: &'a [JobFact],
    /// The open asks of the attention inbox.
    pub attention: &'a [AttentionItem],
    /// Calls held for the user, oldest first.
    pub approvals: &'a [PendingApproval],
    /// The Guard's decisions of this run, newest first.
    pub decisions: &'a [DecisionRecord],
    pub missions: &'a [Mission],
    /// The board's entries; the live claims are read from them.
    pub claims: &'a [BoardEntry],
    /// The latest gate verdict of each job.
    pub gates: &'a HashMap<JobId, GateState>,
    pub queue: QueueStats,
    pub telemetry: Option<&'a TelemetrySnapshot>,
    /// Where the sessions' streams are stored.
    pub stream: &'a dyn JobRepository,
    /// A mission's plan record.
    pub plans: PlanLookup<'a>,
    /// What a job's latest verdict found, in full.
    pub gate_detail: Box<dyn Fn(JobId) -> Option<&'a GateDetail> + 'a>,
    /// What the workspace's repositories provide and use.
    pub contracts: Box<dyn Fn(WorkspaceId) -> Option<Arc<Index>> + 'a>,
    /// A delivered mission's sealed bundle.
    pub evidence: EvidenceLookup<'a>,
    /// A session, for the folder its jobs work in.
    pub sessions: Box<dyn Fn(SessionId) -> Option<Session> + 'a>,
}

/// What is copied out of the engine, since the engine hands it out by
/// value.
pub struct Gathered {
    pub facts: Vec<JobFact>,
    pub approvals: Vec<PendingApproval>,
    pub decisions: Vec<DecisionRecord>,
    pub claims: Vec<BoardEntry>,
    pub gates: HashMap<JobId, GateState>,
    pub queue: QueueStats,
}

impl Gathered {
    /// Reads the engine's jobs, the Guard, the gate and the board.
    pub fn of(engine: &Engine) -> Gathered {
        Gathered {
            facts: engine.jobs.facts(),
            approvals: engine.guard.approvals(),
            decisions: engine.guard.recent().cloned().collect(),
            claims: engine
                .board
                .as_ref()
                .map(|(service, _)| service.board().entries())
                .unwrap_or_default(),
            gates: engine
                .gate
                .as_ref()
                .map(|g| g.states().clone())
                .unwrap_or_default(),
            queue: engine.jobs.stats(),
        }
    }

    /// The sources over `self` and the engine. `attention` are the open
    /// asks ([`crate::AttentionService::open`]), `evidence` where sealed
    /// bundles are read, `stream` where the sessions' streams are stored.
    pub fn sources<'a>(&'a self, engine: &'a Engine, extra: Extra<'a>) -> Sources<'a> {
        let Extra {
            now,
            attention,
            evidence,
            telemetry,
            stream,
        } = extra;
        Sources {
            now,
            facts: &self.facts,
            attention,
            approvals: &self.approvals,
            decisions: &self.decisions,
            missions: engine.missions.as_ref().map_or(&[], |m| m.missions()),
            claims: &self.claims,
            gates: &self.gates,
            queue: self.queue,
            telemetry,
            stream,
            plans: Box::new(move |slug: &str| engine.missions.as_ref().and_then(|m| m.plan(slug))),
            gate_detail: Box::new(move |job: JobId| {
                engine.gate.as_ref().and_then(|g| g.detail(job))
            }),
            contracts: Box::new(move |ws: WorkspaceId| {
                engine.contracts.as_ref().and_then(|c| c.index(ws).ok())
            }),
            evidence: Box::new(move |slug: &str| evidence.and_then(|e| e.get(slug).ok()).flatten()),
            sessions: Box::new(move |id: SessionId| engine.jobs.session(id).ok().flatten()),
        }
    }
}

/// What [`Gathered::sources`] needs besides the engine.
pub struct Extra<'a> {
    pub now: Millis,
    pub attention: &'a [AttentionItem],
    pub evidence: Option<&'a EvidenceService>,
    pub telemetry: Option<&'a TelemetrySnapshot>,
    pub stream: &'a dyn JobRepository,
}

/// What is kept between snapshots.
#[derive(Default)]
pub(super) struct Caches {
    /// The folder name a session works in.
    repos: HashMap<SessionId, String>,
    /// A delivered mission's sealed bundle, by slug, with the time of the
    /// mission when it was read: a sealed bundle does not change.
    sealed: HashMap<String, (Millis, Option<Sealed>)>,
    /// What a workspace's repositories link, with the index it came from.
    flows: HashMap<WorkspaceId, (usize, Vec<rules::ContractFlow>)>,
}

impl Caches {
    pub(super) fn repo(&mut self, id: SessionId, src: &Sources<'_>) -> Option<String> {
        if let Some(r) = self.repos.get(&id) {
            return Some(r.clone());
        }
        let session = (src.sessions)(id)?;
        let name = session
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or(session.name);
        self.repos.insert(id, name.clone());
        Some(name)
    }

    /// The sealed evidence of `slug`, read once for as long as the mission
    /// stays as it was.
    pub(super) fn sealed(
        &mut self,
        slug: &str,
        updated_at: Millis,
        read: impl FnOnce() -> Option<Sealed>,
    ) -> Option<Sealed> {
        if let Some((at, sealed)) = self.sealed.get(slug)
            && *at == updated_at
            && sealed.is_some()
        {
            return sealed.clone();
        }
        let sealed = read();
        self.sealed
            .insert(slug.to_string(), (updated_at, sealed.clone()));
        sealed
    }

    pub(super) fn flows(&mut self, ws: WorkspaceId, src: &Sources<'_>) -> Vec<rules::ContractFlow> {
        let Some(index) = (src.contracts)(ws) else {
            return Vec::new();
        };
        let key = Arc::as_ptr(&index) as usize;
        if let Some((k, flows)) = self.flows.get(&ws)
            && *k == key
        {
            return flows.clone();
        }
        let flows = flows_of(&index);
        self.flows.insert(ws, (key, flows.clone()));
        flows
    }
}

/// The repositories that provide something another uses, with the symbols.
fn flows_of(index: &Index) -> Vec<rules::ContractFlow> {
    let mut by_pair: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for p in index.provided() {
        for site in index.consumers_of(p) {
            if site.repo == p.repo {
                continue;
            }
            by_pair
                .entry((p.repo.clone(), site.repo.clone()))
                .or_default()
                .push(p.sym.label());
        }
    }
    by_pair
        .into_iter()
        .map(|((provider, consumer), mut symbols)| {
            symbols.sort();
            symbols.dedup();
            rules::ContractFlow {
                provider,
                consumer,
                symbols,
            }
        })
        .collect()
}

/// Builds snapshots, and rebuilds one only when its inputs changed.
pub struct ConsoleService {
    landed: usize,
    revision: u64,
    last: Option<(u64, ConsoleSnapshot)>,
    caches: Caches,
}

impl Default for ConsoleService {
    fn default() -> ConsoleService {
        ConsoleService::new()
    }
}

impl ConsoleService {
    pub fn new() -> ConsoleService {
        ConsoleService {
            landed: DEFAULT_LANDED,
            revision: 0,
            last: None,
            caches: Caches::default(),
        }
    }

    /// Holds `n` outcomes of the landed group in full.
    pub fn with_landed(mut self, n: usize) -> ConsoleService {
        self.landed = n;
        self
    }

    /// The snapshot of `src`: the one built last when nothing it depends
    /// on changed, else a new one.
    pub fn snapshot(&mut self, src: &Sources<'_>) -> &ConsoleSnapshot {
        let stamp = stamp(src);
        let fresh = self.last.as_ref().is_some_and(|(s, _)| *s == stamp);
        if !fresh {
            self.revision += 1;
            let snap = build(src, &mut self.caches, self.revision, self.landed);
            self.last = Some((stamp, snap));
        }
        match &self.last {
            Some((_, snap)) => snap,
            None => unreachable!("a snapshot was just built"),
        }
    }

    /// The latest snapshot, if one was built.
    pub fn current(&self) -> Option<&ConsoleSnapshot> {
        self.last.as_ref().map(|(_, s)| s)
    }

    /// How many snapshots were built (not served from the last one).
    pub fn rebuilds(&self) -> u64 {
        self.revision
    }
}

/// A fingerprint of everything a snapshot depends on. Equal inputs give
/// an equal stamp; the countdown of a held call is not part of it (its
/// deadline is absolute), and neither is the time.
fn stamp(src: &Sources<'_>) -> u64 {
    let mut h = DefaultHasher::new();
    for f in src.facts {
        f.job_id.0.hash(&mut h);
        f.status.as_str().hash(&mut h);
        f.cost_usd.to_bits().hash(&mut h);
        f.context.map(f64::to_bits).hash(&mut h);
        (f.started_at, f.finished_at).hash(&mut h);
        f.error.hash(&mut h);
        f.result.hash(&mut h);
        format!("{:?}", f.review).hash(&mut h);
    }
    for i in src.attention {
        (i.id.as_str(), i.priority.as_str(), i.deadline, i.since).hash(&mut h);
        i.detail.hash(&mut h);
    }
    for a in src.approvals {
        (a.request.request_id, &a.rule).hash(&mut h);
    }
    (
        src.decisions.len(),
        src.decisions.first().map(|d| (d.ts, d.outcome.as_str())),
    )
        .hash(&mut h);
    for m in src.missions {
        (
            m.slug.as_str(),
            m.state.as_str(),
            m.updated_at,
            m.job.map(|j| j.0),
        )
            .hash(&mut h);
        m.landing.is_some().hash(&mut h);
        if let Some(r) = (src.plans)(&m.slug) {
            (
                r.status.as_str(),
                r.rounds,
                r.drift.as_ref().map(|d| d.summary()),
            )
                .hash(&mut h);
            if let Some(g) = &r.graph {
                for t in &g.tasks {
                    (t.id.as_str(), t.state.as_str(), t.job.map(|j| j.0)).hash(&mut h);
                }
                g.merged.len().hash(&mut h);
            }
        }
    }
    let mut gates: Vec<_> = src.gates.iter().collect();
    gates.sort_by_key(|(job, _)| job.0);
    for (job, g) in gates {
        (job.0, g.outcome.as_str(), g.at).hash(&mut h);
    }
    for c in src.claims.iter().filter(|c| c.is_live_claim()) {
        (c.job.0, &c.areas).hash(&mut h);
    }
    let q = src.queue;
    (
        q.running,
        q.queued,
        q.max_concurrent,
        q.deferred,
        q.awaiting_reset,
        q.held,
    )
        .hash(&mut h);
    src.telemetry.map(|t| t.taken_at).hash(&mut h);
    h.finish()
}

/// Builds a snapshot from scratch.
fn build(src: &Sources<'_>, caches: &mut Caches, revision: u64, landed: usize) -> ConsoleSnapshot {
    let holds = hold::build(src);
    let built = outcome::build(src, caches, &holds);

    let standings: Vec<rules::Standing> = built.iter().map(|b| b.standing.clone()).collect();
    let ranked = rules::rank(&standings);
    let placed: Vec<bool> = {
        let mut p = vec![false; built.len()];
        for r in &ranked {
            p[r.index] = true;
        }
        p
    };

    let mut groups: Vec<GroupView> = rules::Group::ALL
        .into_iter()
        .map(|g| GroupView {
            kind: GroupKind::from(g),
            total: 0,
            outcomes: Vec::new(),
        })
        .collect();
    for r in &ranked {
        let mut view = built[r.index].view.clone();
        view.group = GroupKind::from(r.placement.group);
        view.priority = r.placement.priority.map(Into::into);
        let slot = &mut groups[rules::Group::ALL
            .iter()
            .position(|g| *g == r.placement.group)
            .unwrap_or(0)];
        slot.total += 1;
        let keep = r.placement.group != rules::Group::Landed || slot.outcomes.len() < landed;
        if keep {
            slot.outcomes.push(view);
        }
    }
    let counts = Counts {
        decide: groups[0].total,
        ready_to_land: groups[1].total,
        running: groups[2].total,
        landed: groups[3].total,
        idle: placed.iter().filter(|p| !**p).count(),
    };

    let groups_of: Vec<Option<GroupKind>> = ranked_groups(&built, &ranked);
    let deck = deck::build(src, caches, &built, &groups_of);

    let header = Header {
        counts,
        queue: QueueFigures {
            running: src.queue.running,
            queued: src.queue.queued,
            max_concurrent: src.queue.max_concurrent,
            deferred: src.queue.deferred,
            awaiting_reset: src.queue.awaiting_reset,
            held_for_attention: src.queue.held,
        },
        limits: limits(src),
        spend: SpendFigures {
            today_usd: usage(src).map(|u| outcome::usd(u.today.cost)),
            week_usd: usage(src).map(|u| outcome::usd(u.week.cost)),
            // Folded from 0.0, not `sum()`: an empty sum is `-0.0`.
            missions_usd: outcome::usd(
                built
                    .iter()
                    .filter(|b| b.view.mission.is_some())
                    .fold(0.0, |total, b| total + b.view.spend.total_usd),
            ),
            jobs_usd: outcome::usd(src.facts.iter().fold(0.0, |total, f| total + f.cost_usd)),
        },
    };
    ConsoleSnapshot {
        schema: SCHEMA_VERSION,
        revision,
        taken_at: src.now,
        header,
        groups,
        holds,
        deck,
    }
}

/// The group of every built outcome, `None` on no queue.
fn ranked_groups(built: &[outcome::Built], ranked: &[rules::Ranked]) -> Vec<Option<GroupKind>> {
    let mut out = vec![None; built.len()];
    for r in ranked {
        out[r.index] = Some(GroupKind::from(r.placement.group));
    }
    out
}

fn usage<'a>(src: &'a Sources<'_>) -> Option<&'a swarmux_domain::telemetry::UsageReport> {
    src.telemetry.and_then(|t| t.usage.data.as_ref())
}

fn limits(src: &Sources<'_>) -> Vec<LimitWindow> {
    src.telemetry
        .map(|t| {
            t.limits
                .iter()
                .map(|l| LimitWindow {
                    window: l.window.clone(),
                    status: l.status.clone(),
                    utilization: l.utilization,
                    resets_at: l.resets_at,
                    hits_at: l.projection.map(|p| p.hits_at),
                    stale: l.stale,
                })
                .collect()
        })
        .unwrap_or_default()
}
