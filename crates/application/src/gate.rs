//! The done-gate use cases.
//!
//! [`GateKeeper`] answers `Stop` and `SubagentStop` hooks on the hook
//! transport's threads: it runs the repository's checks, compares the
//! protected files with their pins, looks for the evidence the agent's last
//! message claims, and blocks the stop with a digest up to the gate's limit.
//! It also refuses writes to protected paths for the Guard. What it decides
//! is sent to [`GateService`], which lives on the UI thread, records it in
//! the ledger and turns it into wall chips, change-log rows and, once a gate
//! gives up, an inbox item.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::gate::{
    CheckRun, Executed, GateCheck, GateFindings, GateOrigin, GateOutcome, GateSpec, Pin, PinChange,
    digest, evidence_gaps, judge, pin_changes, protected_write,
};
use swarmux_domain::mission::MissionGate;
use swarmux_domain::verifier::{BaseSuite, HiddenTally};
use swarmux_domain::{
    AttentionItem, CheckpointReason, HookEvent, JobId, Millis, Priority, SessionId,
};

use crate::attention::{Change, ChangeKind};
use crate::checkpoint::{CheckpointService, Notes};
use crate::contracts::{ContractGate, ContractJudgement};
use crate::hookbus::{HookNotice, StopAnswer, StopContext, StopPolicy};
use crate::knowledge::{RuleCheck, RuleChecks};
use crate::ledger::LedgerService;
use crate::ports::{Clock, JobRepository};
use crate::verifier::{
    Verifier, VerifyRequest, fenced_folder, runs_holdout_command, runs_knowledge_command,
};
use crate::{AppError, AppResult, JobFact};

/// Commands remembered per job for the evidence check.
const EXECUTED_KEPT: usize = 500;
/// Gate changes kept for the change log.
const CHANGES_KEPT: usize = 100;

/// A repository's gate, as found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedGate {
    /// The repository root the checks run in and protected globs are
    /// relative to.
    pub root: PathBuf,
    pub spec: GateSpec,
    pub origin: GateOrigin,
    /// Why `.swarmux/gates.toml` couldn't be used; the spec is then the
    /// default (nothing but the gate's own folder protected, no checks).
    pub problem: Option<String>,
}

/// Where gates come from.
pub trait GateSource: Send + Sync {
    /// The gate of the repository `dir` lies in.
    fn load(&self, dir: &Path) -> LoadedGate;
    /// Content hashes of the files under `root` that `globs` protect.
    fn pins(&self, root: &Path, globs: &[String]) -> Vec<Pin>;
}

/// Runs one check: directly (no shell), in `root`, killed at its timeout,
/// with its output cut.
pub trait Checker: Send + Sync {
    fn run(&self, check: &GateCheck, root: &Path) -> CheckRun;
}

/// Where the job runner reports the shell commands agents ran.
pub trait ExecutionLog {
    fn executed(&self, job: JobId, command: &str, ok: bool);
}

/// Refuses writes for the Guard, before any rule is consulted.
pub trait WriteFence: Send + Sync {
    /// The rule that refuses `tool` on `subject` from `cwd` (for `job`,
    /// when known), if one does.
    fn refuses(&self, job: Option<JobId>, tool: &str, subject: &str, cwd: &str) -> Option<String>;
}

/// Gives a job checks and protected paths of its own, on top of its
/// repository's gate, and runs them on demand.
pub trait JobGates {
    /// From now on `job`'s stops are judged with `gate` as well.
    fn assign(&self, job: JobId, gate: MissionGate);
    /// Runs `job`'s checks now, in the repository it worked in (or
    /// `dir`'s), with no agent waiting. The verdict arrives like a stop's:
    /// a pass, or an escalation. Why the checks could not start, if not.
    fn verify(&self, job: JobId, dir: &Path) -> Result<(), String>;
}

/// Runs `checks` in `root`; a failing check is run once more, and passing
/// then marks it flaky instead of failed.
pub fn run_checks(checker: &dyn Checker, checks: &[GateCheck], root: &Path) -> Vec<CheckRun> {
    checks
        .iter()
        .map(|c| {
            let first = checker.run(c, root);
            if first.passed() || first.timed_out || first.spawn_error.is_some() {
                return first;
            }
            let mut again = checker.run(c, root);
            if again.passed() {
                again.flaky = true;
                again
            } else {
                first
            }
        })
        .collect()
}

