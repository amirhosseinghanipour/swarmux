//! Task graphs: an approved plan whose tasks can run side by side is built
//! as one job per task instead of one job for everything.
//!
//! Each task's job gets its own session and a worktree branched from the
//! mission's integration branch (which holds the work of the tasks it
//! depends on), and claims the files the plan gave it: writes outside the
//! claim are refused in `PreToolUse` by [`ScopeClaims`], and two tasks
//! with overlapping claims never run at once. A task whose job ended and
//! whose gate passed joins the merge queue, which merges it into the
//! integration branch in dependency order. A conflict sends the task's
//! agent back once to rebase; a second one goes to the user. Once every
//! task merged, the last task's branch is brought up to the integration
//! branch and the mission is verified and delivered from there, as a
//! single-job mission is.
//!
//! The git side is the [`MergeQueue`] port.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::budget::Reservation;
use swarmux_domain::gate::{GateOutcome, written_paths};
use swarmux_domain::mission::{
    MissionGate, MissionPhase, MissionState, OnExhaust, Signal, compile,
};
use swarmux_domain::plan::{Plan, PlanRecord, is_incidental};
use swarmux_domain::taskgraph::{
    GraphRun, TaskState, Topology, conflict_brief, in_claim, integration_branch, out_of_scope,
    scope_reason, task_brief, width,
};
use swarmux_domain::{AttentionItem, JobId, JobStatus, Millis};

use super::{MissionReport, MissionService, refused};
use crate::gate::{GateState, WriteFence};
use crate::jobs::{JobFact, JobService};
use crate::worktree::WorktreeService;
use crate::{AppError, AppResult};

/// What merging a branch did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The branch is in; `commit` is the merge.
    Merged { commit: String },
    /// It conflicts in these files; nothing changed.
    Conflict { files: Vec<String> },
}

/// The git side of the merge queue. Only branches swarmux made are ever
/// written: the integration branch and the tasks' own.
pub trait MergeQueue {
    /// Makes sure `branch` exists (starting at the repository's HEAD when
    /// it is new) and is checked out in a folder of swarmux's own; returns
    /// that folder.
    fn open(&self, repo: &Path, branch: &str) -> AppResult<PathBuf>;
    /// The files `branch` changed since it left the branch checked out at
    /// `workdir`, relative to the repository.
    fn changed(&self, workdir: &Path, branch: &str) -> AppResult<Vec<String>>;
    /// Merges `branch` into the branch checked out at `workdir`. A
    /// conflict is undone before this returns.
    fn merge(&self, workdir: &Path, branch: &str, message: &str) -> AppResult<MergeOutcome>;
    /// Moves the branch checked out at `worktree` forward to `branch`;
    /// fails unless that is a fast-forward.
    fn fast_forward(&self, worktree: &Path, branch: &str) -> AppResult<()>;
}

/// A task's claim on the files its plan named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub mission: String,
    pub task: String,
    pub areas: Vec<String>,
    /// The worktree's folder, once the job has one.
    pub root: Option<String>,
}

/// A write refused for leaving its task's scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeDenial {
    pub job: JobId,
    pub mission: String,
    pub task: String,
    pub tool: String,
    pub path: String,
}

