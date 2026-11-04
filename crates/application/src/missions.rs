//! Mission use cases: write a mission, lint it, compile it into a job and
//! its gate, and follow it to delivery, escalation or refusal.
//!
//! The lint is asked of a cheap model through [`MissionLinter`] and never
//! blocks: the local lint answers at once, the model's questions arrive
//! later through [`MissionService::pump`]. A mission is only delivered by
//! a passing gate verdict on its job, taken after the job ended.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::budget::Reservation;
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::mission::{
    BadTransition, Clarification, Contract, Finding, LintReport, Mission, MissionPhase,
    MissionState, Question, Severity, Signal, apply_fixes, compile, lint_report,
};
use swarmux_domain::{JobId, JobStatus, Millis, SessionId, slug};

use crate::evidence::EvidenceService;
use crate::gate::{GateState, JobGates};
use crate::jobs::{JobFact, JobService};
use crate::ledger::LedgerService;
use crate::ports::Clock;
use crate::{AppError, AppResult};

mod graph;
mod plan;
pub use graph::{Claim, MergeOutcome, MergeQueue, ScopeClaims, ScopeDenial};
pub use plan::{
    Begun, ChangedFiles, PlanReply, PlanRequest, PlanSettings, PlanSink, PlanStore, Planner,
    WorkDirs,
};

/// Where missions are kept.
pub trait MissionStore {
    /// Inserts the mission, or replaces the one with its slug.
    fn save(&self, mission: &Mission) -> AppResult<()>;
    /// Every mission, oldest first.
    fn list(&self) -> AppResult<Vec<Mission>>;
}

/// What the linter is asked about.
#[derive(Clone, Debug, PartialEq)]
pub struct LintRequest {
    pub contract: Contract,
    /// What the local lint already found, so the questions go further.
    pub findings: Vec<Finding>,
}

/// The linter's questions (or why there are none) and what asking cost.
#[derive(Clone, Debug, PartialEq)]
pub struct LintReply {
    pub result: Result<Vec<Question>, String>,
    pub cost_usd: f64,
    /// Who answered: `haiku`, …
    pub source: String,
}

/// Receives a linter's reply, from any thread.
pub type LintSink = Box<dyn FnOnce(LintReply) + Send + 'static>;

/// Asks clarifying questions about a contract. Never blocks; the reply
/// arrives through the sink.
pub trait MissionLinter {
    fn lint(&self, request: LintRequest, sink: LintSink);
}

/// What one [`MissionService::pump`] did.
#[derive(Debug, Default)]
pub struct MissionReport {
    /// A mission changed.
    pub changed: bool,
    pub errors: Vec<AppError>,
    /// Jobs to stop: their mission ran out of budget.
    pub cancel: Vec<JobId>,
}

/// Writes, lints, launches and follows missions.
pub struct MissionService {
    store: Rc<dyn MissionStore>,
    clock: Rc<dyn Clock>,
    ledger: Option<Rc<LedgerService>>,
    linter: Option<Rc<dyn MissionLinter>>,
    gates: Option<Rc<dyn JobGates>>,
    evidence: Option<Rc<EvidenceService>>,
    /// Bundles of missions delivered earlier were looked for.
    backfilled: bool,
    /// Oldest first.
    missions: Vec<Mission>,
    tx: Sender<(String, u64, LintReply)>,
    rx: Receiver<(String, u64, LintReply)>,
    /// The newest lint asked per mission; older replies are dropped.
    linting: HashMap<String, u64>,
    tickets: u64,
    /// Missions whose checks were run by hand already.
    verifying: HashSet<String>,
    /// Plan-first: planner, plans and what each build was compared with.
    plans: plan::PlanState,
    /// Task graphs: merge queue, worktrees and scope claims.
    graphs: graph::GraphState,
}

impl MissionService {
    pub fn new(store: Rc<dyn MissionStore>, clock: Rc<dyn Clock>) -> AppResult<MissionService> {
        let (tx, rx) = channel();
        let missions = store.list()?;
        Ok(MissionService {
            store,
            clock,
            ledger: None,
            linter: None,
            gates: None,
            evidence: None,
            backfilled: false,
            missions,
            tx,
            rx,
            linting: HashMap::new(),
            tickets: 0,
            verifying: HashSet::new(),
            plans: plan::PlanState::new(),
            graphs: graph::GraphState::default(),
        })
    }