/// One judged stop, or one manual run of a repository's checks.
#[derive(Clone, Debug, PartialEq)]
pub struct GateRecord {
    pub job: Option<JobId>,
    /// Claude Code's session id (empty for manual runs).
    pub session: String,
    /// `Stop` or `SubagentStop`; `None` for a manual run.
    pub event: Option<HookEvent>,
    pub root: String,
    pub origin: GateOrigin,
    pub outcome: GateOutcome,
    /// Blocks before this stop.
    pub blocks: u32,
    pub max_blocks: u32,
    pub findings: GateFindings,
    /// What the agent was told (on a block) or the user is told.
    pub digest: String,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Job(JobId),
    Session(String),
}

impl Key {
    fn of(job: Option<JobId>, session: &str) -> Key {
        job.map_or_else(|| Key::Session(session.to_string()), Key::Job)
    }
}

#[derive(Default)]
struct Watch {
    /// The repository root and its protected files' hashes at the start.
    pins: Option<(PathBuf, Vec<Pin>)>,
    /// The commit the session started from: where the tamper scan starts.
    base: Option<String>,
    executed: VecDeque<Executed>,
    /// A tool that can change files ran since the last pass: the checks
    /// must run at the next stop.
    touched: bool,
    lead_blocks: u32,
    subagent_blocks: u32,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Judges stops and fences protected paths. Shared with the hook
/// transport's threads.
pub struct GateKeeper {
    source: Arc<dyn GateSource>,
    checker: Arc<dyn Checker>,
    watches: Mutex<HashMap<Key, Watch>>,
    out: Mutex<Sender<GateRecord>>,
    /// Checks and protected paths of single jobs.
    jobs: Mutex<HashMap<JobId, MissionGate>>,
    /// Runs the checks in clean copies, with the hidden ones.
    verifier: Option<Verifier>,
    /// The checks knowledge rules add, and where their results go.
    rules: Option<Arc<dyn RuleChecks>>,
    /// The cross-repo check of a mission's change (contracts).
    contracts: Option<Arc<dyn ContractGate>>,
}

/// What running a repository's checks found.
#[derive(Default)]
struct Checked {
    runs: Vec<CheckRun>,
    /// Protected files the clean copy changes, as git compares them.
    protected: Vec<PinChange>,
    hidden: Option<HiddenTally>,
    tampering: Vec<String>,
    checks_weakened: bool,
    base_suite: Option<BaseSuite>,
    problem: Option<String>,
}

/// What running checks takes.
struct Work<'a> {
    checker: &'a dyn Checker,
    verifier: Option<&'a Verifier>,
}

impl Work<'_> {
    /// Runs the gate's checks: in a clean copy when there is a verifier and
    /// the repository is in git, else in place. The hidden checks and the
    /// tamper scan need the copy.
    fn check(&self, gate: &LoadedGate, base: Option<&str>, visible: bool, hidden: bool) -> Checked {
        let spec = &gate.spec;
        let in_place = |problem: Option<String>| Checked {
            runs: if visible {
                run_checks(self.checker, &spec.checks, &gate.root)
            } else {
                Vec::new()
            },
            problem,
            ..Checked::default()
        };
        // Checks of a workspace's repositories run in the task folder: one
        // clean copy of one repository can't hold them.
        if spec.checks.iter().any(|c| c.dir.is_some()) {
            return in_place(None);
        }
        let Some(verifier) = self.verifier.filter(|_| spec.verifier.clean) else {
            return in_place(None);
        };
        if !verifier.applies(&gate.root, spec, hidden) {
            return in_place(None);
        }
        let request = VerifyRequest {
            dir: &gate.root,
            spec,
            base,
            visible,
            hidden,
        };
        match verifier.verify(&request) {
            Ok(v) => Checked {
                runs: v.runs,
                protected: v.protected,
                hidden: v.hidden,
                tampering: v.tampering,
                checks_weakened: v.checks_weakened,
                base_suite: v.base_suite,
                problem: v.problem,
            },
            Err(why) => in_place(Some(format!(
                "verifier: no clean copy ({why}); the checks ran in the worktree"
            ))),
        }
    }
}

