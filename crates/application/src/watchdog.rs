//! The watchdog: notices agents that are stuck (looping, oscillating,
//! stalled, out of context, burning money) and climbs a ladder of
//! recoveries, cheapest first, before it puts the job in the inbox:
//!
//! 1. a nudge on the agent's next tool call,
//! 2. a checkpoint, a handoff and a fresh session,
//! 3. a Haiku diagnosis of the last steps,
//! 4. a fresh session from the last checkpoint before the trouble,
//! 5. a fresh session on a stronger model,
//! 6. a fresh session that plans smaller steps first,
//! 7. the inbox.
//!
//! Every alarm, rung and outcome is written to the ledger.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::budget::is_limit_error;
use swarmux_domain::telemetry::model_key;
use swarmux_domain::{
    AgentEvent, AlarmKind, AttentionItem, Checkpoint, CheckpointReason, Detector, Gauges, Handoff,
    JobId, JobStatus, Ladder, LadderPolicy, Millis, NUDGE_MARK, NextRung, Rung, RungOptions,
    Session, SessionId, SessionState, WatchAlarm, WatchLimits, describe,
};

use crate::attention::{Change, ChangeKind};
use crate::checkpoint::{CheckpointService, Notes};
use crate::ledger::{LedgerService, WatchdogEntry};
use crate::ports::Clock;
use crate::{AppError, AppResult, JobFact, JobService, Restart};

#[cfg(test)]
mod planted;
#[cfg(test)]
mod tests;

/// Steps of a job kept for a diagnosis.
const STEPS_KEPT: usize = 12;
/// Longest step line kept, in characters.
const STEP_MAX: usize = 160;
/// Longest goal carried into a handoff, in characters.
const GOAL_MAX: usize = 600;
/// Changes kept for the change log.
const CHANGES_KEPT: usize = 100;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the watchdog watches for and how it recovers.
#[derive(Clone, Debug, PartialEq)]
pub struct WatchdogSettings {
    /// Off: no detectors, no rungs; the loop breaker stops loops as before.
    pub enabled: bool,
    pub limits: WatchLimits,
    pub ladder: LadderPolicy,
    /// Models a stuck job may move up to, weakest first.
    pub escalate_to: Vec<String>,
}

impl Default for WatchdogSettings {
    fn default() -> WatchdogSettings {
        WatchdogSettings {
            enabled: true,
            limits: WatchLimits::default(),
            ladder: LadderPolicy::default(),
            escalate_to: vec!["sonnet".into(), "opus".into()],
        }
    }
}

impl WatchdogSettings {
    /// Identical failures that stop a job, given the guard's setting: with
    /// the watchdog on, it nudges at the guard's count and the loop breaker
    /// stops the job one failure later.
    pub fn breaker_repeats(&self, guard: u32) -> u32 {
        if self.enabled && guard > 0 && self.limits.loop_repeats > 0 {
            guard + 1
        } else {
            guard
        }
    }
}

/// A watchdog message for a running agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nudge {
    pub text: String,
    /// Only this call, as (tool, summary), carries it; any call when `None`.
    pub call: Option<(String, String)>,
}

/// Gets a message to a running agent.
pub trait SessionMessenger {
    /// Leaves `nudge` for `job`. False when nothing can carry it there.
    fn send(&self, job: JobId, nudge: Nudge) -> bool;
    /// Messages that reached their agent since the last call.
    fn delivered(&self) -> Vec<(JobId, String)>;
    /// Drops what waits for `job`: it was stopped.
    fn forget(&self, job: JobId);
}

/// Where the hook transport picks waiting messages up. Called on its
/// threads.
pub trait NudgeGate: Send + Sync {
    /// The message for `job`, when this call (`tool`, `subject`) should
    /// carry it. Taking it delivers it.
    fn take(&self, job: JobId, tool: &str, subject: &str) -> Option<String>;
}

/// Messages waiting for the agents' next tool calls, shared with the hook
/// transport. A message goes out as the reason the call was not run
/// (PreToolUse `deny`), which Claude Code shows the agent.
#[derive(Default)]
pub struct NudgeBoard {
    waiting: Mutex<HashMap<JobId, Nudge>>,
    delivered: Mutex<Vec<(JobId, String)>>,
}

impl NudgeBoard {
    pub fn new() -> Arc<NudgeBoard> {
        Arc::new(NudgeBoard::default())
    }

    /// Whether a message waits for `job`.
    pub fn is_waiting(&self, job: JobId) -> bool {
        lock(&self.waiting).contains_key(&job)
    }
}