    /// Asks `linter` for clarifying questions after every lint.
    pub fn with_linter(mut self, linter: Rc<dyn MissionLinter>) -> MissionService {
        self.linter = Some(linter);
        self
    }

    /// Gives each mission's job its checks and protected paths.
    pub fn with_gates(mut self, gates: Rc<dyn JobGates>) -> MissionService {
        self.gates = Some(gates);
        self
    }

    /// Seals an evidence bundle for every mission that is delivered. A
    /// mission whose bundle can't be sealed is escalated instead.
    pub fn with_evidence(mut self, evidence: Rc<EvidenceService>) -> MissionService {
        self.set_evidence(evidence);
        self
    }

    /// Like [`MissionService::with_evidence`], for a service already built.
    pub fn set_evidence(&mut self, evidence: Rc<EvidenceService>) {
        self.evidence = Some(evidence);
    }

    /// Records every change of state in the ledger.
    pub fn with_ledger(mut self, ledger: Rc<LedgerService>) -> MissionService {
        self.ledger = Some(ledger);
        self
    }

    /// Every mission, oldest first: cards keep their places.
    pub fn missions(&self) -> &[Mission] {
        &self.missions
    }

    pub fn get(&self, slug: &str) -> Option<&Mission> {
        self.missions.iter().find(|m| m.slug == slug)
    }

    /// A lint is out for the mission.
    pub fn linting(&self, slug: &str) -> bool {
        self.linting.contains_key(slug)
    }

    fn index(&self, slug: &str) -> AppResult<usize> {
        self.missions
            .iter()
            .position(|m| m.slug == slug)
            .ok_or_else(|| AppError::MissionNotFound(slug.to_string()))
    }