/// The protected files changed since `before` was taken in the worktree,
/// and those the clean copy changes since the job's first commit.
fn pin_findings(before: Option<&[Pin]>, now: &[Pin], copy: &[PinChange]) -> Vec<PinChange> {
    let mut out = before.map_or_else(Vec::new, |b| pin_changes(b, now));
    for change in copy {
        if !out.iter().any(|c| c.path() == change.path()) {
            out.push(change.clone());
        }
    }
    out.sort();
    out
}

impl GateKeeper {
    /// Runs the checks in clean copies made by `verifier`, followed by the
    /// repository's hidden checks, and scans for tampered tests. Tool calls
    /// that name the hidden checks or the copies are refused.
    pub fn with_verifier(mut self, verifier: Verifier) -> GateKeeper {
        self.verifier = Some(verifier);
        self
    }

    /// The keeper, and where its records arrive for [`GateService`].
    pub fn new(
        source: Arc<dyn GateSource>,
        checker: Arc<dyn Checker>,
    ) -> (GateKeeper, Receiver<GateRecord>) {
        let (tx, rx) = channel();
        let keeper = GateKeeper {
            source,
            checker,
            watches: Mutex::default(),
            out: Mutex::new(tx),
            jobs: Mutex::default(),
            verifier: None,
            rules: None,
            contracts: None,
        };
        (keeper, rx)
    }

    /// Runs the checks of knowledge rules in scope: lint rules' with the
    /// visible checks, gate rules' (missions only) after them as hidden
    /// ones, and shadow rules' for the record only.
    pub fn with_rules(mut self, rules: Arc<dyn RuleChecks>) -> GateKeeper {
        self.rules = Some(rules);
        self
    }

    /// Judges a mission's change against the contracts between repositories:
    /// the checks of the repositories that consume what changed run with
    /// the others, and a removed or renamed symbol that is still used
    /// fails the gate with the places that use it.
    pub fn with_contracts(mut self, contracts: Arc<dyn ContractGate>) -> GateKeeper {
        self.contracts = Some(contracts);
        self
    }

    /// The contract verdict of a mission's `job`, when it changed something
    /// since the last stop.
    fn contract_judgement(&self, job: Option<JobId>, key: &Key) -> Option<ContractJudgement> {
        let (job, contracts) = (job?, self.contracts.as_ref()?);
        let mission = lock(&self.jobs).contains_key(&job);
        let touched = lock(&self.watches).get(key).is_some_and(|w| w.touched);
        (mission && touched).then(|| contracts.judge(job)).flatten()
    }

    /// The rule checks of `job`, hidden ones when it is a mission's.
    fn rule_checks(&self, job: Option<JobId>) -> Vec<RuleCheck> {
        let (Some(job), Some(rules)) = (job, &self.rules) else {
            return Vec::new();
        };
        let mission = lock(&self.jobs).contains_key(&job);
        rules.checks(job, mission)
    }

    /// The gate of the repository `dir` lies in, with what `job` was given
    /// on top.
    fn load(&self, job: Option<JobId>, dir: &Path) -> LoadedGate {
        let mut gate = self.source.load(dir);
        if let Some(extra) = job.and_then(|j| lock(&self.jobs).get(&j).cloned()) {
            merge(&mut gate.spec, extra);
        }
        gate
    }

    /// Takes the pins of the repository `cwd` lies in for a session, unless
    /// it has them already.
    fn pin(&self, key: Key, cwd: &str) {
        if cwd.is_empty()
            || lock(&self.watches)
                .get(&key)
                .is_some_and(|w| w.pins.is_some())
        {
            return;
        }
        let job = match &key {
            Key::Job(j) => Some(*j),
            Key::Session(_) => None,
        };
        let gate = self.load(job, Path::new(cwd));
        let pins = self.source.pins(&gate.root, &gate.spec.protected_globs());
        let base = self.verifier.as_ref().and_then(|v| v.head(&gate.root));
        let mut watches = lock(&self.watches);
        let watch = watches.entry(key).or_default();
        if watch.pins.is_none() {
            watch.pins = Some((gate.root, pins));
            watch.base = base;
        }
    }