impl NudgeGate for NudgeBoard {
    fn take(&self, job: JobId, tool: &str, subject: &str) -> Option<String> {
        let mut waiting = lock(&self.waiting);
        let hit = waiting.get(&job).is_some_and(|n| {
            n.call
                .as_ref()
                .is_none_or(|c| call_matches(c, tool, subject))
        });
        if !hit {
            return None;
        }
        let nudge = waiting.remove(&job)?;
        lock(&self.delivered).push((job, nudge.text.clone()));
        Some(format!("{NUDGE_MARK}{}", nudge.text))
    }
}

/// The hook-backed [`SessionMessenger`]: messages wait on a [`NudgeBoard`]
/// for the agent's next tool call.
pub struct HookMessenger(pub Arc<NudgeBoard>);

impl SessionMessenger for HookMessenger {
    fn send(&self, job: JobId, nudge: Nudge) -> bool {
        lock(&self.0.waiting).insert(job, nudge);
        true
    }

    fn delivered(&self) -> Vec<(JobId, String)> {
        std::mem::take(&mut *lock(&self.0.delivered))
    }

    fn forget(&self, job: JobId) {
        lock(&self.0.waiting).remove(&job);
    }
}

/// A messenger that reaches nobody: the nudge rung is skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMessenger;

impl SessionMessenger for NoMessenger {
    fn send(&self, _: JobId, _: Nudge) -> bool {
        false
    }

    fn delivered(&self) -> Vec<(JobId, String)> {
        Vec::new()
    }

    fn forget(&self, _: JobId) {}
}

/// Whether a hook call (`tool`, `subject`) is the call a stream event
/// summarised as `call`. Summaries may be cut short with `…`.
fn call_matches((name, summary): &(String, String), tool: &str, subject: &str) -> bool {
    if name != tool {
        return false;
    }
    let s = summary.trim_end_matches('…');
    s.is_empty() || s == name || subject.starts_with(s)
}

/// What a diagnosis is given: nothing but the watchdog's notes and the
/// one-line steps the job's log already shows.
#[derive(Clone, Debug, PartialEq)]
pub struct DiagnosisRequest {
    pub job: JobId,
    pub goal: String,
    pub alarm: String,
    pub steps: Vec<String>,
    pub tried: Vec<String>,
}

/// Which step went wrong, and what to do instead.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnosis {
    pub suspect: String,
    pub instruction: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiagnosisReply {
    pub result: Result<Diagnosis, String>,
    pub cost_usd: f64,
}

/// Receives a diagnosis, from any thread.
pub type DiagnosisSink = Box<dyn FnOnce(DiagnosisReply) + Send + 'static>;

/// Reads a stuck job's last steps and names what went wrong. Never blocks;
/// the answer arrives through the sink.
pub trait Diagnoser {
    fn diagnose(&self, request: DiagnosisRequest, sink: DiagnosisSink);
}

/// Puts a worktree's files back as a checkpoint had them.
pub trait CheckpointRestore {
    /// Commits what `worktree` holds now (nothing is lost), then restores
    /// its files to `commit`. Returns the commit that keeps what was replaced.
    fn restore(&self, worktree: &Path, commit: &str) -> AppResult<String>;
}

/// Counts since the start, for the status line and the tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchStats {
    pub alarms: u32,
    /// Rungs taken, in [`Rung::ALL`] order.
    pub rungs: [u32; 7],
    /// Work that finished after at least one rung, below the inbox.
    pub rescued: u32,
    /// Work handed to the user.
    pub escalated: u32,
    /// Alarms that cleared without a further rung.
    pub settled: u32,
}

impl WatchStats {
    pub fn taken(&self, rung: Rung) -> u32 {
        self.rungs[Rung::ALL.iter().position(|r| *r == rung).unwrap_or(0)]
    }
}

/// What one [`Watchdog::pump`] did.
#[derive(Debug, Default)]
pub struct WatchReport {
    pub errors: Vec<AppError>,
    /// Sessions made for fresh starts.
    pub new_sessions: Vec<Session>,
    pub changed: bool,
}

/// One run of work: a job and the jobs that took it over.
struct Chain {
    root: JobId,
    current: JobId,
    jobs: Vec<JobId>,
    session: SessionId,
    goal: String,
    title: String,
    model: String,
    ladder: Ladder,
    /// The trouble being worked on.
    open: Option<WatchAlarm>,
    /// Checkpoints before this are from before the trouble.
    first_alarm: Option<Millis>,
    diagnosis: Option<Diagnosis>,
    /// What was tried, one line each.
    trail: Vec<String>,
    escalation: Option<AttentionItem>,
    /// A diagnosis was asked for; a job that ended waits for it until then.
    diagnosing_until: Option<Millis>,
    /// The work ended; nothing more to do.
    closed: bool,
}

