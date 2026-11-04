//! Plan-first missions: before the build, a planning run proposes a plan
//! and task graph; the user approves it, sends it back with a note or
//! cancels it; the build then gets the approved plan in its prompt, and
//! when it finishes the change is compared with the plan.
//!
//! The ports live here, in the feature: who plans ([`Planner`]), where
//! plans are kept ([`PlanStore`]) and how to see what a job changed
//! ([`ChangedFiles`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::mission::{MissionState, compile};
use swarmux_domain::plan::{
    DEFAULT_DRIFT_THRESHOLD, Plan, PlanRecord, PlanStatus, check_drift, plan_brief, plan_prompt,
};
use swarmux_domain::{AttentionItem, JobId, Millis};

use super::MissionService;
use crate::jobs::JobService;
use crate::{AppError, AppResult};

/// A change older than this no longer raises a drift item.
const DRIFT_ITEM_MS: Millis = 24 * 60 * 60 * 1000;

/// The `[missions]` section: plan-first by default or not, which model
/// plans, and how much drift is too much.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanSettings {
    /// Missions plan first unless they say otherwise.
    pub plan_first: bool,
    /// The model that plans.
    pub model: String,
    /// Drift at or above this share (0 to 1) of the change is flagged.
    pub drift_threshold: f64,
}

impl Default for PlanSettings {
    fn default() -> PlanSettings {
        PlanSettings {
            plan_first: false,
            model: "sonnet".into(),
            drift_threshold: DEFAULT_DRIFT_THRESHOLD,
        }
    }
}

/// What the planner is asked.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRequest {
    /// The folder to explore, read-only.
    pub root: PathBuf,
    pub prompt: String,
    pub model: String,
}

/// The planner's answer and what asking cost.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanReply {
    pub result: Result<Plan, String>,
    pub cost_usd: f64,
}

/// Receives a planner's reply, from any thread.
pub type PlanSink = Box<dyn FnOnce(PlanReply) + Send + 'static>;

/// Proposes a plan and task graph for a mission, in plan mode. Never
/// blocks; the reply arrives through the sink.
pub trait Planner {
    fn plan(&self, request: PlanRequest, sink: PlanSink);
}

/// Where missions' plans are kept.
pub trait PlanStore {
    /// Inserts the mission's record or replaces it.
    fn save(&self, slug: &str, record: &PlanRecord, at: Millis) -> AppResult<()>;
    fn list(&self) -> AppResult<Vec<(String, PlanRecord)>>;
}

/// The files a folder's work changed, relative to the repository.
pub trait ChangedFiles {
    fn changed(&self, dir: &Path) -> AppResult<Vec<String>>;
}

/// Where a job's work is: its worktree, else the mission's folder.
pub type WorkDirs = Rc<dyn Fn(JobId, &Path) -> PathBuf>;

/// What starting a mission did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begun {
    /// The build is out as this job.
    Job(JobId),
    /// A planning run is out; the build waits for the plan's approval.
    Planning,
}

type PlanMsg = (String, u64, PlanReply);

/// Everything plan-first needs, held by the mission service.
pub(super) struct PlanState {
    settings: PlanSettings,
    planner: Option<Rc<dyn Planner>>,
    store: Option<Rc<dyn PlanStore>>,
    diffs: Option<Rc<dyn ChangedFiles>>,
    workdirs: Option<WorkDirs>,
    records: HashMap<String, PlanRecord>,
    tx: Sender<PlanMsg>,
    rx: Receiver<PlanMsg>,
    /// The newest planning run per mission; older replies are dropped.
    out: HashMap<String, u64>,
    tickets: u64,
    /// Missions whose build was compared with its plan already.
    checked: HashSet<String>,
    /// Why a build could not be compared, to be reported once.
    pub(super) problems: Vec<AppError>,
}

impl PlanState {
    pub(super) fn new() -> PlanState {
        let (tx, rx) = channel();
        PlanState {
            settings: PlanSettings::default(),
            planner: None,
            store: None,
            diffs: None,
            workdirs: None,
            records: HashMap::new(),
            tx,
            rx,
            out: HashMap::new(),
            tickets: 0,
            checked: HashSet::new(),
            problems: Vec::new(),
        }
    }
}