    fn send(&self, record: GateRecord) {
        // Nobody listening (the app is closing): nothing to record.
        let _ = lock(&self.out).send(record);
    }

    fn judge_stop(&self, stop: &StopContext) -> Option<GateRecord> {
        if stop.cwd.is_empty() {
            return None;
        }
        let mut gate = self.load(stop.job, Path::new(&stop.cwd));
        let rules = self.rule_checks(stop.job);
        add_lint(&mut gate.spec, &rules);
        let key = Key::of(stop.job, &stop.session);
        let subagent = stop.event == HookEvent::SubagentStop;
        let contract = if subagent {
            None
        } else {
            self.contract_judgement(stop.job, &key)
        };
        if let Some(c) = &contract {
            add_checks(&mut gate.spec, &c.checks);
        }
        let spec = &gate.spec;
        let (baseline, base, executed, blocks, touched) = {
            let mut watches = lock(&self.watches);
            let w = watches.entry(key.clone()).or_default();
            let blocks = if subagent {
                w.subagent_blocks
            } else {
                w.lead_blocks
            };
            let executed: Vec<Executed> = w.executed.iter().cloned().collect();
            (w.pins.clone(), w.base.clone(), executed, blocks, w.touched)
        };
        // A session that changed nothing since the last pass has nothing
        // for the checks to judge; its claims and pins still are. Hidden
        // checks judge the lead's work, not a subagent's part of it.
        let mut checked = if touched && (!subagent || spec.subagent_checks) {
            let work = Work {
                checker: &*self.checker,
                verifier: self.verifier.as_ref(),
            };
            work.check(&gate, base.as_deref(), true, !subagent)
        } else {
            Checked::default()
        };
        if let (true, Some(c)) = (touched && !subagent, contract) {
            checked.runs.push(c.run);
        }
        if touched && !subagent {
            run_rule_checks(
                &*self.checker,
                self.rules.as_deref(),
                stop.job,
                &rules,
                &gate.root,
                &mut checked,
            );
        }
        let pins = match &baseline {
            Some((root, before)) if *root == gate.root => pin_findings(
                Some(before),
                &self.source.pins(root, &spec.protected_globs()),
                &checked.protected,
            ),
            _ => pin_findings(None, &[], &checked.protected),
        };
        let gaps = if spec.evidence {
            evidence_gaps(&stop.last_message, &executed, &spec.checks)
        } else {
            Vec::new()
        };
        let findings = GateFindings {
            runs: checked.runs,
            pins,
            gaps,
            hidden: checked.hidden,
            tampering: checked.tampering,
            checks_weakened: checked.checks_weakened,
            base_suite: checked.base_suite,
        };
        let problem = gate.problem.clone().or(checked.problem);
        if !findings.checked() && findings.clean() && problem.is_none() {
            // Nothing was checked and nothing is wrong: not worth a record.
            return None;
        }
        let outcome = judge(&findings, blocks, spec.max_blocks);
        {
            let mut watches = lock(&self.watches);
            let w = watches.entry(key).or_default();
            let counter = if subagent {
                &mut w.subagent_blocks
            } else {
                &mut w.lead_blocks
            };
            *counter = match outcome {
                GateOutcome::Block => blocks + 1,
                GateOutcome::Pass | GateOutcome::Escalate => 0,
            };
            // After a block the next stop runs the checks again, changed
            // or not.
            if !subagent && outcome != GateOutcome::Block {
                w.touched = false;
            }
        }
        let text = digest(
            &findings,
            outcome,
            blocks,
            spec.max_blocks,
            &spec.protected_globs(),
        );
        Some(GateRecord {
            job: stop.job,
            session: stop.session.clone(),
            event: Some(stop.event),
            root: gate.root.display().to_string(),
            origin: gate.origin,
            outcome,
            blocks,
            max_blocks: spec.max_blocks,
            findings,
            digest: text,
            problem,
        })
    }
}