/// A diagnosis that came back, with the root of the work it is for.
type Reply = (JobId, DiagnosisReply);

/// Watches running jobs and recovers stuck ones.
pub struct Watchdog {
    settings: WatchdogSettings,
    clock: Rc<dyn Clock>,
    messenger: Rc<dyn SessionMessenger>,
    ledger: Option<Rc<LedgerService>>,
    checkpoints: Option<Rc<CheckpointService>>,
    restore: Option<Rc<dyn CheckpointRestore>>,
    diagnoser: Option<Rc<dyn Diagnoser>>,
    detectors: HashMap<JobId, Detector>,
    steps: HashMap<JobId, VecDeque<String>>,
    chains: Vec<Chain>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    stats: WatchStats,
    changes: VecDeque<Change>,
}

impl Watchdog {
    pub fn new(
        settings: WatchdogSettings,
        messenger: Rc<dyn SessionMessenger>,
        clock: Rc<dyn Clock>,
    ) -> Watchdog {
        let (tx, rx) = channel();
        Watchdog {
            settings,
            clock,
            messenger,
            ledger: None,
            checkpoints: None,
            restore: None,
            diagnoser: None,
            detectors: HashMap::new(),
            steps: HashMap::new(),
            chains: Vec::new(),
            tx,
            rx,
            stats: WatchStats::default(),
            changes: VecDeque::new(),
        }
    }

    /// Writes every alarm, rung and outcome to the ledger.
    pub fn with_ledger(mut self, ledger: Rc<LedgerService>) -> Watchdog {
        self.ledger = Some(ledger);
        self
    }

    /// Takes checkpoints for handoffs, and finds the ones forks start from.
    pub fn with_checkpoints(mut self, checkpoints: Rc<CheckpointService>) -> Watchdog {
        self.checkpoints = Some(checkpoints);
        self
    }

    /// Lets the fork rung put a worktree back to a checkpoint.
    pub fn with_restore(mut self, restore: Rc<dyn CheckpointRestore>) -> Watchdog {
        self.restore = Some(restore);
        self
    }

    pub fn with_diagnoser(mut self, diagnoser: Rc<dyn Diagnoser>) -> Watchdog {
        self.diagnoser = Some(diagnoser);
        self
    }

    pub fn settings(&self) -> &WatchdogSettings {
        &self.settings
    }

    pub fn stats(&self) -> &WatchStats {
        &self.stats
    }

    /// Items for the inbox: work the ladder could not rescue.
    pub fn escalations(&self) -> Vec<AttentionItem> {
        self.chains
            .iter()
            .filter_map(|c| c.escalation.clone())
            .collect()
    }

    /// Alarms, rungs and outcomes for the change log, oldest first.
    pub fn changes(&self) -> Vec<Change> {
        self.changes.iter().cloned().collect()
    }

    /// What is wrong with `job` now, if anything.
    pub fn health(&self, job: JobId) -> Option<AlarmKind> {
        self.chain_of(job)
            .and_then(|i| self.chains[i].open.as_ref())
            .map(|a| a.kind)
    }

    /// The rungs tried for the work `job` belongs to, one line each.
    pub fn trail(&self, job: JobId) -> &[String] {
        self.chain_of(job).map_or(&[], |i| &self.chains[i].trail)
    }

    fn chain_of(&self, job: JobId) -> Option<usize> {
        self.chains.iter().rposition(|c| c.jobs.contains(&job))
    }