/// The claims of running tasks, shared with the hook transport: a write
/// outside its job's claim is refused before any rule is consulted.
#[derive(Debug, Default)]
pub struct ScopeClaims {
    held: Mutex<HashMap<JobId, Claim>>,
    denials: Mutex<Vec<ScopeDenial>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ScopeClaims {
    pub fn new() -> Arc<ScopeClaims> {
        Arc::new(ScopeClaims::default())
    }

    pub fn claim(&self, job: JobId, claim: Claim) {
        lock(&self.held).insert(job, claim);
    }

    /// Where `job`'s worktree is; whether that is news.
    pub fn set_root(&self, job: JobId, root: &str) -> bool {
        let mut held = lock(&self.held);
        match held.get_mut(&job) {
            Some(c) if c.root.as_deref() != Some(root) => {
                c.root = Some(root.to_string());
                true
            }
            _ => false,
        }
    }

    pub fn release(&self, job: JobId) -> Option<Claim> {
        lock(&self.held).remove(&job)
    }

    pub fn get(&self, job: JobId) -> Option<Claim> {
        lock(&self.held).get(&job).cloned()
    }

    /// Every claim held, by job.
    pub fn held(&self) -> Vec<(JobId, Claim)> {
        let mut all: Vec<(JobId, Claim)> = lock(&self.held)
            .iter()
            .map(|(j, c)| (*j, c.clone()))
            .collect();
        all.sort_by_key(|(j, _)| *j);
        all
    }

    /// The writes refused since the last call.
    pub fn take_denials(&self) -> Vec<ScopeDenial> {
        std::mem::take(&mut *lock(&self.denials))
    }
}

impl WriteFence for ScopeClaims {
    fn refuses(&self, job: Option<JobId>, tool: &str, subject: &str, cwd: &str) -> Option<String> {
        let job = job?;
        let claim = self.get(job)?;
        if written_paths(tool, subject).is_empty() {
            return None;
        }
        let Some(root) = &claim.root else {
            // The worktree is not known yet: nothing is let through blind.
            return Some(format!(
                "swarmux is still setting up task {}'s scope; try again in a moment (scope claim)",
                claim.task
            ));
        };
        let path = out_of_scope(&claim.areas, root, tool, subject, cwd)?;
        lock(&self.denials).push(ScopeDenial {
            job,
            mission: claim.mission.clone(),
            task: claim.task.clone(),
            tool: tool.to_string(),
            path: path.clone(),
        });
        Some(scope_reason(&claim.task, &claim.areas, &path))
    }
}

/// What task graphs need, held by the mission service.
#[derive(Default)]
pub(super) struct GraphState {
    merge: Option<Rc<dyn MergeQueue>>,
    worktrees: Option<Rc<WorktreeService>>,
    claims: Option<Arc<ScopeClaims>>,
}

/// The deps a running graph needs, cloned out of the service.
struct Deps {
    merge: Rc<dyn MergeQueue>,
    worktrees: Rc<WorktreeService>,
    claims: Arc<ScopeClaims>,
}

impl MissionService {
    /// Lets approved plans run as task graphs: `merge` merges the tasks'
    /// branches, `worktrees` gives each task its own, and `claims` is the
    /// fence the Guard consults.
    pub fn with_graph(
        mut self,
        merge: Rc<dyn MergeQueue>,
        worktrees: Rc<WorktreeService>,
        claims: Arc<ScopeClaims>,
    ) -> MissionService {
        self.graphs = GraphState {
            merge: Some(merge),
            worktrees: Some(worktrees),
            claims: Some(claims),
        };
        self
    }

    fn deps(&self) -> Option<Deps> {
        let g = &self.graphs;
        Some(Deps {
            merge: g.merge.clone()?,
            worktrees: g.worktrees.clone()?,
            claims: g.claims.clone()?,
        })
    }

    /// The mission's task graph, once its plan is being built as one.
    pub fn graph(&self, slug: &str) -> Option<&GraphRun> {
        self.plan(slug)?.graph.as_ref()
    }

    /// How the mission's approved plan is built: one job per task when
    /// its tasks can run side by side and each can have a worktree, else
    /// one job.
    pub fn topology(&self, slug: &str, jobs: &JobService) -> Topology {
        let Some(plan) = self.plan(slug).and_then(PlanRecord::approved) else {
            return Topology::Single;
        };
        let (Some(deps), true) = (self.deps(), self.plans.settings.parallel) else {
            return Topology::Single;
        };
        let session = self
            .get(slug)
            .and_then(|m| m.session)
            .and_then(|s| jobs.session(s).ok().flatten());
        match session {
            Some(s) if deps.worktrees.enabled_for(&s) => Topology::choose(plan),
            _ => Topology::Single,
        }
    }