impl StopPolicy for GateKeeper {
    fn on_stop(&self, stop: &StopContext) -> StopAnswer {
        let Some(record) = self.judge_stop(stop) else {
            return StopAnswer::Allow;
        };
        let answer = match record.outcome {
            GateOutcome::Block => StopAnswer::Block {
                reason: record.digest.clone(),
            },
            GateOutcome::Pass | GateOutcome::Escalate => StopAnswer::Allow,
        };
        self.send(record);
        answer
    }

    fn observe(&self, notice: &HookNotice) {
        let key = Key::of(notice.job, &notice.session);
        match notice.event {
            HookEvent::SessionStart => self.pin(key, &notice.cwd),
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure
                if notice.tool.as_deref().is_some_and(changes_files) =>
            {
                lock(&self.watches).entry(key).or_default().touched = true;
            }
            _ => {}
        }
    }
}

impl ExecutionLog for GateKeeper {
    fn executed(&self, job: JobId, command: &str, ok: bool) {
        let mut watches = lock(&self.watches);
        let w = watches.entry(Key::Job(job)).or_default();
        w.touched = true;
        if w.executed.len() == EXECUTED_KEPT {
            w.executed.pop_front();
        }
        w.executed.push_back(Executed {
            command: command.to_string(),
            ok,
        });
    }
}

/// Tools that can change files (a shell can do anything).
fn changes_files(tool: &str) -> bool {
    matches!(
        tool,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Bash" | "PowerShell"
    )
}

impl<T: ExecutionLog + ?Sized> ExecutionLog for Arc<T> {
    fn executed(&self, job: JobId, command: &str, ok: bool) {
        (**self).executed(job, command, ok);
    }
}

impl WriteFence for GateKeeper {
    fn refuses(&self, job: Option<JobId>, tool: &str, subject: &str, cwd: &str) -> Option<String> {
        if let Some(verifier) = &self.verifier {
            let fenced = verifier.fenced();
            if let Some(folder) = fenced_folder(&fenced, subject, cwd) {
                return Some(format!(
                    "{} belongs to the verifier (gate)",
                    folder.display()
                ));
            }
            if runs_holdout_command(tool, subject) {
                return Some("hidden checks belong to the verifier (gate)".into());
            }
        }
        if self.rules.is_some() && runs_knowledge_command(tool, subject) {
            return Some(
                "the knowledge store and its rules' checks belong to the owner (gate)".into(),
            );
        }
        if !changes_files(tool) || cwd.is_empty() {
            return None;
        }
        let gate = self.load(job, Path::new(cwd));
        let root = gate.root.to_string_lossy().replace('\\', "/");
        let globs = gate.spec.protected_globs();
        protected_write(&globs, &root, tool, subject, cwd)
            .map(|glob| format!("protected path {glob} (gate)"))
    }
}

impl JobGates for GateKeeper {
    fn assign(&self, job: JobId, gate: MissionGate) {
        lock(&self.jobs).insert(job, gate);
    }

    fn verify(&self, job: JobId, dir: &Path) -> Result<(), String> {
        let (baseline, base) = lock(&self.watches)
            .get(&Key::Job(job))
            .map(|w| (w.pins.clone(), w.base.clone()))
            .unwrap_or_default();
        let dir = baseline
            .as_ref()
            .map_or_else(|| dir.to_path_buf(), |(root, _)| root.clone());
        let mut gate = self.load(Some(job), &dir);
        let rules = self.rule_checks(Some(job));
        add_lint(&mut gate.spec, &rules);
        let provider = self.rules.clone();
        let contracts = self.contracts.clone();
        let source = self.source.clone();
        let checker = self.checker.clone();
        let verifier = self.verifier.clone();
        let out = lock(&self.out).clone();
        std::thread::Builder::new()
            .name("swarmux-verify".into())
            .spawn(move || {
                let work = Work {
                    checker: &*checker,
                    verifier: verifier.as_ref(),
                };
                let mut gate = gate;
                let contract = contracts.and_then(|c| c.judge(job));
                if let Some(c) = &contract {
                    add_checks(&mut gate.spec, &c.checks);
                }
                let extra = contract.map(|c| c.run);
                let mut record = verify_now(
                    &*source,
                    &work,
                    job,
                    &gate,
                    baseline,
                    base.as_deref(),
                    extra,
                );
                if !rules.is_empty() {
                    let mut checked = Checked {
                        runs: record.findings.runs.clone(),
                        hidden: record.findings.hidden,
                        ..Checked::default()
                    };
                    run_rule_checks(
                        &*checker,
                        provider.as_deref(),
                        Some(job),
                        &rules,
                        &gate.root,
                        &mut checked,
                    );
                    if checked.hidden != record.findings.hidden {
                        record.findings.hidden = checked.hidden;
                        let checked_any = record.findings.checked();
                        record.outcome = if checked_any && record.findings.clean() {
                            GateOutcome::Pass
                        } else {
                            GateOutcome::Escalate
                        };
                        record.digest = digest(
                            &record.findings,
                            record.outcome,
                            0,
                            gate.spec.max_blocks,
                            &gate.spec.protected_globs(),
                        );
                    }
                }
                // Nobody listening (the app is closing): nothing to record.
                let _ = out.send(record);
            })
            .map(drop)
            .map_err(|e| format!("cannot start the checks: {e}"))
    }
}