    /// Reads the agents' events and gauges, and acts on what they show.
    /// `compactions` says how often a job's context was compacted.
    pub fn pump(
        &mut self,
        jobs: &mut JobService,
        events: &[(JobId, AgentEvent)],
        compactions: &dyn Fn(JobId) -> u32,
    ) -> WatchReport {
        let mut report = WatchReport::default();
        if !self.settings.enabled {
            return report;
        }
        let now = self.clock.now();
        let facts: HashMap<JobId, JobFact> =
            jobs.facts().into_iter().map(|f| (f.job_id, f)).collect();
        self.note_delivered(jobs, now, &mut report);
        let mut raised = Vec::new();
        for (job, ev) in events {
            let Some(fact) = facts.get(job).filter(|f| f.review_of.is_none()) else {
                continue;
            };
            self.keep_step(*job, ev);
            let limits = self.settings.limits;
            let det = self
                .detectors
                .entry(*job)
                .or_insert_with(|| Detector::new(limits, &fact.model, now));
            raised.extend(det.observe(ev, now).map(|a| (*job, a)));
        }
        for fact in facts.values() {
            if fact.status != JobStatus::Running || fact.review_of.is_some() {
                continue;
            }
            let limits = self.settings.limits;
            let gauges = gauges_of(fact, compactions(fact.job_id));
            let det = self
                .detectors
                .entry(fact.job_id)
                .or_insert_with(|| Detector::new(limits, &fact.model, now));
            raised.extend(det.tick(now, &gauges).into_iter().map(|a| (fact.job_id, a)));
        }
        for (job, alarm) in raised {
            if let Some(fact) = facts.get(&job) {
                self.raise(fact, alarm, jobs, &mut report);
            }
        }
        while let Ok(reply) = self.rx.try_recv() {
            self.on_reply(reply, jobs, &mut report);
        }
        for i in 0..self.chains.len() {
            if self.chains[i].closed {
                continue;
            }
            let Some(fact) = facts.get(&self.chains[i].current).cloned() else {
                continue;
            };
            if fact.status.is_finished() {
                self.ended(i, &fact, jobs, &mut report);
            } else if fact.status == JobStatus::Running {
                let gauges = gauges_of(&fact, compactions(fact.job_id));
                self.step(i, &fact, &gauges, jobs, &mut report);
            }
        }
        self.detectors
            .retain(|j, _| facts.get(j).is_some_and(|f| !f.status.is_finished()));
        self.steps.retain(|j, _| facts.contains_key(j));
        // Finished work is in the jobs' logs and the ledger; the inbox keeps
        // what it still needs.
        self.chains.retain(|c| !c.closed || c.escalation.is_some());
        report
    }

    fn keep_step(&mut self, job: JobId, ev: &AgentEvent) {
        if !matches!(
            ev,
            AgentEvent::ToolUse { .. } | AgentEvent::ToolResult { .. }
        ) {
            return;
        }
        let steps = self.steps.entry(job).or_default();
        if steps.len() == STEPS_KEPT {
            steps.pop_front();
        }
        steps.push_back(describe(ev).chars().take(STEP_MAX).collect());
    }

    fn note_delivered(&mut self, jobs: &mut JobService, now: Millis, report: &mut WatchReport) {
        for (job, text) in self.messenger.delivered() {
            let Some(i) = self.chain_of(job) else {
                continue;
            };
            let kind = self.alarm_name(i);
            jobs.note(job, "watchdog", "the nudge reached the agent");
            self.ledger(
                i,
                job,
                "delivered",
                &kind,
                Some(Rung::Nudge),
                &text,
                None,
                now,
                report,
            );
        }
    }

    fn alarm_name(&self, i: usize) -> String {
        let c = &self.chains[i];
        c.open
            .as_ref()
            .map(|a| a.kind)
            .or_else(|| c.ladder.taken().last().map(|t| t.kind))
            .map_or("-", AlarmKind::as_str)
            .to_string()
    }

    /// A detector fired for the job `fact`.
    fn raise(
        &mut self,
        fact: &JobFact,
        alarm: WatchAlarm,
        jobs: &mut JobService,
        report: &mut WatchReport,
    ) {
        let job = fact.job_id;
        let i = match self.chains.iter().position(|c| c.current == job) {
            Some(i) => i,
            None => {
                self.chains.push(Chain {
                    root: job,
                    current: job,
                    jobs: vec![job],
                    session: fact.session_id,
                    goal: fact.prompt.clone(),
                    title: title_of(&fact.prompt),
                    model: fact.model.clone(),
                    ladder: Ladder::default(),
                    open: None,
                    first_alarm: None,
                    diagnosis: None,
                    trail: Vec::new(),
                    escalation: None,
                    diagnosing_until: None,
                    closed: false,
                });
                self.chains.len() - 1
            }
        };
        let c = &mut self.chains[i];
        if c.closed || c.ladder.escalated() {
            return;
        }
        let fresh = c.open.as_ref().is_none_or(|o| o.kind != alarm.kind);
        c.first_alarm.get_or_insert(alarm.at);
        c.open = Some(alarm.clone());
        self.stats.alarms += 1;
        if fresh {
            let line = format!(
                "{} {}: {}",
                alarm.kind.glyph(),
                alarm.kind.as_str(),
                alarm.detail
            );
            jobs.note(job, "watchdog", &line);
            self.change(alarm.at, job, &format!("watchdog: {}", line));
            self.ledger(
                i,
                job,
                "alarm",
                alarm.kind.as_str(),
                None,
                &alarm.detail,
                None,
                alarm.at,
                report,
            );
        }
    }