    /// Starts the build of an approved plan in the topology it gets.
    pub(super) fn build(&mut self, slug: &str, jobs: &mut JobService) -> AppResult<JobId> {
        match self.topology(slug, jobs) {
            Topology::Fanout => self.start_graph(slug, jobs),
            Topology::Single => self.launch(slug, jobs),
        }
    }

    fn start_graph(&mut self, slug: &str, jobs: &mut JobService) -> AppResult<JobId> {
        let i = self.index(slug)?;
        if self.missions[i].state == MissionState::Draft {
            self.lint(slug)?;
        }
        let m = &self.missions[i];
        if m.state != MissionState::Linted {
            return Err(AppError::Mission(format!(
                "{slug} is {}: only a linted mission can run",
                m.state
            )));
        }
        if compile(&m.contract).is_err() {
            // The single path refuses it, with the reasons.
            return self.launch(slug, jobs);
        }
        let deps = self
            .deps()
            .ok_or_else(|| AppError::Mission("task graphs are not set up in this run".into()))?;
        let plan = self
            .plan(slug)
            .and_then(PlanRecord::approved)
            .cloned()
            .ok_or_else(|| AppError::Mission(format!("{slug} has no approved plan")))?;
        let repo = deps
            .worktrees
            .git()
            .repository(Path::new(&m.root))?
            .ok_or_else(|| AppError::Mission(format!("{} is not in a git repository", m.root)))?;
        let integration = integration_branch(slug);
        let workdir = deps.merge.open(&repo, &integration)?;
        let now = self.clock.now();
        let mut run = GraphRun::new(&plan, &integration, now);
        run.workdir = workdir.display().to_string();
        self.plan_build_started(slug);
        self.verifying.remove(slug);
        self.ledger_plan(
            slug,
            "graph",
            &format!(
                "fanout: {} tasks, up to {} at once, into {integration}",
                plan.tasks.len(),
                width(&plan)
            ),
            Some(&plan),
        )?;
        let started = self.dispatch_ready(i, &plan, &mut run, jobs, &deps);
        self.put_graph(slug, run);
        let started = started?;
        let first = *started
            .first()
            .ok_or_else(|| AppError::Mission(format!("no task of {slug}'s plan can start")))?;
        let m = &mut self.missions[i];
        let from = m.apply(Signal::Queued(first), now).map_err(refused)?;
        self.store.save(&self.missions[i])?;
        self.record(slug, from)?;
        Ok(first)
    }

    fn put_graph(&mut self, slug: &str, run: GraphRun) {
        if let Some(r) = self.plans.records.get_mut(slug) {
            r.graph = Some(run);
        }
        if let Err(e) = self.save_plan(slug) {
            self.plans.problems.push(e);
        }
    }

    /// Starts every task that may start; their jobs.
    fn dispatch_ready(
        &mut self,
        i: usize,
        plan: &Plan,
        run: &mut GraphRun,
        jobs: &mut JobService,
        deps: &Deps,
    ) -> AppResult<Vec<JobId>> {
        let ready = run.ready(plan);
        if ready.is_empty() {
            return Ok(Vec::new());
        }
        let m = &self.missions[i];
        let slug = m.slug.clone();
        let contract = m.contract.clone();
        let parent = m
            .session
            .ok_or_else(|| AppError::Mission(format!("{slug} has no session to run in")))?;
        let base = jobs.session(parent)?.ok_or(AppError::SessionNotFound)?;
        // What is left of the implement share, split over the tasks not
        // yet started.
        let waiting = run
            .tasks
            .iter()
            .filter(|t| t.state == TaskState::Waiting)
            .count()
            .max(1);
        let cap = contract.budget.usd.map(|usd| {
            let left = Reservation::new(usd).headroom(MissionPhase::Implement, &m.spend);
            (left / waiting as f64).max(0.01)
        });
        let now = self.clock.now();
        let mut out = Vec::new();
        for id in ready {
            let Some(task) = plan.tasks.iter().find(|t| t.id == id) else {
                continue;
            };
            let session = jobs.spawn_session(parent, &base.task_name(&id))?;
            let prompt = task_brief(&contract, plan, task);
            let job = jobs.dispatch_from(session.id, &prompt, cap, &run.integration)?;
            if let Some(board) = &self.board {
                board.join(job, &slug, Some(&id));
            }
            self.assign_task_gate(i, job);
            deps.claims.claim(
                job,
                Claim {
                    mission: slug.clone(),
                    task: id.clone(),
                    areas: task.files.clone(),
                    root: None,
                },
            );
            if let Some(rec) = deps.worktrees.record_for(job)? {
                deps.claims.set_root(job, &rec.path.display().to_string());
            }
            if let Some(t) = run.task_mut(&id) {
                t.state = TaskState::Running;
                t.job = Some(job);
                t.started_at = Some(now);
                t.note.clear();
            }
            self.ledger_plan(
                &slug,
                "claim",
                &format!("{id} (job {job}) claims {}", task.files.join(", ")),
                None,
            )?;
            out.push(job);
        }
        Ok(out)
    }