impl MissionService {
    /// Lets missions plan first: `planner` proposes, `store` keeps the
    /// plans (those kept before are loaded now).
    pub fn with_planner(
        mut self,
        planner: Rc<dyn Planner>,
        store: Rc<dyn PlanStore>,
    ) -> AppResult<MissionService> {
        for (slug, record) in store.list()? {
            self.plans.records.insert(slug, record);
        }
        self.plans.planner = Some(planner);
        self.plans.store = Some(store);
        Ok(self)
    }

    pub fn with_plan_settings(mut self, settings: PlanSettings) -> MissionService {
        self.plans.settings = settings;
        self
    }

    /// Lets a finished build be compared with its plan.
    pub fn with_changes(
        mut self,
        diffs: Rc<dyn ChangedFiles>,
        workdirs: WorkDirs,
    ) -> MissionService {
        self.plans.diffs = Some(diffs);
        self.plans.workdirs = Some(workdirs);
        self
    }

    /// The mission's plan, once one was asked for or the setting made.
    pub fn plan(&self, slug: &str) -> Option<&PlanRecord> {
        self.plans.records.get(slug)
    }

    /// The mission plans before it builds: its own setting, else the default.
    pub fn plan_first(&self, slug: &str) -> bool {
        self.plan(slug)
            .and_then(|r| r.plan_first)
            .unwrap_or(self.plans.settings.plan_first)
    }

    /// A planning run is out for the mission.
    pub fn planning(&self, slug: &str) -> bool {
        self.plans.out.contains_key(slug)
    }

    /// The drift threshold in force.
    pub fn drift_threshold(&self) -> f64 {
        self.plans.settings.drift_threshold
    }

    fn save_plan(&self, slug: &str) -> AppResult<()> {
        let (Some(store), Some(record)) = (&self.plans.store, self.plans.records.get(slug)) else {
            return Ok(());
        };
        store.save(slug, record, self.clock.now())
    }

    fn ledger_plan(
        &self,
        slug: &str,
        event: &str,
        detail: &str,
        plan: Option<&Plan>,
    ) -> AppResult<()> {
        let (Some(ledger), Some(m)) = (&self.ledger, self.get(slug)) else {
            return Ok(());
        };
        let digest = plan.map(Plan::digest);
        let tasks = plan.map_or(0, |p| p.tasks.len());
        ledger.record_plan(m, event, detail, digest.as_deref(), tasks, self.clock.now())
    }

    /// Sets whether the mission plans first. A mission that is out keeps
    /// the setting it started with.
    pub fn set_plan_first(&mut self, slug: &str, on: bool) -> AppResult<()> {
        let i = self.index(slug)?;
        if self.missions[i].state.in_flight() {
            return Err(AppError::Mission(format!(
                "{slug} is {}: it can't change now",
                self.missions[i].state
            )));
        }
        self.plans
            .records
            .entry(slug.to_string())
            .or_default()
            .plan_first = Some(on);
        self.save_plan(slug)
    }

    /// Starts the mission: plans it first when it is plan-first and has no
    /// approved plan, else launches the build.
    pub fn begin(&mut self, slug: &str, jobs: &mut JobService) -> AppResult<Begun> {
        self.index(slug)?;
        if !self.plan_first(slug)
            || self
                .plans
                .records
                .get(slug)
                .is_some_and(|r| r.approved().is_some())
        {
            return self.launch(slug, jobs).map(Begun::Job);
        }
        match self.plan(slug).map(|r| r.status) {
            Some(PlanStatus::Planning) => {
                Err(AppError::Mission(format!("{slug} is still being planned")))
            }
            Some(PlanStatus::Proposed) => Err(AppError::Mission(format!(
                "{slug} has a plan waiting: approve, change or cancel it"
            ))),
            _ => self.request_plan(slug, "").map(|()| Begun::Planning),
        }
    }