    /// Decides about a running job's open alarm.
    fn step(
        &mut self,
        i: usize,
        fact: &JobFact,
        gauges: &Gauges,
        jobs: &mut JobService,
        report: &mut WatchReport,
    ) {
        let now = self.clock.now();
        let Some(alarm) = self.chains[i].open.clone() else {
            return;
        };
        let since = self.chains[i]
            .ladder
            .taken()
            .last()
            .map_or(alarm.at, |t| t.at.max(alarm.at));
        let still = self
            .detectors
            .get(&fact.job_id)
            .is_none_or(|d| d.still(&alarm, since, now, gauges));
        if !still {
            self.settle(i, fact.job_id, &alarm, now, report);
            return;
        }
        // Settling: nothing to look up (checkpoints are read from storage).
        let ladder = &self.chains[i].ladder;
        let quick = RungOptions {
            live: true,
            ..RungOptions::default()
        };
        if !matches!(
            ladder.plan(alarm.kind, now, &self.settings.ladder, &quick),
            NextRung::Take(_)
        ) {
            return;
        }
        let options = self.options(i, fact, &alarm, true);
        if let NextRung::Take(rung) =
            self.chains[i]
                .ladder
                .plan(alarm.kind, now, &self.settings.ladder, &options)
        {
            self.take(i, rung, &alarm, fact, jobs, report);
        }
    }

    fn settle(
        &mut self,
        i: usize,
        job: JobId,
        alarm: &WatchAlarm,
        now: Millis,
        report: &mut WatchReport,
    ) {
        self.chains[i].open = None;
        self.stats.settled += 1;
        self.change(
            now,
            job,
            &format!("watchdog: {} cleared", alarm.kind.as_str()),
        );
        self.ledger(
            i,
            job,
            "settled",
            alarm.kind.as_str(),
            None,
            "the agent is making progress again",
            None,
            now,
            report,
        );
    }

    /// The chain's current job finished.
    fn ended(&mut self, i: usize, fact: &JobFact, jobs: &mut JobService, report: &mut WatchReport) {
        let now = self.clock.now();
        let job = fact.job_id;
        let c = &self.chains[i];
        let worked_on = !c.ladder.taken().is_empty();
        match fact.status {
            JobStatus::Done => {
                if worked_on && !c.ladder.escalated() {
                    let rung = c.ladder.highest().unwrap_or(Rung::Nudge);
                    self.stats.rescued += 1;
                    let kind = self.alarm_name(i);
                    let text = format!("recovered after {}", rung.label());
                    jobs.record_recovery(job, &text);
                    self.change(now, job, &format!("watchdog: {text}"));
                    self.ledger(
                        i,
                        job,
                        "rescued",
                        &kind,
                        Some(rung),
                        &text,
                        None,
                        now,
                        report,
                    );
                }
                self.chains[i].closed = true;
            }
            // Stopped by the user, or replaced by a rung.
            JobStatus::Cancelled if fact.loop_stops == 0 => self.chains[i].closed = true,
            // Failed, or stopped by the loop breaker.
            _ => {
                let error = fact.error.clone().unwrap_or_default();
                // Usage limits are the governor's: it resumes after the reset.
                let stopped_by_loop = fact.loop_stops > 0;
                if c.ladder.escalated() || is_limit_error(&error) || !(worked_on || stopped_by_loop)
                {
                    self.chains[i].closed = true;
                    return;
                }
                // A diagnosis on its way informs the next rung: wait for it a while.
                if c.diagnosis.is_none() && c.diagnosing_until.is_some_and(|t| now < t) {
                    return;
                }
                let kind = c
                    .open
                    .as_ref()
                    .map(|a| a.kind)
                    .or_else(|| c.ladder.taken().last().map(|t| t.kind))
                    .unwrap_or(AlarmKind::Loop);
                let detail = match &c.open {
                    Some(a) if !a.reachable && a.detail.starts_with(ENDED) => a.detail.clone(),
                    Some(a) => format!("{ENDED}{} ({})", first_line(&error, "no result"), a.detail),
                    None => format!("{ENDED}{}", first_line(&error, "no result")),
                };
                let alarm = WatchAlarm {
                    kind,
                    detail,
                    at: now,
                    reachable: false,
                    call: None,
                    error: None,
                };
                self.chains[i].open = Some(alarm.clone());
                self.chains[i].first_alarm.get_or_insert(now);
                let options = self.options(i, fact, &alarm, false);
                if let NextRung::Take(rung) =
                    self.chains[i]
                        .ladder
                        .plan(kind, now, &self.settings.ladder, &options)
                {
                    self.take(i, rung, &alarm, fact, jobs, report);
                }
            }
        }
    }

    fn options(&self, i: usize, fact: &JobFact, alarm: &WatchAlarm, live: bool) -> RungOptions {
        RungOptions {
            live,
            reachable: alarm.reachable,
            checkpoint: self.restore.is_some() && self.before_trouble(i).is_some(),
            stronger_model: self.stronger(&fact.model).is_some(),
        }
    }