    /// Follows every task graph: refused writes, finished jobs, the merge
    /// queue, new tasks to start, and the mission's verification once all
    /// merged.
    pub fn pump_graphs(
        &mut self,
        jobs: &mut JobService,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
    ) -> MissionReport {
        let mut report = MissionReport::default();
        let Some(deps) = self.deps() else {
            return report;
        };
        for d in deps.claims.take_denials() {
            report.changed = true;
            if let Some(run) = self
                .plans
                .records
                .get_mut(&d.mission)
                .and_then(|r| r.graph.as_mut())
            {
                run.denials += 1;
            }
            let detail = format!("{} (job {}) {} {}", d.task, d.job, d.tool, d.path);
            if let Err(e) = self
                .save_plan(&d.mission)
                .and_then(|()| self.ledger_plan(&d.mission, "scope_denied", &detail, None))
            {
                report.errors.push(e);
            }
        }
        for i in 0..self.missions.len() {
            if !self.missions[i].state.in_flight() {
                continue;
            }
            let slug = self.missions[i].slug.clone();
            let Some(plan) = self.plan(&slug).and_then(PlanRecord::approved).cloned() else {
                continue;
            };
            let Some(mut run) = self
                .plans
                .records
                .get_mut(&slug)
                .and_then(|r| r.graph.take())
            else {
                continue;
            };
            let before = run.clone();
            let step = self.step_graph(i, &plan, &mut run, jobs, facts, gates, &deps);
            let moved = run != before;
            if moved {
                self.put_graph(&slug, run);
            } else if let Some(r) = self.plans.records.get_mut(&slug) {
                r.graph = Some(run);
            }
            match step {
                Ok(step) => {
                    report.changed |= moved || step.changed;
                    report.cancel.extend(step.cancel);
                }
                Err(e) => report.errors.push(e),
            }
        }
        report.errors.append(&mut self.plans.problems);
        report
    }