    /// A slug no other mission has.
    fn fresh_slug(&self, contract: &Contract) -> String {
        let name = if contract.title.trim().is_empty() {
            &contract.goal
        } else {
            &contract.title
        };
        let base = slug(
            name.split_whitespace()
                .take(6)
                .collect::<Vec<_>>()
                .join(" ")
                .as_str(),
        );
        let taken = |s: &str| self.missions.iter().any(|m| m.slug == s);
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|s| !taken(s))
            .unwrap_or(base)
    }

    /// Writes a new mission for `session`, working in `root`, and lints it.
    pub fn create(
        &mut self,
        contract: Contract,
        root: &str,
        session: Option<SessionId>,
    ) -> AppResult<String> {
        if contract.goal.trim().is_empty() {
            return Err(AppError::Mission("a mission needs a goal".into()));
        }
        let slug = self.fresh_slug(&contract);
        let mut mission = Mission::new(&slug, root, contract, self.clock.now());
        mission.session = session;
        self.store.save(&mission)?;
        self.missions.push(mission);
        self.record(&slug, MissionState::Draft)?;
        self.lint(&slug)?;
        Ok(slug)
    }

    /// Replaces the contract of a mission that is not out, and lints it again.
    pub fn edit(&mut self, slug: &str, contract: Contract) -> AppResult<()> {
        let i = self.index(slug)?;
        let now = self.clock.now();
        let m = &mut self.missions[i];
        let from = m.apply(Signal::Edited, now).map_err(refused)?;
        m.contract = contract;
        self.store.save(&self.missions[i])?;
        self.record(slug, from)?;
        self.forget_plan(slug)?;
        self.lint(slug)
    }

    /// Lints the mission now and asks the linter for questions. The local
    /// findings apply at once; the questions come with a later pump.
    pub fn lint(&mut self, slug: &str) -> AppResult<()> {
        let i = self.index(slug)?;
        let now = self.clock.now();
        // Questions asked before stay (with their answers) until new ones
        // replace them.
        let m = &self.missions[i];
        let mut asked: Vec<Question> = m
            .lint
            .as_ref()
            .map(|l| l.questions.clone())
            .unwrap_or_default();
        asked.extend(m.contract.clarifications.iter().map(|c| Question {
            question: c.question.clone(),
            why: String::new(),
        }));
        let report = lint_report(&self.missions[i].contract).with_questions(asked);
        let m = &mut self.missions[i];
        let from = m
            .apply(
                Signal::Linted {
                    blocking: report.blocking(),
                },
                now,
            )
            .map_err(refused)?;
        set_questions(m, report);
        self.store.save(&self.missions[i])?;
        if from != MissionState::Linted {
            self.record(slug, from)?;
        }
        if let Some(linter) = &self.linter {
            self.tickets += 1;
            let ticket = self.tickets;
            self.linting.insert(slug.to_string(), ticket);
            let m = &self.missions[i];
            let request = LintRequest {
                contract: m.contract.clone(),
                findings: m
                    .lint
                    .as_ref()
                    .map(|l| l.findings.clone())
                    .unwrap_or_default(),
            };
            let tx = self.tx.clone();
            let key = slug.to_string();
            linter.lint(
                request,
                Box::new(move |reply| {
                    // Nobody listening (the app is closing): nothing to keep.
                    let _ = tx.send((key, ticket, reply));
                }),
            );
        }
        Ok(())
    }

    /// The user's answer to the mission's `n`th question.
    pub fn answer(&mut self, slug: &str, n: usize, answer: &str) -> AppResult<()> {
        let i = self.index(slug)?;
        let m = &mut self.missions[i];
        if m.state.in_flight() {
            return Err(AppError::Mission(format!(
                "{slug} is {}: its answers are already with the agent",
                m.state
            )));
        }
        let Some(q) = m.contract.clarifications.get_mut(n) else {
            return Err(AppError::Mission(format!(
                "{slug} has no question {}",
                n + 1
            )));
        };
        q.answer = answer.trim().to_string();
        m.updated_at = self.clock.now();
        self.store.save(&self.missions[i])
    }

    /// Protects the oracles the lint found unprotected. Returns how many
    /// globs it added.
    pub fn fix(&mut self, slug: &str) -> AppResult<usize> {
        let i = self.index(slug)?;
        let m = &self.missions[i];
        let findings = m
            .lint
            .as_ref()
            .map(|l| l.findings.clone())
            .unwrap_or_default();
        let mut contract = m.contract.clone();
        let n = apply_fixes(&mut contract, &findings);
        if n > 0 {
            self.edit(slug, contract)?;
        }
        Ok(n)
    }

    /// Compiles the mission into a job of its session and gives the job
    /// the mission's gate. A mission that can't be checked or contradicts
    /// itself is refused instead.
    pub fn launch(&mut self, slug: &str, jobs: &mut JobService) -> AppResult<JobId> {
        let i = self.index(slug)?;
        if self.missions[i].state == MissionState::Draft {
            self.lint(slug)?;
        }
        let now = self.clock.now();
        let m = &self.missions[i];
        if m.state != MissionState::Linted {
            return Err(AppError::Mission(format!(
                "{slug} is {}: only a linted mission can run",
                m.state
            )));
        }
        if let Some(why) = self.plan_blocks(slug) {
            return Err(AppError::Mission(why));
        }
        let m = &self.missions[i];
        let compiled = match compile(&m.contract) {
            Ok(c) => c,
            Err(blocking) => {
                let why = blocking
                    .iter()
                    .filter(|f| f.severity == Severity::Blocking)
                    .map(|f| f.detail.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                let m = &mut self.missions[i];
                let from = m.apply(Signal::Refuse(why.clone()), now).map_err(refused)?;
                self.store.save(&self.missions[i])?;
                self.record(slug, from)?;
                return Err(AppError::Mission(format!("{slug} refused: {why}")));
            }
        };
        let session = m
            .session
            .ok_or_else(|| AppError::Mission(format!("{slug} has no session to run in")))?;
        // The job may spend planning's leftovers and its own share; the
        // verify and review shares stay reserved.
        let cap = m
            .contract
            .budget
            .usd
            .map(|usd| Reservation::new(usd).headroom(MissionPhase::Implement, &m.spend))
            // Nothing left still caps the job rather than freeing it.
            .map(|left| left.max(0.01));
        let mut prompt = compiled.prompt;
        if let Some(plan) = self.plan_prompt_part(slug) {
            prompt.push_str("\n\n");
            prompt.push_str(plan.trim_end());
        }
        let job = jobs.dispatch_capped(session, &prompt, cap)?;
        if let Some(gates) = &self.gates {
            gates.assign(job, compiled.gate);
        }
        let m = &mut self.missions[i];
        let from = m.apply(Signal::Queued(job), now).map_err(refused)?;
        self.verifying.remove(slug);
        self.plan_build_started(slug);
        self.store.save(&self.missions[i])?;
        self.record(slug, from)?;
        Ok(job)
    }

    /// Takes the linter's replies, follows each mission's job and gate,
    /// and keeps budgets.
    pub fn pump(&mut self, facts: &[JobFact], gates: &HashMap<JobId, GateState>) -> MissionReport {
        let mut report = MissionReport::default();
        if let (false, Some(evidence)) = (self.backfilled, &self.evidence) {
            self.backfilled = true;
            for (slug, e) in evidence.backfill(&self.missions) {
                report
                    .errors
                    .push(AppError::Mission(format!("no bundle for {slug}: {e}")));
            }
        }
        while let Ok((slug, ticket, reply)) = self.rx.try_recv() {
            if self.linting.get(&slug) != Some(&ticket) {
                continue;
            }
            self.linting.remove(&slug);
            report.changed = true;
            if let Err(e) = self.take_lint(&slug, reply) {
                report.errors.push(e);
            }
        }
        report.changed |= self.pump_plans(&mut report.errors);
        for i in 0..self.missions.len() {
            if !self.missions[i].state.in_flight() || self.graph(&self.missions[i].slug).is_some() {
                continue;
            }
            match self.follow(i, facts, gates, &mut report.cancel) {
                Ok(changed) => report.changed |= changed,
                Err(e) => report.errors.push(e),
            }
        }
        report.errors.append(&mut self.plans.problems);
        report
    }

    fn take_lint(&mut self, slug: &str, reply: LintReply) -> AppResult<()> {
        let i = self.index(slug)?;
        let m = &mut self.missions[i];
        m.spend.plan += reply.cost_usd;
        let Some(mut report) = m.lint.clone() else {
            return Ok(());
        };
        report.cost_usd += reply.cost_usd;
        match reply.result {
            Ok(questions) => {
                report = report.with_questions(questions);
                report.source = reply.source;
                report.problem = None;
            }
            Err(why) => report.problem = Some(why),
        }
        set_questions(m, report);
        self.store.save(&self.missions[i])
    }

    /// Moves one mission along with its job; whether it changed.
    fn follow(
        &mut self,
        i: usize,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
        cancel: &mut Vec<JobId>,
    ) -> AppResult<bool> {
        let Some(job) = self.missions[i].job else {
            return Ok(false);
        };
        let Some(fact) = facts.iter().find(|f| f.job_id == job) else {
            return Ok(false);
        };
        let now = self.clock.now();
        self.missions[i].spend.implement = fact.cost_usd;
        let mut signals = Vec::new();
        let state = self.missions[i].state;
        match fact.status {
            JobStatus::Running if state == MissionState::Queued => signals.push(Signal::Started),
            JobStatus::Done if state != MissionState::Verifying => {
                if state == MissionState::Queued {
                    signals.push(Signal::Started);
                }
                signals.push(Signal::Finished { ok: true });
            }
            JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted => {
                signals.push(Signal::Finished { ok: false });
            }
            _ => {}
        }
        if let Some(why) = self.exhausted(i, fact, now) {
            signals.push(Signal::Exhausted(why));
            cancel.push(job);
        }
        let mut changed = false;
        for signal in signals {
            changed |= self.take(i, signal, now)?;
        }
        if self.missions[i].state == MissionState::Verifying {
            changed |= self.check_plan_drift(i);
            changed |= self.verify(i, fact, gates.get(&job), now)?;
        }
        if changed {
            self.store.save(&self.missions[i])?;
        }
        Ok(changed)
    }

    /// While verifying: the gate's verdict on the job, taken since it
    /// started, decides. Without one the checks are run once by hand.
    fn verify(
        &mut self,
        i: usize,
        fact: &JobFact,
        gate: Option<&GateState>,
        now: Millis,
    ) -> AppResult<bool> {
        let since = fact.started_at.unwrap_or(fact.created_at);
        let verdict = gate.filter(|g| g.at >= since).map(|g| g.outcome);
        match verdict {
            Some(outcome @ (GateOutcome::Pass | GateOutcome::Escalate)) => {
                self.take(i, Signal::Gate(outcome), now)
            }
            _ => {
                let m = &self.missions[i];
                let (Some(gates), Some(job)) = (&self.gates, m.job) else {
                    return Ok(false);
                };
                if !self.verifying.insert(m.slug.clone()) {
                    return Ok(false);
                }
                match gates.verify(job, Path::new(&m.root)) {
                    Ok(()) => Ok(false),
                    Err(why) => {
                        let changed = self.take(i, Signal::Gate(GateOutcome::Escalate), now)?;
                        self.missions[i].note = Some(why);
                        Ok(changed)
                    }
                }
            }
        }
    }

    /// Why the mission's budget is spent, once it is.
    fn exhausted(&self, i: usize, fact: &JobFact, now: Millis) -> Option<String> {
        let m = &self.missions[i];
        let out = matches!(m.state, MissionState::Queued | MissionState::Running);
        if !out || fact.status.is_finished() {
            return None;
        }
        let budget = &m.contract.budget;
        let spent = m.spend.total();
        let over_usd = budget
            .usd
            .filter(|usd| spent > *usd)
            .map(|usd| format!("spent ${spent:.2} of ${usd:.2}"));
        let over_wall = budget.wall_minutes.and_then(|mins| {
            let ran = now - fact.started_at?;
            (ran > Millis::from(mins) * 60_000)
                .then(|| format!("ran {} min of {mins}", ran / 60_000))
        });
        let why = over_usd.or(over_wall)?;
        match budget.on_exhaust {
            swarmux_domain::mission::OnExhaust::Escalate => Some(why),
            swarmux_domain::mission::OnExhaust::Warn => None,
        }
    }

    /// Applies `signal`; whether the state changed.
    fn take(&mut self, i: usize, signal: Signal, now: Millis) -> AppResult<bool> {
        let from = self.missions[i].apply(signal, now).map_err(refused)?;
        if from == self.missions[i].state {
            return Ok(false);
        }
        let slug = self.missions[i].slug.clone();
        self.record(&slug, from)?;
        if self.missions[i].state == MissionState::Delivered {
            self.seal(i, now)?;
        }
        Ok(true)
    }

    /// Delivered means a bundle exists: without one the mission goes to
    /// the user instead.
    fn seal(&mut self, i: usize, now: Millis) -> AppResult<()> {
        let Some(evidence) = self.evidence.clone() else {
            return Ok(());
        };
        let Err(why) = evidence.seal(&self.missions[i]) else {
            return Ok(());
        };
        let m = &mut self.missions[i];
        m.state = MissionState::Escalated;
        m.note = Some(format!("no evidence bundle: {why}"));
        m.updated_at = now;
        let slug = m.slug.clone();
        self.record(&slug, MissionState::Delivered)
    }

    fn record(&self, slug: &str, from: MissionState) -> AppResult<()> {
        let (Some(ledger), Some(m)) = (&self.ledger, self.get(slug)) else {
            return Ok(());
        };
        ledger.record_mission(m, from, self.clock.now())
    }
}

/// Keeps the report and turns its questions into the contract's
/// clarifications, keeping answers already given.
fn set_questions(m: &mut Mission, report: LintReport) {
    let old = std::mem::take(&mut m.contract.clarifications);
    m.contract.clarifications = report
        .questions
        .iter()
        .map(|q| Clarification {
            question: q.question.clone(),
            answer: old
                .iter()
                .find(|c| c.question == q.question)
                .map(|c| c.answer.clone())
                .unwrap_or_default(),
        })
        .collect();
    m.lint = Some(report);
}

fn refused(e: BadTransition) -> AppError {
    AppError::Mission(e.to_string())
}

#[cfg(test)]
mod tests;