    /// The newest checkpoint of the work from before its first alarm.
    fn before_trouble(&self, i: usize) -> Option<Checkpoint> {
        let c = &self.chains[i];
        let since = c.first_alarm?;
        let checkpoints = self.checkpoints.as_ref()?;
        c.jobs
            .iter()
            .filter_map(|j| checkpoints.list(*j).ok())
            .flatten()
            .filter(|cp| cp.at < since && cp.reason != CheckpointReason::Recovery)
            .max_by_key(|cp| (cp.at, cp.id))
    }

    /// The next model up from `model`, when one is configured.
    fn stronger(&self, model: &str) -> Option<String> {
        let key = model_key(model);
        let ladder = &self.settings.escalate_to;
        let at = ladder
            .iter()
            .position(|m| key.contains(&model_key(m)) || model_key(m).contains(&key));
        match at {
            Some(p) => ladder.get(p + 1).cloned(),
            // Not on the list: the first listed model of a higher tier.
            None => {
                let current = tier(&key)?;
                ladder
                    .iter()
                    .find(|m| tier(&model_key(m)).is_some_and(|t| t > current))
                    .cloned()
            }
        }
    }

    /// Carries out `rung` for the chain's current job.
    fn take(
        &mut self,
        i: usize,
        rung: Rung,
        alarm: &WatchAlarm,
        fact: &JobFact,
        jobs: &mut JobService,
        report: &mut WatchReport,
    ) {
        let now = self.clock.now();
        let job = self.chains[i].current;
        let root = self.chains[i].root;
        self.chains[i].ladder.record(rung, alarm.kind, now);
        self.stats.rungs[Rung::ALL.iter().position(|r| *r == rung).unwrap_or(0)] += 1;
        report.changed = true;
        let outcome: Result<(String, Option<JobId>), AppError> = match rung {
            Rung::Nudge => {
                let call = matches!(alarm.kind, AlarmKind::Loop | AlarmKind::Oscillation)
                    .then(|| alarm.call.clone())
                    .flatten();
                let text = nudge_text(alarm);
                if self.messenger.send(job, Nudge { text, call }) {
                    Ok(("waiting for the agent's next call".into(), None))
                } else {
                    Ok(("no way to reach the agent".into(), None))
                }
            }
            Rung::Diagnose => match &self.diagnoser {
                Some(d) => {
                    let request = DiagnosisRequest {
                        job,
                        goal: cut(&self.chains[i].goal, GOAL_MAX),
                        alarm: format!("{}: {}", alarm.kind.as_str(), alarm.detail),
                        steps: self
                            .steps
                            .get(&job)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default(),
                        tried: self.chains[i].trail.clone(),
                    };
                    let tx = self.tx.clone();
                    d.diagnose(
                        request,
                        Box::new(move |reply| {
                            let _ = tx.send((root, reply));
                        }),
                    );
                    self.chains[i].diagnosing_until =
                        Some(now + self.settings.ladder.nudge_cooldown_ms);
                    Ok(("asked Haiku".into(), None))
                }
                None => Ok(("no diagnoser".into(), None)),
            },
            Rung::Fresh => self.restart(i, alarm, jobs, None, "", report),
            Rung::Fork => self.fork(i, alarm, jobs, report),
            Rung::Escalate => {
                let model = self.stronger(&fact.model);
                self.restart(i, alarm, jobs, model, "", report)
            }
            Rung::Replan => self.restart(i, alarm, jobs, None, REPLAN, report),
            Rung::Inbox => {
                self.escalate(i, alarm, fact);
                Ok(("waiting for you".into(), None))
            }
        };
        let (what, next) = match outcome {
            Ok(done) => done,
            Err(e) => {
                let text = format!("failed: {e}");
                report.errors.push(e);
                (text, None)
            }
        };
        let line = format!("{}: {what}", rung.label());
        self.chains[i].trail.push(line.clone());
        jobs.record_recovery(job, &line);
        if let Some(next) = next {
            jobs.record_recovery(
                next,
                &format!("continues job {job} after a {}", alarm.kind.as_str()),
            );
        }
        self.change(now, job, &format!("watchdog: {line}"));
        self.ledger(
            i,
            job,
            "rung",
            alarm.kind.as_str(),
            Some(rung),
            &what,
            next,
            now,
            report,
        );
    }

    /// Hands the work to a new session: checkpoint, handoff, new job.
    #[allow(clippy::too_many_arguments)]
    fn restart(
        &mut self,
        i: usize,
        alarm: &WatchAlarm,
        jobs: &mut JobService,
        model: Option<String>,
        extra: &str,
        report: &mut WatchReport,
    ) -> Result<(String, Option<JobId>), AppError> {
        let job = self.chains[i].current;
        let briefing = self.handoff(i, alarm, job)?;
        let prompt = format!("{}\n\n---\n{briefing}{extra}", self.chains[i].goal);
        let what = match &model {
            Some(m) => format!("new session on {m}"),
            None => "new session with a handoff".to_string(),
        };
        self.start_successor(i, job, prompt, model, jobs, report)
            .map(|next| (what, Some(next)))
    }