    #[allow(clippy::too_many_arguments)]
    fn step_graph(
        &mut self,
        i: usize,
        plan: &Plan,
        run: &mut GraphRun,
        jobs: &mut JobService,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
        deps: &Deps,
    ) -> AppResult<Step> {
        let mut step = Step::default();
        let now = self.clock.now();
        let fact_of = |job: Option<JobId>| job.and_then(|j| facts.iter().find(|f| f.job_id == j));
        // Where each job works, and the branch its work is on.
        for t in run.tasks.iter_mut().filter(|t| t.state.holds_claim()) {
            let Some(job) = t.job else { continue };
            let rec = match deps.worktrees.record_for(job)? {
                Some(rec) => Some(rec),
                // The watchdog carried the work on in a new job, in the
                // same worktree: the task follows it, claim and all.
                None if !t.branch.is_empty() => deps
                    .worktrees
                    .store()
                    .active()?
                    .into_iter()
                    .find(|r| r.branch == t.branch),
                None => None,
            };
            let Some(rec) = rec else { continue };
            if rec.job != job {
                if let Some(c) = deps.claims.release(job) {
                    deps.claims.claim(rec.job, c);
                }
                self.assign_task_gate(i, rec.job);
                if let Some(board) = &self.board {
                    board.join(rec.job, &self.missions[i].slug, Some(&t.id));
                }
                t.job = Some(rec.job);
            }
            deps.claims
                .set_root(rec.job, &rec.path.display().to_string());
            t.branch = rec.branch;
        }
        // Finished jobs and their gates.
        let mut started = false;
        for t in run.tasks.iter_mut() {
            if !matches!(
                t.state,
                TaskState::Running | TaskState::Conflict | TaskState::Gate
            ) {
                continue;
            }
            let Some(fact) = fact_of(t.job) else { continue };
            let job = fact.job_id;
            match fact.status {
                JobStatus::Running => started = true,
                JobStatus::Done => {
                    if t.state != TaskState::Gate {
                        run.spent_usd += fact.cost_usd;
                        t.ended_at = fact.finished_at;
                        started = true;
                    }
                    let since = fact.started_at.unwrap_or(fact.created_at);
                    match gates.get(&job).filter(|g| g.at >= since).map(|g| g.outcome) {
                        Some(GateOutcome::Escalate) => {
                            t.state = TaskState::Failed;
                            t.note = "its gate gave up".into();
                            deps.claims.release(job);
                        }
                        // The last word was a block: the verdict on the
                        // stop that ended the job is still to come.
                        Some(GateOutcome::Block) => t.state = TaskState::Gate,
                        _ => t.state = TaskState::Merging,
                    }
                }
                JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted => {
                    run.spent_usd += fact.cost_usd;
                    t.ended_at = fact.finished_at;
                    t.state = TaskState::Failed;
                    t.note = match &fact.error {
                        Some(e) => format!("its job {}: {e}", fact.status.as_str()),
                        None => format!("its job {}", fact.status.as_str()),
                    };
                    deps.claims.release(job);
                }
                _ => {}
            }
        }
        if started && self.missions[i].state == MissionState::Queued {
            step.changed |= self.take(i, Signal::Started, now)?;
        }
        // The merge queue, producers first.
        while let Some(id) = run.next_merge(plan) {
            self.merge_task(i, plan, run, &id, jobs, deps)?;
        }
        if self.missions[i].state.in_flight() && run.final_job.is_none() {
            self.dispatch_ready(i, plan, run, jobs, deps)?;
        }
        // What the tasks cost so far.
        let out: f64 = run
            .tasks
            .iter()
            .filter(|t| t.state.holds_claim() && t.state != TaskState::Merging)
            .filter_map(|t| fact_of(t.job))
            .filter(|f| !f.status.is_finished())
            .map(|f| f.cost_usd)
            .sum();
        self.missions[i].spend.implement = run.spent_usd + out;
        if let Some(why) = self.graph_exhausted(i) {
            for t in run.tasks.iter().filter(|t| t.state.holds_claim()) {
                if let Some(f) = fact_of(t.job).filter(|f| !f.status.is_finished()) {
                    step.cancel.push(f.job_id);
                }
            }
            step.changed |= self.take(i, Signal::Exhausted(why), now)?;
            self.store.save(&self.missions[i])?;
            return Ok(step);
        }
        if run.all_merged() && run.final_job.is_none() {
            step.changed |= self.integrate(i, run, deps, now)?;
        }
        if let (MissionState::Verifying, Some(job), Some(from)) =
            (self.missions[i].state, run.final_job, run.verify_from)
        {
            step.changed |= self.check_plan_drift(i);
            if let Some(outcome @ (GateOutcome::Pass | GateOutcome::Escalate)) =
                gates.get(&job).filter(|g| g.at > from).map(|g| g.outcome)
            {
                step.changed |= self.take(i, Signal::Gate(outcome), now)?;
            }
        }
        if run.stuck(plan)
            && matches!(
                self.missions[i].state,
                MissionState::Queued | MissionState::Running
            )
        {
            let why: Vec<String> = run
                .tasks
                .iter()
                .filter(|t| t.state == TaskState::Failed)
                .map(|t| format!("{}: {}", t.id, t.note))
                .collect();
            step.changed |= self.take(i, Signal::Finished { ok: false }, now)?;
            self.missions[i].note = Some(format!("tasks need you: {}", why.join("; ")));
        }
        self.store.save(&self.missions[i])?;
        Ok(step)
    }