    /// Asks the planner for a plan. `note` is why the last one was sent
    /// back; the plan it answers is kept to be revised.
    pub fn request_plan(&mut self, slug: &str, note: &str) -> AppResult<()> {
        let i = self.index(slug)?;
        let Some(planner) = self.plans.planner.clone() else {
            return Err(AppError::Mission(
                "planning is not set up in this run".into(),
            ));
        };
        if self.missions[i].state == MissionState::Draft {
            self.lint(slug)?;
        }
        let m = &self.missions[i];
        if m.state != MissionState::Linted {
            return Err(AppError::Mission(format!(
                "{slug} is {}: only a linted mission can be planned",
                m.state
            )));
        }
        if let Err(blocking) = compile(&m.contract) {
            let why = blocking
                .iter()
                .map(|f| f.detail.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(AppError::Mission(format!(
                "{slug} can't run as written: {why}"
            )));
        }
        let previous = self.plan(slug).and_then(|r| r.plan.clone());
        let request = PlanRequest {
            root: PathBuf::from(&m.root),
            prompt: plan_prompt(&m.contract, previous.as_ref(), note),
            model: self.plans.settings.model.clone(),
        };
        let record = self.plans.records.entry(slug.to_string()).or_default();
        record.status = PlanStatus::Planning;
        record.rounds += 1;
        record.note = note.trim().to_string();
        self.save_plan(slug)?;
        if !note.trim().is_empty() {
            self.ledger_plan(slug, "rejected", note.trim(), previous.as_ref())?;
        }
        self.plans.tickets += 1;
        let ticket = self.plans.tickets;
        self.plans.out.insert(slug.to_string(), ticket);
        let tx = self.plans.tx.clone();
        let key = slug.to_string();
        planner.plan(
            request,
            Box::new(move |reply| {
                // Nobody listening (the app is closing): nothing to keep.
                let _ = tx.send((key, ticket, reply));
            }),
        );
        Ok(())
    }

    /// Approves the proposed plan, keeps it on the ledger and launches the
    /// build with the plan in its prompt.
    pub fn approve_plan(&mut self, slug: &str, jobs: &mut JobService) -> AppResult<JobId> {
        self.index(slug)?;
        let record = self.plans.records.get_mut(slug);
        let Some(record) = record.filter(|r| r.status == PlanStatus::Proposed && r.plan.is_some())
        else {
            return Err(AppError::Mission(format!("{slug} has no plan to approve")));
        };
        record.status = PlanStatus::Approved;
        record.note.clear();
        record.drift = None;
        let plan = record.plan.clone();
        self.save_plan(slug)?;
        self.ledger_plan(slug, "approved", "", plan.as_ref())?;
        self.launch(slug, jobs)
    }

    /// Sends the proposed plan back with `note` and asks for a new one.
    pub fn reject_plan(&mut self, slug: &str, note: &str) -> AppResult<()> {
        let proposed = self
            .plan(slug)
            .is_some_and(|r| r.status == PlanStatus::Proposed);
        if !proposed {
            return Err(AppError::Mission(format!("{slug} has no plan to change")));
        }
        self.request_plan(slug, note)
    }

    /// Drops the plan and any planning run; the mission stays as it was.
    pub fn cancel_plan(&mut self, slug: &str) -> AppResult<()> {
        self.index(slug)?;
        let Some(record) = self.plans.records.get_mut(slug) else {
            return Ok(());
        };
        if record.status == PlanStatus::Idle {
            return Ok(());
        }
        let was = record.plan.take();
        record.status = PlanStatus::Idle;
        record.note.clear();
        record.drift = None;
        self.plans.out.remove(slug);
        self.save_plan(slug)?;
        self.ledger_plan(slug, "cancelled", "", was.as_ref())
    }

    /// A changed contract is a different task: its plan no longer stands.
    pub(super) fn forget_plan(&mut self, slug: &str) -> AppResult<()> {
        self.plans.checked.remove(slug);
        if self
            .plans
            .records
            .get(slug)
            .is_some_and(|r| r.status != PlanStatus::Idle)
        {
            self.cancel_plan(slug)?;
        }
        Ok(())
    }

    /// Why the mission can't launch yet, when it is plan-first.
    pub(super) fn plan_blocks(&self, slug: &str) -> Option<String> {
        (self.plan_first(slug)
            && self
                .plans
                .records
                .get(slug)
                .is_none_or(|r| r.approved().is_none()))
        .then(|| format!("{slug} plans first: its plan must be approved before it builds"))
    }

    /// The approved plan, as the part of the build prompt that carries it.
    pub(super) fn plan_prompt_part(&self, slug: &str) -> Option<String> {
        self.plans
            .records
            .get(slug)
            .and_then(PlanRecord::approved)
            .map(plan_brief)
    }

    /// A new build starts: it will be compared with the plan when done.
    pub(super) fn plan_build_started(&mut self, slug: &str) {
        self.plans.checked.remove(slug);
        if let Some(r) = self.plans.records.get_mut(slug) {
            r.drift = None;
        }
    }

    /// Takes the planner's replies; whether a plan changed.
    pub(super) fn pump_plans(&mut self, errors: &mut Vec<AppError>) -> bool {
        let mut changed = false;
        while let Ok((slug, ticket, reply)) = self.plans.rx.try_recv() {
            if self.plans.out.get(&slug) != Some(&ticket) {
                continue;
            }
            self.plans.out.remove(&slug);
            changed = true;
            if let Err(e) = self.take_plan(&slug, reply) {
                errors.push(e);
            }
        }
        changed
    }

    fn take_plan(&mut self, slug: &str, reply: PlanReply) -> AppResult<()> {
        let i = self.index(slug)?;
        self.missions[i].spend.plan += reply.cost_usd;
        self.store.save(&self.missions[i])?;
        let record = self.plans.records.entry(slug.to_string()).or_default();
        record.cost_usd += reply.cost_usd;
        let (event, detail, plan) = match reply.result {
            Ok(plan) => {
                record.status = PlanStatus::Proposed;
                record.plan = Some(plan.clone());
                record.note.clear();
                ("proposed", String::new(), Some(plan))
            }
            Err(why) => {
                record.status = PlanStatus::Failed;
                record.note = why.clone();
                ("failed", why, None)
            }
        };
        self.save_plan(slug)?;
        self.ledger_plan(slug, event, &detail, plan.as_ref())
    }

    /// Compares the finished build with the approved plan, once. Whether
    /// a report was made.
    pub(super) fn check_plan_drift(&mut self, i: usize) -> bool {
        let m = &self.missions[i];
        let slug = m.slug.clone();
        let Some(plan) = self
            .plans
            .records
            .get(&slug)
            .and_then(PlanRecord::approved)
            .cloned()
        else {
            return false;
        };
        if self.plans.checked.contains(&slug) {
            return false;
        }
        let (Some(diffs), Some(job)) = (self.plans.diffs.clone(), m.job) else {
            return false;
        };
        self.plans.checked.insert(slug.clone());
        let root = PathBuf::from(&m.root);
        let dir = match &self.plans.workdirs {
            Some(dirs) => dirs(job, &root),
            None => root,
        };
        let files = match diffs.changed(&dir) {
            Ok(files) => files,
            Err(e) => {
                self.plans.problems.push(AppError::Mission(format!(
                    "{slug}: could not compare with the plan: {e}"
                )));
                return false;
            }
        };
        let report = check_drift(&plan, &files, self.plans.settings.drift_threshold);
        let summary = report.summary();
        let drifted = report.drifted();
        if let Some(r) = self.plans.records.get_mut(&slug) {
            r.drift = Some(report);
        }
        if let Err(e) = self.save_plan(&slug) {
            self.plans.problems.push(e);
        }
        let event = if drifted { "drift" } else { "on_plan" };
        if let Err(e) = self.ledger_plan(&slug, event, &summary, Some(&plan)) {
            self.plans.problems.push(e);
        }
        true
    }

    /// What the plans ask of the user now: plans to decide on, and builds
    /// that strayed from theirs.
    pub fn plan_items(&self, now: Millis) -> Vec<AttentionItem> {
        let mut out = Vec::new();
        for m in &self.missions {
            let Some(r) = self.plans.records.get(&m.slug) else {
                continue;
            };
            let title = if m.contract.title.is_empty() {
                m.slug.as_str()
            } else {
                m.contract.title.as_str()
            };
            if r.status == PlanStatus::Proposed
                && let Some(plan) = &r.plan
            {
                let detail = format!(
                    "{} tasks over {} files: approve, change or cancel",
                    plan.tasks.len(),
                    plan.areas().len()
                );
                out.push(AttentionItem::plan_ready(
                    &m.slug,
                    m.session,
                    title,
                    &detail,
                    m.updated_at,
                ));
            }
            if let Some(d) = r.drift.as_ref().filter(|d| d.drifted())
                && now - m.updated_at < DRIFT_ITEM_MS
            {
                out.push(AttentionItem::plan_drift(
                    &m.slug,
                    m.session,
                    m.job,
                    title,
                    &d.summary(),
                    m.updated_at,
                ));
            }
        }
        out
    }
}