    /// Back to the last checkpoint before the trouble, in a new session.
    fn fork(
        &mut self,
        i: usize,
        alarm: &WatchAlarm,
        jobs: &mut JobService,
        report: &mut WatchReport,
    ) -> Result<(String, Option<JobId>), AppError> {
        let job = self.chains[i].current;
        let (Some(cp), Some(restore), Some(checkpoints)) = (
            self.before_trouble(i),
            self.restore.clone(),
            self.checkpoints.clone(),
        ) else {
            return Ok(("no checkpoint from before the trouble".into(), None));
        };
        // The agent must stop writing before its files are put back.
        match jobs.cancel(job) {
            Ok(()) | Err(AppError::JobFinished(_)) => {}
            Err(e) => return Err(e),
        }
        let worktree = checkpoints
            .worktree(job)?
            .map_or_else(|| cp.worktree.clone(), |w| w.path);
        let saved = restore.restore(&worktree, &cp.commit)?;
        let mut briefing = cp
            .handoff
            .as_ref()
            .map(Handoff::briefing)
            .unwrap_or_default();
        briefing.push_str(&format!(
            "The files were put back as they were at checkpoint {} (before the trouble). \
             A later attempt from here got stuck ({}: {}); its files are kept in commit {}. \
             Do not repeat it: take a different route.\n",
            short(&cp.commit),
            alarm.kind.as_str(),
            alarm.detail,
            short(&saved),
        ));
        if let Some(d) = &self.chains[i].diagnosis {
            briefing.push_str(&format!(
                "A diagnosis of that attempt: {}. {}\n",
                d.suspect, d.instruction
            ));
        }
        let prompt = format!("{}\n\n---\n{briefing}", self.chains[i].goal);
        let next = self.start_successor(i, job, prompt, None, jobs, report)?;
        Ok((
            format!("back to checkpoint {}", short(&cp.commit)),
            Some(next),
        ))
    }

    fn start_successor(
        &mut self,
        i: usize,
        job: JobId,
        prompt: String,
        model: Option<String>,
        jobs: &mut JobService,
        report: &mut WatchReport,
    ) -> AppResult<JobId> {
        self.messenger.forget(job);
        let (next, session) = jobs.restart(job, Restart { prompt, model })?;
        let c = &mut self.chains[i];
        c.current = next;
        c.jobs.push(next);
        c.session = session.id;
        // The new session starts calm; its own detectors judge it.
        c.open = None;
        if let Some(f) = jobs.facts().into_iter().find(|f| f.job_id == next) {
            c.model = f.model;
        }
        report.new_sessions.push(session);
        Ok(next)
    }

    /// The handoff briefing for a successor of `job`: a checkpoint when the
    /// job has a worktree, else the same notes on their own.
    fn handoff(&self, i: usize, alarm: &WatchAlarm, job: JobId) -> AppResult<String> {
        let c = &self.chains[i];
        let mut tried = c.trail.clone();
        tried.push(format!("{}: {}", alarm.kind.as_str(), alarm.detail));
        let mut next_step = "Check the files in the working folder, then carry on with a \
                             different approach from the one that got stuck."
            .to_string();
        if let Some(d) = &c.diagnosis {
            next_step = format!("{} {} {next_step}", d.suspect, d.instruction);
        }
        let notes = Notes {
            state: format!(
                "The watchdog stopped the previous session: {}.",
                alarm.kind.as_str()
            ),
            tried_and_failed: tried,
            next_step,
        };
        if let Some(checkpoints) = &self.checkpoints
            && let Some(cp) = checkpoints.take(job, "", CheckpointReason::Recovery, &notes)?
            && let Some(h) = &cp.handoff
        {
            return Ok(h.briefing());
        }
        Ok(Handoff {
            goal: cut(&c.goal, GOAL_MAX),
            state: notes.state,
            decisions: Vec::new(),
            tried_and_failed: notes.tried_and_failed,
            next_step: notes.next_step,
            open_questions: Vec::new(),
        }
        .briefing())
    }