    /// A task's job is judged by the repository's gate with the mission's
    /// protected paths; the mission's own checks wait for the integrated
    /// result.
    fn assign_task_gate(&self, i: usize, job: JobId) {
        let Some(gates) = &self.gates else { return };
        let protected = compile(&self.missions[i].contract)
            .map(|c| c.gate.protected)
            .unwrap_or_default();
        gates.assign(
            job,
            MissionGate {
                checks: Vec::new(),
                protected,
            },
        );
    }

    /// Why the graph's budget is spent, once it is.
    fn graph_exhausted(&self, i: usize) -> Option<String> {
        let m = &self.missions[i];
        if !matches!(m.state, MissionState::Queued | MissionState::Running) {
            return None;
        }
        let usd = m.contract.budget.usd?;
        let spent = m.spend.total();
        (spent > usd && m.contract.budget.on_exhaust == OnExhaust::Escalate)
            .then(|| format!("spent ${spent:.2} of ${usd:.2}"))
    }

    /// Merges task `id` into the integration branch: its work committed,
    /// its files checked against its claim, then merged; a conflict sends
    /// its agent back once.
    fn merge_task(
        &mut self,
        i: usize,
        plan: &Plan,
        run: &mut GraphRun,
        id: &str,
        jobs: &mut JobService,
        deps: &Deps,
    ) -> AppResult<()> {
        let slug = self.missions[i].slug.clone();
        let Some(task) = plan.tasks.iter().find(|t| t.id == id) else {
            return Ok(());
        };
        let integration = run.integration.clone();
        let workdir = PathBuf::from(&run.workdir);
        let Some(t) = run.task_mut(id) else {
            return Ok(());
        };
        let Some(job) = t.job else {
            t.state = TaskState::Failed;
            t.note = "it has no job".into();
            return Ok(());
        };
        let Some(rec) = deps.worktrees.record_for(job)? else {
            t.state = TaskState::Failed;
            t.note = "its worktree is gone".into();
            deps.claims.release(job);
            return Ok(());
        };
        t.branch = rec.branch.clone();
        let committed = deps
            .worktrees
            .git()
            .snapshot(&rec.path, &format!("swarmux: task {id} of {slug}"))
            .and_then(|_| deps.merge.changed(&workdir, &rec.branch));
        let changed = match committed {
            Ok(changed) => changed,
            Err(e) => {
                t.state = TaskState::Failed;
                t.note = format!("its work could not be committed: {e}");
                deps.claims.release(job);
                return self.ledger_plan(&slug, "merge_failed", &format!("{id}: {e}"), None);
            }
        };
        let outside: Vec<String> = changed
            .into_iter()
            .filter(|f| !is_incidental(f) && !in_claim(&task.files, f))
            .collect();
        if !outside.is_empty() {
            t.state = TaskState::Failed;
            t.merge_blocked = true;
            t.note = format!("changed files outside its scope: {}", outside.join(", "));
            deps.claims.release(job);
            let detail = format!("{id} (job {job}): {}", outside.join(", "));
            return self.ledger_plan(&slug, "scope_blocked", &detail, None);
        }
        let message = format!("swarmux: merge task {id} ({}) of {slug}", task.title);
        let outcome = deps.merge.merge(&workdir, &rec.branch, &message);
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                t.state = TaskState::Failed;
                t.merge_blocked = true;
                t.note = format!("the merge failed: {e}");
                deps.claims.release(job);
                return self.ledger_plan(&slug, "merge_failed", &format!("{id}: {e}"), None);
            }
        };
        match outcome {
            MergeOutcome::Merged { commit } => {
                t.state = TaskState::Merged;
                t.note.clear();
                deps.claims.release(job);
                run.merged.push(id.to_string());
                self.ledger_plan(&slug, "merged", &format!("{id} as {commit}"), None)
            }
            MergeOutcome::Conflict { files } if !t.reinvoked => {
                let brief = conflict_brief(task, &integration, &files);
                let again = jobs.continue_with(job, &brief)?;
                self.assign_task_gate(i, again);
                if let Some(board) = &self.board {
                    board.join(again, &slug, Some(&t.id));
                }
                if let Some(c) = deps.claims.release(job) {
                    deps.claims.claim(again, c);
                }
                t.job = Some(again);
                t.reinvoked = true;
                t.state = TaskState::Conflict;
                t.note = format!("conflict in {}: its agent is rebasing", files.join(", "));
                self.ledger_plan(
                    &slug,
                    "conflict",
                    &format!("{id}: {}; re-invoked as job {again}", files.join(", ")),
                    None,
                )
            }
            MergeOutcome::Conflict { files } => {
                t.state = TaskState::Failed;
                t.merge_blocked = true;
                t.note = format!(
                    "still conflicts after one rebase, in {}: resolve it in {}",
                    files.join(", "),
                    rec.path.display()
                );
                deps.claims.release(job);
                self.ledger_plan(
                    &slug,
                    "conflict",
                    &format!("{id}: {} again; to the user", files.join(", ")),
                    None,
                )
            }
        }
    }

    /// Every task merged: the last task's branch is brought up to the
    /// integration branch and the mission's own checks run there.
    fn integrate(
        &mut self,
        i: usize,
        run: &mut GraphRun,
        deps: &Deps,
        now: Millis,
    ) -> AppResult<bool> {
        let Some(job) = run
            .merged
            .last()
            .and_then(|id| run.task(id))
            .and_then(|t| t.job)
        else {
            return Ok(false);
        };
        let slug = self.missions[i].slug.clone();
        let rec = deps.worktrees.record_for(job)?.ok_or_else(|| {
            AppError::Mission(format!("{slug}: the last task's worktree is gone"))
        })?;
        deps.merge.fast_forward(&rec.path, &run.integration)?;
        if let (Some(gates), Ok(compiled)) = (&self.gates, compile(&self.missions[i].contract)) {
            gates.assign(job, compiled.gate);
        }
        run.final_job = Some(job);
        run.verify_from = Some(now);
        self.missions[i].job = Some(job);
        self.ledger_plan(
            &slug,
            "integrated",
            &format!(
                "{} tasks merged in order {}; verifying as job {job}",
                run.merged.len(),
                run.merged.join(", ")
            ),
            None,
        )?;
        let mut changed = false;
        if self.missions[i].state == MissionState::Queued {
            changed |= self.take(i, Signal::Started, now)?;
        }
        changed |= self.take(i, Signal::Finished { ok: true }, now)?;
        if let Some(gates) = &self.gates
            && let Err(why) = gates.verify(job, &rec.path)
        {
            changed |= self.take(i, Signal::Gate(GateOutcome::Escalate), now)?;
            self.missions[i].note = Some(why);
        }
        Ok(changed)
    }

    /// Tasks that can't merge without the user.
    pub(super) fn graph_items(&self) -> Vec<AttentionItem> {
        let mut out = Vec::new();
        for m in &self.missions {
            let Some(run) = self.graph(&m.slug) else {
                continue;
            };
            let title = if m.contract.title.is_empty() {
                m.slug.as_str()
            } else {
                m.contract.title.as_str()
            };
            for t in run.tasks.iter().filter(|t| t.merge_blocked) {
                if t.state != TaskState::Failed {
                    continue;
                }
                out.push(AttentionItem::merge_blocked(
                    &m.slug,
                    &t.id,
                    (m.session, t.job),
                    (title, &format!("task {}: {}", t.id, t.note)),
                    t.ended_at.unwrap_or(m.updated_at),
                ));
            }
        }
        out
    }
}

/// What one graph step asks of the caller.
#[derive(Default)]
struct Step {
    changed: bool,
    cancel: Vec<JobId>,
}