impl<T: JobGates + ?Sized> JobGates for Arc<T> {
    fn assign(&self, job: JobId, gate: MissionGate) {
        (**self).assign(job, gate);
    }

    fn verify(&self, job: JobId, dir: &Path) -> Result<(), String> {
        (**self).verify(job, dir)
    }
}

/// `checks` (by name) added to the visible ones.
fn add_checks(spec: &mut GateSpec, checks: &[GateCheck]) {
    for c in checks {
        if !spec.checks.iter().any(|k| k.name == c.name) {
            spec.checks.push(c.clone());
        }
    }
}

/// The enforced lint rules' checks, added to the visible ones.
fn add_lint(spec: &mut GateSpec, rules: &[RuleCheck]) {
    for r in rules.iter().filter(|r| !r.hidden && !r.shadow) {
        if !spec.checks.iter().any(|c| c.name == r.check.name) {
            spec.checks.push(r.check.clone());
        }
    }
}

/// Runs the rules' checks that aren't among the visible ones and reports
/// every result: enforced gate rules count as hidden checks (only how
/// many ran and passed is ever told), shadow rules count for nothing.
fn run_rule_checks(
    checker: &dyn Checker,
    provider: Option<&dyn RuleChecks>,
    job: Option<JobId>,
    rules: &[RuleCheck],
    root: &Path,
    checked: &mut Checked,
) {
    let (Some(job), Some(provider)) = (job, provider) else {
        return;
    };
    for r in rules {
        if !r.hidden && !r.shadow {
            if let Some(run) = checked.runs.iter().find(|run| run.name == r.check.name) {
                provider.report(job, &r.rule, run.passed(), false);
            }
            continue;
        }
        let passed = run_checks(checker, std::slice::from_ref(&r.check), root)
            .first()
            .is_some_and(CheckRun::passed);
        provider.report(job, &r.rule, passed, r.shadow);
        if r.hidden && !r.shadow {
            let tally = checked
                .hidden
                .get_or_insert(HiddenTally { ran: 0, passed: 0 });
            tally.ran += 1;
            if passed {
                tally.passed += 1;
            }
        }
    }
}

/// `extra`'s checks (by name) and protected globs added to `spec`.
fn merge(spec: &mut GateSpec, extra: MissionGate) {
    for check in extra.checks {
        if !spec.checks.iter().any(|c| c.name == check.name) {
            spec.checks.push(check);
        }
    }
    for glob in extra.protected {
        if !spec.protected.contains(&glob) {
            spec.protected.push(glob);
        }
    }
}