    fn escalate(&mut self, i: usize, alarm: &WatchAlarm, fact: &JobFact) {
        let c = &self.chains[i];
        let tried: Vec<&str> = c
            .ladder
            .taken()
            .iter()
            .map(|t| t.rung.label())
            .filter(|l| *l != Rung::Inbox.label())
            .collect();
        let mut detail = format!("{}: {}", alarm.kind.as_str(), alarm.detail);
        if !tried.is_empty() {
            detail.push_str(&format!(" · tried {}", tried.join(" → ")));
        }
        if let Some(d) = &c.diagnosis {
            detail.push_str(&format!(" · diagnosis: {}", d.suspect));
        }
        let item = AttentionItem::recovery_exhausted(
            fact.job_id,
            c.session,
            &c.title,
            &detail,
            self.clock.now(),
        );
        self.stats.escalated += 1;
        self.chains[i].escalation = Some(item);
    }

    fn on_reply(&mut self, reply: Reply, jobs: &mut JobService, report: &mut WatchReport) {
        let now = self.clock.now();
        let (root, reply) = reply;
        let Some(i) = self.chains.iter().position(|c| c.root == root) else {
            return;
        };
        let job = self.chains[i].current;
        let kind = self.alarm_name(i);
        let live = jobs
            .facts()
            .iter()
            .any(|f| f.job_id == job && f.status == JobStatus::Running);
        let text = match reply.result {
            Ok(d) => {
                let text = format!("{}. {}", d.suspect, d.instruction);
                if live && !self.chains[i].closed {
                    self.messenger.send(
                        job,
                        Nudge {
                            text: format!("a diagnosis of your last steps: {text}"),
                            call: None,
                        },
                    );
                }
                self.chains[i].diagnosis = Some(d);
                format!("diagnosis: {text}")
            }
            Err(e) => format!("no diagnosis: {e}"),
        };
        let rung = Rung::Diagnose;
        let line = cut(&text, 200);
        self.chains[i].trail.push(line.clone());
        jobs.record_recovery(job, &line);
        self.change(now, job, &format!("watchdog: {line}"));
        self.ledger(i, job, "reply", &kind, Some(rung), &text, None, now, report);
    }

    fn change(&mut self, at: Millis, job: JobId, text: &str) {
        if self.changes.len() == CHANGES_KEPT {
            self.changes.pop_front();
        }
        self.changes.push_back(Change {
            at,
            job: Some(job),
            session: String::new(),
            kind: ChangeKind::Watchdog,
            text: text.to_string(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn ledger(
        &self,
        i: usize,
        job: JobId,
        event: &str,
        alarm: &str,
        rung: Option<Rung>,
        detail: &str,
        next_job: Option<JobId>,
        now: Millis,
        report: &mut WatchReport,
    ) {
        let Some(ledger) = &self.ledger else { return };
        let entry = WatchdogEntry {
            job,
            session: Some(self.chains[i].session),
            event,
            alarm,
            rung: rung.map(Rung::as_str),
            detail,
            next_job,
        };
        if let Err(e) = ledger.record_watchdog(&entry, now) {
            report.errors.push(e);
        }
    }
}

/// How the alarm of a job that ended begins.
const ENDED: &str = "the job ended: ";

/// What a successor that should plan first is told.
const REPLAN: &str = "Earlier sessions got stuck on this task. Before changing code, write a \
short numbered plan that splits the task into smaller steps and avoids what failed, then \
carry out the first step and continue.\n";

/// How strong a model family is, for models not on the escalation list.
fn tier(key: &str) -> Option<u8> {
    [
        ("haiku", 0),
        ("sonnet", 1),
        ("opus", 2),
        ("fable", 3),
        ("mythos", 3),
    ]
    .into_iter()
    .find(|(name, _)| key.contains(name))
    .map(|(_, t)| t)
}

fn gauges_of(fact: &JobFact, compactions: u32) -> Gauges {
    Gauges {
        context: fact.context,
        compactions,
        waiting_on_user: fact.session_state == Some(SessionState::RequiresAction),
    }
}

/// What the agent is told on the nudge rung.
fn nudge_text(alarm: &WatchAlarm) -> String {
    let advice = match alarm.kind {
        AlarmKind::Loop => {
            "This call was not run again. Read the error, then try a different fix or approach instead of repeating it."
        }
        AlarmKind::Oscillation => {
            "Your changes are not fixing it. Step back, re-read the error and change approach."
        }
        AlarmKind::Stall => "Sum up what you have learned and make the next concrete change.",
        AlarmKind::Burn => {
            "Work in smaller steps: read only what you need and avoid re-running expensive commands."
        }
        AlarmKind::Context => "Wrap up the current step.",
    };
    format!("{}. {advice}", alarm.detail)
}

fn title_of(prompt: &str) -> String {
    first_line(prompt, "job")
}

fn first_line(s: &str, empty: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty());
    cut(line.unwrap_or(empty), 120)
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn short(commit: &str) -> String {
    commit.chars().take(10).collect()
}