/// Runs a job's checks with no agent waiting: they pass, or the user is
/// asked. With nothing to run there is nothing to pass.
fn verify_now(
    source: &dyn GateSource,
    work: &Work<'_>,
    job: JobId,
    gate: &LoadedGate,
    baseline: Option<(PathBuf, Vec<Pin>)>,
    base: Option<&str>,
    extra: Option<CheckRun>,
) -> GateRecord {
    let spec = &gate.spec;
    let globs = spec.protected_globs();
    let mut found = work.check(gate, base, true, true);
    found.runs.extend(extra);
    let pins = match &baseline {
        Some((root, before)) if *root == gate.root => {
            pin_findings(Some(before), &source.pins(root, &globs), &found.protected)
        }
        _ => pin_findings(None, &[], &found.protected),
    };
    let findings = GateFindings {
        runs: found.runs,
        pins,
        gaps: Vec::new(),
        hidden: found.hidden,
        tampering: found.tampering,
        checks_weakened: found.checks_weakened,
        base_suite: found.base_suite,
    };
    let checked = findings.checked();
    let outcome = if checked && findings.clean() {
        GateOutcome::Pass
    } else {
        GateOutcome::Escalate
    };
    let text = if checked {
        digest(&findings, outcome, 0, spec.max_blocks, &globs)
    } else {
        "swarmux gate: nothing to check".to_string()
    };
    GateRecord {
        job: Some(job),
        session: String::new(),
        event: None,
        root: gate.root.display().to_string(),
        origin: gate.origin,
        outcome,
        blocks: 0,
        max_blocks: spec.max_blocks,
        findings,
        digest: text,
        problem: gate.problem.clone().or(found.problem),
    }
}

/// The latest gate verdict of one job, as the wall and inbox show it.
#[derive(Clone, Debug, PartialEq)]
pub struct GateState {
    pub outcome: GateOutcome,
    /// Blocks so far, including this one.
    pub blocks: u32,
    pub max_blocks: u32,
    /// Each check's name, whether it passed and whether it was flaky.
    pub checks: Vec<(String, bool, bool)>,
    pub protected_changed: usize,
    pub evidence_gaps: usize,
    /// How the hidden checks went: counts only.
    pub hidden: Option<HiddenTally>,
    /// Tests weakened: edited, switched off, deleted, loosened.
    pub tampering: usize,
    /// Whether any of that touches what the checks run (inbox: P1, else P2).
    pub checks_weakened: bool,
    /// The base-suite check: `Some(true)` it passed, `Some(false)` it failed.
    pub base_suite: Option<bool>,
    pub digest: String,
    pub problem: Option<String>,
    pub at: Millis,
}

/// Records what the keeper decided and keeps the latest per job.
pub struct GateService {
    records: Receiver<GateRecord>,
    ledger: Rc<LedgerService>,
    jobs: Rc<dyn JobRepository>,
    clock: Rc<dyn Clock>,
    states: HashMap<JobId, GateState>,
    changes: VecDeque<Change>,
    checkpoints: Option<Rc<CheckpointService>>,
}

impl GateService {
    pub fn new(
        records: Receiver<GateRecord>,
        ledger: Rc<LedgerService>,
        jobs: Rc<dyn JobRepository>,
        clock: Rc<dyn Clock>,
    ) -> GateService {
        GateService {
            records,
            ledger,
            jobs,
            clock,
            states: HashMap::new(),
            changes: VecDeque::new(),
            checkpoints: None,
        }
    }

    /// Takes a checkpoint of the job's worktree whenever its gate passes.
    pub fn with_checkpoints(mut self, checkpoints: Rc<CheckpointService>) -> GateService {
        self.checkpoints = Some(checkpoints);
        self
    }

    /// Takes the verdicts that arrived and records them. Returns whether
    /// any arrived, and what could not be recorded.
    pub fn pump(&mut self) -> (bool, Vec<AppError>) {
        let mut errors = Vec::new();
        let mut any = false;
        while let Ok(record) = self.records.try_recv() {
            any = true;
            errors.extend(self.apply(&record).err());
        }
        (any, errors)
    }

    fn apply(&mut self, r: &GateRecord) -> AppResult<()> {
        let now = self.clock.now();
        if let Some(job) = r.job {
            let blocks = match r.outcome {
                GateOutcome::Block => r.blocks + 1,
                _ => r.blocks,
            };
            self.states.insert(
                job,
                GateState {
                    outcome: r.outcome,
                    blocks,
                    max_blocks: r.max_blocks,
                    checks: r
                        .findings
                        .runs
                        .iter()
                        .map(|c| (c.name.clone(), c.passed(), c.flaky))
                        .collect(),
                    protected_changed: r.findings.pins.len(),
                    evidence_gaps: r.findings.gaps.len(),
                    hidden: r.findings.hidden,
                    tampering: r.findings.tampering.len(),
                    checks_weakened: r.findings.checks_weakened,
                    base_suite: r.findings.base_suite.as_ref().map(BaseSuite::clean),
                    digest: r.digest.clone(),
                    problem: r.problem.clone(),
                    at: now,
                },
            );
        }
        let text = match r.outcome {
            GateOutcome::Pass => format!("gate passed: {}", first_line(&r.digest)),
            GateOutcome::Block => format!(
                "gate blocked a stop ({} of {}): {}",
                r.blocks + 1,
                r.max_blocks,
                headline(&r.findings)
            ),
            GateOutcome::Escalate => format!("gate gave up: {}", headline(&r.findings)),
        };
        if self.changes.len() == CHANGES_KEPT {
            self.changes.pop_front();
        }
        self.changes.push_back(Change {
            at: now,
            job: r.job,
            session: String::new(),
            kind: ChangeKind::Gate,
            text,
        });
        let session: Option<SessionId> = match r.job {
            Some(job) => self.jobs.get(job)?.map(|j| j.session_id),
            None => None,
        };
        self.ledger.record_gate(r, session, now)?;
        self.checkpoint_on_pass(r)
    }

    /// A passed stop of a job's lead is a point worth coming back to.
    fn checkpoint_on_pass(&self, r: &GateRecord) -> AppResult<()> {
        let (Some(checkpoints), Some(job)) = (&self.checkpoints, r.job) else {
            return Ok(());
        };
        if r.outcome != GateOutcome::Pass || r.event != Some(HookEvent::Stop) {
            return Ok(());
        }
        let notes = Notes {
            state: format!("The done-gate passed: {}", first_line(&r.digest)),
            ..Notes::default()
        };
        checkpoints
            .take(job, &r.session, CheckpointReason::GatePass, &notes)
            .map(drop)
    }

    /// The latest verdict for `job`.
    pub fn state(&self, job: JobId) -> Option<&GateState> {
        self.states.get(&job)
    }

    /// Every job's latest verdict.
    pub fn states(&self) -> &HashMap<JobId, GateState> {
        &self.states
    }

    /// Inbox items for the jobs whose gate gave up.
    pub fn escalations(&self, facts: &[JobFact]) -> Vec<AttentionItem> {
        let mut out: Vec<AttentionItem> = self
            .states
            .iter()
            .filter(|(_, s)| s.outcome == GateOutcome::Escalate)
            .filter_map(|(job, s)| {
                let f = facts.iter().find(|f| f.job_id == *job)?;
                let title = f.prompt.lines().next().unwrap_or_default().trim();
                let detail = s
                    .digest
                    .lines()
                    .nth(1)
                    .unwrap_or("checks still failing")
                    .to_string();
                let priority = if s.tampering > 0 && s.checks_weakened {
                    Priority::P1
                } else {
                    Priority::P2
                };
                Some(AttentionItem::gate_escalated_at(
                    *job,
                    f.session_id,
                    priority,
                    title,
                    &detail,
                    s.at,
                ))
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Blocks and verdicts, oldest first. Session names are empty; the
    /// attention service fills them in.
    pub fn changes(&self) -> Vec<Change> {
        self.changes.iter().cloned().collect()
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
}

/// The first thing wrong, in a few words.
fn headline(f: &GateFindings) -> String {
    if let Some(run) = f.failed().next() {
        return format!("{} {}", run.name, run.status_text());
    }
    if let Some(change) = f.pins.first() {
        return format!("protected {} {}", change.path(), change.verb());
    }
    if let Some(t) = f.tampering.first() {
        return format!("tests weakened: {t}");
    }
    if let Some(b) = f.base_suite.as_ref().filter(|b| !b.clean()) {
        return b.text();
    }
    if let Some(h) = f.hidden.filter(|h| !h.clean()) {
        return format!("{} of {} hidden checks failed", h.failed(), h.ran);
    }
    f.gaps
        .first()
        .cloned()
        .unwrap_or_else(|| "checks failed".to_string())
}

#[cfg(test)]
mod tests;
